#if os(macOS)
import Foundation
import AVFoundation
import FaceMac
import FaceAudioKit
import CogletSurface
import CogletTypes

// face-acoustic: M2b real-Mac acoustic proof harness (ADR-0044).
//
// Drives the REAL FaceSurfaceHost + FaceSurfaceBackend + NativeFaceDuplex
// (FaceAudioKit, one shared AVAudioEngine graph on real hardware) from the frozen
// conformance speech timelines (spec/fixtures/speech/timelines.json) and measures
// the acoustic path through a physical microphone. Test seams, declared honestly:
//   * synthesis route "acoustic-tone": deterministic 1 kHz word bursts with
//     time-aligned spans, NOT a voice;
//   * recognition route "acoustic-scripted": delivers the timeline's scripted
//     partial/final text, it is NOT speech recognition;
//   * a second AVAudioEngine plays a 1.8 kHz stimulus standing in for the user's voice.
// It must run from a signed .app bundle (microphone TCC grant); see
// scripts/acoustic/run.sh. It is never shipped; it opens no network route.

// MARK: time

func nowNS() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
func msBetween(_ a: UInt64, _ b: UInt64) -> Double { (Double(b) - Double(a)) / 1e6 }
func round3(_ x: Double) -> Double { (x * 1000).rounded() / 1000 }

// MARK: audio seams

/// Wraps the real NativeFaceDuplex and timestamps every capture batch / render counter
/// change as the host polls it (the host polls at ~1 ms in this harness).
@MainActor final class TapDuplex: FaceDuplexIO {
    let inner: NativeFaceDuplex
    var batches: [(t: UInt64, n: Int)] = []
    var samples: [Int16] = []
    var renderLog: [(t: UInt64, frames: UInt64)] = []
    private var lastRendered: UInt64 = 0
    init(inner: NativeFaceDuplex) { self.inner = inner }
    var renderedFrames: UInt64 {
        let f = inner.renderedFrames
        if f != lastRendered { lastRendered = f; renderLog.append((nowNS(), f)) }
        return f
    }
    var isRunning: Bool { inner.isRunning }
    var generation: UInt64 { inner.generation }
    var captureFailed: Bool { inner.captureFailed }
    var onVoiceActivity: ((Bool) -> Void)? {
        get { inner.onVoiceActivity }
        set { inner.onVoiceActivity = newValue }
    }
    func start() throws { try inner.start() }
    func stop() { inner.stop() }
    func enqueue(_ s: [Int16]) throws { try inner.enqueue(s) }
    func flush() { inner.flush() }
    func readCapture() -> [Int16] {
        let s = inner.readCapture()
        if !s.isEmpty { batches.append((nowNS(), s.count)); samples.append(contentsOf: s) }
        _ = renderedFrames
        return s
    }
    func beginRun() { batches = []; samples = []; renderLog = [] }
    /// Estimated host time of every captured sample (newest sample ~ poll time).
    func sampleTimes() -> [UInt64] {
        var out = [UInt64](); out.reserveCapacity(samples.count)
        for b in batches {
            for j in 0..<b.n {
                let back = UInt64(Double(b.n - 1 - j) / 24000.0 * 1e9)
                out.append(b.t > back ? b.t - back : 0)
            }
        }
        return out
    }
}

let toneRate = 24000.0
func tone(freq: Double, ms: Int, amp: Double = 0.6, fadeMS: Int = 3) -> [Int16] {
    let n = ms * 24, f = fadeMS * 24
    return (0..<n).map { i in
        var g = 1.0
        if i < f { g = Double(i) / Double(f) } else if i >= n - f { g = Double(n - 1 - i) / Double(f) }
        return Int16(amp * g * 32767.0 * sin(2 * .pi * freq * Double(i) / toneRate))
    }
}
/// 5 words of 500 ms tone followed by a 100 ms gap (period 600 ms, 3000 ms item).
let wordPeriodMS = 600, wordToneMS = 500, wordCount = 5
func wordedItem() -> (samples: [Int16], spans: [SpeechObservations.Span]) {
    var s = [Int16](); var spans = [SpeechObservations.Span]()
    for k in 0..<wordCount {
        s += tone(freq: 1000, ms: wordToneMS)
        s += [Int16](repeating: 0, count: (wordPeriodMS - wordToneMS) * 24)
        spans.append(.init(endMS: UInt64(k * wordPeriodMS + wordToneMS), text: k == wordCount - 1 ? "w\(k)" : "w\(k) "))
    }
    return (s, spans)
}
let itemText = (0..<wordCount).map { "w\($0)" }.joined(separator: " ")

@MainActor final class ToneSynthesis: FaceSynthesis {
    let route = "acoustic-tone"; let usesRemoteAudio = false
    func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, Swift.Error>) -> Void) -> @MainActor () -> Void {
        let item = wordedItem()
        completion(.success(.init(samples: item.samples, spans: item.spans)))
        return {}
    }
}

@MainActor final class ScriptedRecognition: FaceRecognition, FaceRecognitionInput {
    let route = "acoustic-scripted"; let usesRemoteAudio = false
    var receive: (@MainActor (FaceRecognitionUpdate) throws -> Void)?
    var firstAppendNS: UInt64?
    var appendedFrames = 0
    var finishNS: UInt64?
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
               failed: @escaping @MainActor () throws -> Void) throws -> any FaceRecognitionInput {
        self.receive = receive; firstAppendNS = nil; appendedFrames = 0; finishNS = nil; return self
    }
    func append(_ s: [Int16]) throws { if firstAppendNS == nil { firstAppendNS = nowNS() }; appendedFrames += s.count }
    func finish() { finishNS = nowNS() }
    func cancel() {}
}

final class MemoryLedger: SurfaceLedgerStore {
    private var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ value: [UInt8]) throws { bytes = value }
}

// MARK: acoustic analysis

/// Goertzel magnitude of `freq` over 5 ms windows, 1 ms hop. Time = window centre.
struct Envelope {
    var t: [UInt64] = []
    var v: [Double] = []
}
func goertzel(_ x: ArraySlice<Int16>, freq: Double) -> Double {
    let w = 2 * Double.pi * freq / toneRate, c = 2 * cos(w)
    var s1 = 0.0, s2 = 0.0
    for q in x { let s = Double(q) / 32768.0 + c * s1 - s2; s2 = s1; s1 = s }
    let power = s1 * s1 + s2 * s2 - c * s1 * s2
    return (max(power, 0)).squareRoot() * 2 / Double(x.count)
}
func envelope(samples: [Int16], times: [UInt64], freq: Double) -> Envelope {
    var e = Envelope(); let win = 120, hop = 24
    guard samples.count > win else { return e }
    var i = 0
    while i + win <= samples.count {
        e.v.append(goertzel(samples[i..<(i + win)], freq: freq))
        e.t.append(times[i + win / 2]); i += hop
    }
    return e
}
func percentile(_ a: [Double], _ p: Double) -> Double {
    guard !a.isEmpty else { return 0 }
    let s = a.sorted(); return s[min(s.count - 1, Int(p * Double(s.count - 1) + 0.5))]
}
/// First time at/after `from` with `need` consecutive windows above `thr`.
func onsetTime(_ e: Envelope, from: UInt64, thr: Double, need: Int = 3) -> UInt64? {
    var run = 0, start = 0
    for i in 0..<e.v.count where e.t[i] >= from {
        if e.v[i] > thr { if run == 0 { start = i }; run += 1; if run >= need { return e.t[start] } } else { run = 0 }
    }
    return nil
}
/// Start of the first low run of >= `minLowMS` at/after `from` (acoustic stop; word gaps are 100 ms,
/// so the default must exceed them only when the stream is truly finished, see `lowRunMS`).
func lowRunStart(_ e: Envelope, from: UInt64, thr: Double, minLowMS: Int) -> UInt64? {
    var run = 0, start = 0
    for i in 0..<e.v.count where e.t[i] >= from {
        if e.v[i] < thr { if run == 0 { start = i }; run += 1; if run >= minLowMS { return e.t[start] } } else { run = 0 }
    }
    return nil
}
/// Complete high bursts [start, end] (ns) in the envelope, merging dips shorter than `mergeMS`.
func bursts(_ e: Envelope, from: UInt64, to: UInt64, thr: Double, mergeMS: Int = 20) -> [(UInt64, UInt64)] {
    var out: [(UInt64, UInt64)] = []; var cur: (UInt64, UInt64)?; var low = 0; var lastHigh: UInt64 = 0
    for i in 0..<e.v.count where e.t[i] >= from && e.t[i] <= to {
        if e.v[i] > thr {
            if cur == nil { cur = (e.t[i], e.t[i]) }; lastHigh = e.t[i]; low = 0
        } else if cur != nil {
            low += 1
            if low >= mergeMS { out.append((cur!.0, lastHigh)); cur = nil }
        }
    }
    if let c = cur { out.append((c.0, lastHigh)) }
    return out
}

// MARK: harness

struct Options {
    var timelines = "spec/fixtures/speech/timelines.json"
    var out = "."
    var barge = 3, capture = 3, stimulus = 3
    var offsets = [100, 1000, 1500, 2100]
    var aec = false
    var label = "run"
    var calibrateOnly = false
    var seed: UInt64 = 1
}
func parse() -> Options {
    var o = Options(); var a = Array(CommandLine.arguments.dropFirst()); 
    while !a.isEmpty {
        let k = a.removeFirst()
        func v() -> String { a.isEmpty ? "" : a.removeFirst() }
        switch k {
        case "--timelines": o.timelines = v()
        case "--out": o.out = v()
        case "--barge-runs": o.barge = Int(v()) ?? o.barge
        case "--capture-runs": o.capture = Int(v()) ?? o.capture
        case "--stimulus-runs": o.stimulus = Int(v()) ?? o.stimulus
        case "--offsets": o.offsets = v().split(separator: ",").compactMap { Int($0) }
        case "--aec": o.aec = v() == "on"
        case "--label": o.label = v()
        case "--seed": o.seed = UInt64(v()) ?? 1
        case "--calibrate-only": o.calibrateOnly = true
        default: FileHandle.standardError.write(Data("unknown option \(k)\n".utf8))
        }
    }
    return o
}
struct SplitMix { var s: UInt64
    mutating func next() -> UInt64 { s &+= 0x9E3779B97F4A7C15; var z = s; z = (z ^ (z >> 30)) &* 0xBF58476D1CE4E5B9; z = (z ^ (z >> 27)) &* 0x94D049BB133111EB; return z ^ (z >> 31) } }

func hex32() -> String { UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() }
func jsonLine(_ obj: [String: Any]) -> String {
    let d = try! JSONSerialization.data(withJSONObject: obj, options: [.sortedKeys])
    return String(decoding: d, as: UTF8.self)
}
func appendLine(_ path: String, _ s: String) {
    if !FileManager.default.fileExists(atPath: path) { FileManager.default.createFile(atPath: path, contents: nil) }
    let h = FileHandle(forWritingAtPath: path)!; h.seekToEndOfFile(); h.write(Data((s + "\n").utf8)); try? h.close()
}

struct Fail: Swift.Error, CustomStringConvertible { let description: String }

/// Plays a 1.8 kHz burst on a second engine at a scheduled host time (user-voice stand-in).
@MainActor final class Stimulus {
    let engine = AVAudioEngine(); let player = AVAudioPlayerNode()
    let format = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1)!
    var tb = mach_timebase_info_data_t()
    init() throws {
        mach_timebase_info(&tb)
        engine.attach(player); engine.connect(player, to: engine.mainMixerNode, format: format)
        engine.prepare(); try engine.start()
    }
    func schedule(atNS: UInt64, ms: Int) {
        let s = tone(freq: 1800, ms: ms, amp: 0.6)
        let buf = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(s.count))!
        buf.frameLength = AVAudioFrameCount(s.count)
        for i in 0..<s.count { buf.floatChannelData![0][i] = Float(s[i]) / 32768 }
        player.scheduleBuffer(buf, at: nil, options: [], completionHandler: nil)
        let ticks = atNS * UInt64(tb.denom) / UInt64(tb.numer)
        player.play(at: AVAudioTime(hostTime: ticks))
    }
    func stop() { player.stop(); engine.stop() }
}

@MainActor final class Harness {
    let o: Options
    let kit: FaceAudioKit
    let tap: TapDuplex
    var rng: SplitMix
    var thr1k = 0.0, thr18k = 0.0
    var cal: [String: Any] = [:]
    let startNS = nowNS()
    var timelines: [[String: Any]] = []
    let owner = "coglet:" + String(repeating: "b", count: 64)
    init(_ o: Options) {
        self.o = o
        kit = FaceAudioKit(configuration: .init(echoCancellation: o.aec))
        tap = TapDuplex(inner: NativeFaceDuplex(kit: kit))
        rng = SplitMix(s: o.seed)
    }
    func clockUS() -> UInt64 { (nowNS() - startNS) / 1000 }
    func sleepMS(_ ms: Double, until: UInt64? = nil) async { try? await Task.sleep(for: .microseconds(Int(ms * 1000))) }

    /// A fresh host per run: new graph, new ledger, new authority epoch.
    func makeHost() throws -> (FaceSurfaceHost, FaceSurfaceBackend, ScriptedRecognition) {
        let synth = ToneSynthesis(), rec = ScriptedRecognition()
        let backend = FaceSurfaceBackend(audio: tap, synthesis: synth, renderer: .init { _ in },
                                         sttRoute: rec.route, ttsRoute: synth.route)
        let ops = FaceSurfaceHost.operations
        let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "a", count: 64), owner: owner,
            store: MemoryLedger(), backend: backend,
            grants: [.init(coglet: owner, operations: Set(ops.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"],
                           expires: 1_000_000_000_000)], clock: { [self] in clockUS() })
        try host.start()
        healthNext = 0
        let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
        try host.receive(SurfaceWire.encode(.message("hello_reply", .record([("selection", .record([("version", .text("0.1"))])),
            ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
        _ = host.drain()
        try host.activateDuplex(localConsent: true)
        return (host, backend, rec)
    }
    func speakWire(host: FaceSurfaceHost, id: String, stream: String) throws -> [UInt8] {
        let action = CBORValue.record([("id", .text(id)), ("coglet", .text(owner)), ("capability", .text("speech.speak_chunk")),
            ("deadline", .unsigned(clockUS() + 60_000_000)), ("correlation_id", .text(id)),
            ("args", .record([("stream", .text(stream)), ("chunk", .unsigned(0)), ("text", .text(itemText)), ("final", .boolean(true))]))])
        return try SurfaceWire.encode(.message("act", .record([("grant_epoch", .unsigned(host.grantEpoch)), ("action", action)])), from: .runtime)
    }

    struct Ev { var t: UInt64; var kind: String; var payload: CBORValue? }
    func drainEvents(_ host: FaceSurfaceHost, into evs: inout [Ev]) {
        for f in host.drain() {
            guard let m = try? SurfaceWire.decode(f, from: .surface), m["type"] == .text("event"), let b = m["body"],
                  case .text(let kind)? = b["kind"] else { continue }
            evs.append(Ev(t: nowNS(), kind: kind, payload: b["payload"]))
        }
    }
    var healthNext: UInt64 = 0
    /// The runtime peer owes the surface a health frame at least every 3 s (SDK session rule).
    func sendHealth(_ host: FaceSurfaceHost) throws {
        let now = nowNS()
        guard now >= healthNext else { return }
        healthNext = now + 500_000_000
        try host.receive(SurfaceWire.encode(.message("health", .record([("state", .text("ready")), ("clock", .unsigned(clockUS())),
            ("pending", .unsigned(0)), ("correlation_id", .text(hex32()))])), from: .runtime))
    }
    /// Pump the host (as its 20 ms timer would, but at 1 ms for resolution) until `deadline`.
    func pump(_ host: FaceSurfaceHost, until deadline: UInt64, into evs: inout [Ev]) async throws {
        while nowNS() < deadline {
            try sendHealth(host)
            try host.pump(); drainEvents(host, into: &evs)
            await sleepMS(1)
        }
    }
    func uint(_ v: CBORValue?) -> UInt64? { if case .unsigned(let u)? = v { return u }; return nil }

    // MARK: calibration

    func calibrate() async throws {
        let (host, _, _) = try makeHost()
        var evs: [Ev] = []
        try await pump(host, until: nowNS() + 800_000_000, into: &evs) // settle
        tap.beginRun(); let tn0 = nowNS()
        try await pump(host, until: tn0 + 1_000_000_000, into: &evs)
        let times = tap.sampleTimes()
        let nEnv1 = envelope(samples: tap.samples, times: times, freq: 1000)
        let nEnv2 = envelope(samples: tap.samples, times: times, freq: 1800)
        // Tone: 1.2 s continuous 1 kHz straight through the shared graph.
        tap.beginRun(); let tt0 = nowNS()
        try tap.enqueue(tone(freq: 1000, ms: 1200))
        try await pump(host, until: tt0 + 1_700_000_000, into: &evs)
        let times2 = tap.sampleTimes()
        let tEnv = envelope(samples: tap.samples, times: times2, freq: 1000)
        let mid = tEnv.v.enumerated().filter { tEnv.t[$0.offset] > tt0 + 500_000_000 && tEnv.t[$0.offset] < tt0 + 1_100_000_000 }.map(\.element)
        let n99 = percentile(nEnv1.v, 0.99), t10 = percentile(mid, 0.10), tmed = percentile(mid, 0.5)
        thr1k = (n99 * t10).squareRoot()
        cal["noise_1k_p50"] = round3(percentile(nEnv1.v, 0.5)); cal["noise_1k_p99"] = round3(n99)
        cal["tone_1k_p10"] = round3(t10); cal["tone_1k_median"] = round3(tmed)
        cal["snr_1k_db"] = round3(20 * log10(max(t10, 1e-9) / max(n99, 1e-9)))
        cal["threshold_1k"] = round3(thr1k)
        cal["noise_1.8k_p99"] = round3(percentile(nEnv2.v, 0.99))
        // Stimulus level through the second engine.
        let stim = try Stimulus()
        try await pump(host, until: nowNS() + 400_000_000, into: &evs)
        tap.beginRun(); let ts0 = nowNS() + 300_000_000
        stim.schedule(atNS: ts0, ms: 800)
        try await pump(host, until: ts0 + 1_300_000_000, into: &evs)
        let sEnv = envelope(samples: tap.samples, times: tap.sampleTimes(), freq: 1800)
        let smid = sEnv.v.enumerated().filter { sEnv.t[$0.offset] > ts0 + 300_000_000 && sEnv.t[$0.offset] < ts0 + 700_000_000 }.map(\.element)
        stim.stop()
        let s10 = percentile(smid, 0.10), n99s = percentile(nEnv2.v, 0.99)
        thr18k = (n99s * s10).squareRoot()
        cal["stim_1.8k_p10"] = round3(s10); cal["snr_1.8k_db"] = round3(20 * log10(max(s10, 1e-9) / max(n99s, 1e-9)))
        cal["threshold_1.8k"] = round3(thr18k)
        try? host.close()
    }

    // MARK: runs

    /// One conformance timeline on the real graph. `offset` re-times the barge on the `play` timeline.
    func timelineRun(name: String, offset: Int, index: Int, kind: String) async throws -> [String: Any] {
        guard let tl = timelines.first(where: { $0["name"] as? String == name }),
              let steps = tl["steps"] as? [[String: Any]], let expected = tl["expected"] as? [String: Any] else { throw Fail(description: "no timeline \(name)") }
        let (host, backend, rec) = try makeHost()
        var evs: [Ev] = []
        try await pump(host, until: nowNS() + 600_000_000, into: &evs) // pre-roll capture
        tap.beginRun()
        try await pump(host, until: nowNS() + 300_000_000, into: &evs) // quiet lead-in, scored as per-run noise
        let startAt = steps.first { $0["input"] as? String == "start" }?["at_ms"] as? Int ?? 0
        let hasPlay = steps.contains { $0["input"] as? String == "play" }
        let shift = hasPlay ? offset - startAt : 0
        let t0 = nowNS(); var tAct: UInt64 = 0, tPress: UInt64 = 0, tPressDone: UInt64 = 0, tRelease: UInt64 = 0, tEndDelivered: UInt64 = 0
        var finalDeliver: UInt64 = 0
        let item = hex32(), stream = hex32()
        var recvErr: String?
        for step in steps {
            let input = step["input"] as! String, at = (step["at_ms"] as! Int) + ((step["at_ms"] as! Int) >= startAt ? shift : 0)
            let due = t0 + UInt64(max(at, 0)) * 1_000_000
            try await pump(host, until: due, into: &evs)
            switch input {
            case "play":
                tAct = nowNS()
                try host.receive(speakWire(host: host, id: item, stream: stream))
            case "start":
                tPress = nowNS()
                _ = try host.beginCapture(recognition: rec, positionMS: UInt64(step["position_ms"] as? Int ?? 0))
                tPressDone = nowNS()
            case "partial":
                try rec.receive?(.init(revision: UInt64(step["revision"] as! Int), text: step["text"] as! String, final: false, positionMS: 0))
            case "end":
                tRelease = nowNS(); host.finishCapture(); finalDeliver = nowNS()
                try rec.receive?(.init(revision: UInt64(step["revision"] as! Int), text: step["text"] as! String, final: true,
                                       positionMS: UInt64(step["position_ms"] as? Int ?? 0)))
                tEndDelivered = nowNS()
            default: break // barge_in is emitted by the real native reflex, never injected
            }
            try host.pump(); drainEvents(host, into: &evs)
        }
        if tAct == 0 { tAct = t0 }
        // Observe the tail: acoustic stop + ring-down.
        try await pump(host, until: nowNS() + (hasPlay ? 1_500_000_000 : 300_000_000), into: &evs)
        let discarded = backend.discardedRecognitionUpdates
        // speech.position is reported every native tick while audio plays; the frozen
        // timeline lists one per transition, so collapse consecutive repeats.
        var kinds: [String] = []
        var collapsed: [Ev] = []
        for e in evs where !(e.kind == "speech.position" && kinds.last == "speech.position") { kinds.append(e.kind); collapsed.append(e) }
        let positionEvents = evs.filter { $0.kind == "speech.position" }.count
        let wantKinds = expected["events"] as? [String] ?? []
        // The first speech.position may arrive before/after; compare ordered kinds exactly.
        var conformant = kinds == wantKinds && Int(discarded) == (expected["discarded"] as? Int ?? -1)
        var r: [String: Any] = ["kind": kind, "timeline": name, "index": index, "offset_ms": offset,
                                "events": collapsed.map { ["t_ms": round3(msBetween(t0, $0.t)), "kind": $0.kind] },
                                "position_events": positionEvents,
                                "discarded": discarded, "recv_error": recvErr as Any]
        // PTT metrics (adapter + capture path).
        if let fa = rec.firstAppendNS { r["ptt_press_to_first_capture_ms"] = round3(msBetween(tPress, fa)) }
        if let s = evs.first(where: { $0.kind == "speech.start" }) { r["ptt_press_to_start_event_ms"] = round3(msBetween(tPress, s.t)) }
        if let e = evs.first(where: { $0.kind == "speech.end" }) { r["ptt_release_to_end_event_ms"] = round3(msBetween(tRelease, e.t)) }
        r["ptt_final_delivery_call_ms"] = round3(msBetween(finalDeliver, tEndDelivered))
        r["capture_samples"] = tap.samples.count
        r["ptt_press_call_ms"] = round3(msBetween(tPress, tPressDone))
        if hasPlay {
            let times = tap.sampleTimes()
            let env = envelope(samples: tap.samples, times: times, freq: 1000)
            let thr = thr1k
            let preNoise = (0..<env.v.count).filter { env.t[$0] < t0 }.map { env.v[$0] }.max() ?? 0
            r["pre_noise_max"] = round3(preNoise); r["noise_ok"] = preNoise < thr
            let swOnset = tap.renderLog.first { $0.frames > (tap.renderLog.first?.frames ?? 0) }
            // Software render start/stop from the render counter (polled at ~1 ms).
            let firstRender = tap.renderLog.first?.t
            let lastRender = tap.renderLog.last?.t
            if let fr = firstRender { r["play_to_first_render_ms"] = round3(msBetween(tAct, fr)) }
            _ = swOnset
            if let lr = lastRender { r["barge_to_render_stop_ms"] = round3(msBetween(tPress, lr)) }
            var on = onsetTime(env, from: tAct, thr: thr)
            if let o = on { on = o; r["play_to_acoustic_onset_ms"] = round3(msBetween(tAct, o)) }
            let tonePresentAtBarge: Bool = {
                for i in 0..<env.v.count where env.t[i] >= tPress && env.t[i] < tPress + 15_000_000 && env.v[i] > thr { return true }
                return false
            }()
            r["tone_present_at_barge"] = tonePresentAtBarge
            // Acoustic stop: first low run of >= 150 ms (> the 100 ms word gap) after the barge.
            let stop = lowRunStart(env, from: tPress, thr: thr, minLowMS: 150)
            if let st = stop, tonePresentAtBarge { r["barge_to_acoustic_stop_ms"] = round3(msBetween(tPress, st)) }
            if let o = on, let st = stop {
                let heard = msBetween(o, st); r["acoustic_heard_ms"] = round3(heard)
                if let b = evs.first(where: { $0.kind == "speech.barge_in" }), let played = uint(b.payload?["played_ms"]) {
                    r["reported_played_ms"] = played
                    r["played_minus_acoustic_ms"] = round3(Double(played) - heard)
                    let credited = (0..<wordCount).filter { UInt64($0 * wordPeriodMS + wordToneMS) <= played }.count
                    let bs = bursts(env, from: o, to: st + 50_000_000, thr: thr)
                    // A burst is fully heard if it lasted >= 470 of its 500 ms.
                    let full = bs.filter { msBetween($0.0, $0.1) >= 470 }.count
                    r["credited_words"] = credited; r["acoustic_full_words"] = full
                    r["over_credited_words"] = max(0, credited - full)
                    r["word_exact"] = credited == full
                }
            }
            // Decimated raw envelope (5 ms bins, relative to the play action) for plotting/replay.
            var dec: [[Double]] = []
            var i = 0
            while i < env.v.count { dec.append([round3(msBetween(tAct, env.t[i])), round3(env.v[i])]); i += 5 }
            r["envelope_1k"] = dec
            r["barge_event_ok"] = evs.contains { $0.kind == "speech.barge_in" }
            conformant = conformant && (r["barge_event_ok"] as? Bool ?? false)
        }
        r["conformant"] = conformant
        try? host.close()
        return r
    }

    /// Stimulus (user-voice stand-in) onset -> first capture frames carrying it.
    func stimulusRun(index: Int) async throws -> [String: Any] {
        let stim = try Stimulus()
        let (host, _, rec) = try makeHost()
        var evs: [Ev] = []
        try await pump(host, until: nowNS() + 800_000_000, into: &evs)
        tap.beginRun()
        let tPress = nowNS()
        _ = try host.beginCapture(recognition: rec, positionMS: 0)
        let jitter = Int(rng.next() % 40)
        let sched = tPress + UInt64(200 + jitter) * 1_000_000
        stim.schedule(atNS: sched, ms: 300)
        try await pump(host, until: sched + 700_000_000, into: &evs)
        let tRelease = nowNS(); host.finishCapture()
        try rec.receive?(.init(revision: 1, text: "ok", final: true, positionMS: 700))
        try host.pump(); drainEvents(host, into: &evs)
        let env = envelope(samples: tap.samples, times: tap.sampleTimes(), freq: 1800)
        var r: [String: Any] = ["kind": "stimulus", "index": index]
        if let on = onsetTime(env, from: sched - 50_000_000, thr: thr18k) {
            r["stimulus_scheduled_to_capture_ms"] = round3(msBetween(sched, on))
        } else { r["stimulus_missed"] = true }
        if let fa = rec.firstAppendNS { r["ptt_press_to_first_capture_ms"] = round3(msBetween(tPress, fa)) }
        r["ptt_release_to_end_event_ms"] = evs.first { $0.kind == "speech.end" }.map { round3(msBetween(tRelease, $0.t)) } as Any
        stim.stop(); try? host.close()
        return r
    }
}

@main struct FaceAcousticMain {
    @MainActor static func main() async {
        let o = parse()
        try? FileManager.default.createDirectory(atPath: o.out, withIntermediateDirectories: true)
        let runsPath = o.out + "/runs.jsonl", sessionPath = o.out + "/session.json"
        func done(_ code: Int32, _ msg: String) -> Never {
            appendLine(o.out + "/log.txt", msg); print(msg); exit(code)
        }
        if MicrophonePermission.status != .authorized {
            if await !MicrophonePermission.request() || MicrophonePermission.status != .authorized {
                done(3, "microphone permission not granted (status \(MicrophonePermission.status.rawValue))")
            }
        }
        let h = Harness(o)
        do {
            h.timelines = try JSONSerialization.jsonObject(with: Data(contentsOf: URL(fileURLWithPath: o.timelines))) as! [[String: Any]]
            let devices = (try? AudioDevices.list().map { ["name": $0.name, "uid": $0.uid, "in": $0.hasInput, "out": $0.hasOutput] }) ?? []
            try await h.calibrate()
            var session: [String: Any] = ["label": o.label, "aec": o.aec, "calibration": h.cal, "devices": devices,
                "host": ProcessInfo.processInfo.hostName, "os": ProcessInfo.processInfo.operatingSystemVersionString,
                "offsets_ms": o.offsets, "started": ISO8601DateFormatter().string(from: Date()),
                "tool": "face-acoustic", "seed": o.seed]
            let snr = h.cal["snr_1k_db"] as? Double ?? 0
            session["acoustic_coupling_ok"] = snr >= 12
            try JSONSerialization.data(withJSONObject: session, options: [.sortedKeys, .prettyPrinted]).write(to: URL(fileURLWithPath: sessionPath))
            if o.calibrateOnly { done(0, "calibration only: snr_1k_db=\(snr)") }
            if snr < 12 { done(4, "insufficient speaker->microphone coupling: SNR \(snr) dB < 12 dB; refusing to measure") }
            var idx = 0
            for rep in 0..<o.barge {
                for off in o.offsets {
                    let jitter = off == 100 ? 0 : Int(h.rng.next() % 41) - 20
                    let r = try await h.timelineRun(name: "barge-in-stop", offset: off + jitter, index: idx, kind: "barge")
                    appendLine(runsPath, jsonLine(r)); idx += 1
                    print("barge rep=\(rep) off=\(off + jitter) conformant=\(r["conformant"] ?? "?") ack=\(r["barge_to_acoustic_stop_ms"] ?? "-")")
                    await h.sleepMS(300)
                }
            }
            for i in 0..<o.capture {
                let r = try await h.timelineRun(name: "late-partial-and-stale-revision", offset: 0, index: i, kind: "capture")
                appendLine(runsPath, jsonLine(r)); print("capture \(i) conformant=\(r["conformant"] ?? "?")")
                await h.sleepMS(300)
            }
            for i in 0..<o.stimulus {
                let r = try await h.stimulusRun(index: i)
                appendLine(runsPath, jsonLine(r)); print("stimulus \(i) \(r["stimulus_scheduled_to_capture_ms"] ?? "missed")")
                await h.sleepMS(300)
            }
            done(0, "complete")
        } catch { done(2, "error: \(error)") }
    }
}
#else
@main struct FaceAcousticMain { static func main() { print("face-acoustic requires macOS") } }
#endif
