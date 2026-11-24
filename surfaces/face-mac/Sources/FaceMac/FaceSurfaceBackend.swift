import Foundation
import CogletSurface
import CogletTypes

/// A single shared duplex graph. Counters count submitted render frames, never
/// wall time or queued/synthesized samples. All methods are main-actor confined.
@MainActor public protocol FaceDuplexIO: AnyObject {
    var renderedFrames: UInt64 { get }
    var isRunning: Bool { get }
    var generation: UInt64 { get }
    var captureFailed: Bool { get }
    var onVoiceActivity: ((Bool) -> Void)? { get set }
    func start() throws
    func stop()
    func enqueue(_ samples: [Int16]) throws
    func flush()
    func readCapture() -> [Int16]
}
public struct FaceSynthesizedAudio: Sendable {
    public let samples: [Int16] // canonical mono 24 kHz
    public let spans: [SpeechObservations.Span]
    public init(samples: [Int16], spans: [SpeechObservations.Span] = []) { self.samples = samples; self.spans = spans }
}
/// Product selects its route/voice. No implicit network/provider fallback.
/// Completion carries the captured request custody; cancellation must stop the
/// provider too. The backend independently fences a provider that ignores it.
@MainActor public protocol FaceSynthesis: AnyObject {
    var route: String { get }
    var usesRemoteAudio: Bool { get }
    func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, Swift.Error>) -> Void) -> @MainActor () -> Void
}
public struct FaceCompletion {
    public let owner: String
    public let id: String
    public let epoch: UInt64
    public let result: NativeSurfaceResult
}

/// SurfaceSession calls this only on MainActor. @preconcurrency enforces that
/// boundary dynamically when satisfying the SDK's nonisolated backend protocol.
@MainActor public final class FaceSurfaceBackend: @preconcurrency SurfaceNativeBackend {
    private struct Item {
        let owner: String; let epoch: UInt64; let id: String; let stream: String
        let chunk: UInt64; let text: String; let deadline: UInt64
        var audio: FaceSynthesizedAudio?
        var base: UInt64?
        var playedMS: UInt64 = 0
    }
    private let audio: any FaceDuplexIO
    private let sttRoute: String
    private let ttsRoute: String
    private let synthesis: any FaceSynthesis
    public let renderer: FaceExpressionRenderer
    private var owner: String?
    private var epoch: UInt64 = 0
    private var remoteAudioConsent = false
    private var emit: ((String, CBORValue) throws -> Void)?
    private var history = FaceHeardHistory()
    private var items: [Item] = []
    private var cancellation: (@MainActor () -> Void)?
    private var generation = UUID()
    private var completions: [FaceCompletion] = []
    private var stream: String?
    private var nextChunk: UInt64 = 0
    private var inputClosed = false
    private var closedStreams = Set<String>()
    private var streamPosition: UInt64 = 0
    private var idleSince: UInt64 = 0
    private var graph: UInt64?
    private var captureGeneration = UUID()
    private var recognitionInput: (any FaceRecognitionInput)?
    private var captureOpen = false
    private var capturedFrames: UInt64 = 0
    var capturePositionMS: UInt64 { capturedFrames / 24 }
    private var captureNormalizer = SpeechObservations()
    private var utterance: String?
    private var captureRevision: UInt64 = 0
    private var captureStartMS: UInt64 = 0
    private var captureDeadline: UInt64?
    private var lastTick: UInt64 = 0
    private var captureClock: (() -> UInt64)?
    func installCaptureClock(_ clock: @escaping () -> UInt64) { captureClock = clock }
    /// Host-installed effect authority. It expires grants/actions through the SDK
    /// (which may synchronously stop this backend), then returns a FRESH clock
    /// sample and the SDK grant epoch. Absent for direct backend use.
    typealias EffectAuthority = @MainActor () throws -> (now: UInt64, epoch: UInt64)
    private var effectAuthority: EffectAuthority?
    func installEffectAuthority(_ authority: @escaping EffectAuthority) { effectAuthority = authority }
    /// Re-validate custody after any blocking durable save and immediately before
    /// a provider call or audio effect. The authority may reenter stop(); only
    /// unchanged generation/owner/epoch plus a live deadline permits the effect.
    private func reauthorize(generation token: UUID, owner expectedOwner: String, epoch expectedEpoch: UInt64,
                             deadline: UInt64, fallbackNow: UInt64) throws {
        var now = captureClock?() ?? fallbackNow
        if let effectAuthority {
            let sampled = try effectAuthority()
            guard sampled.epoch == expectedEpoch else { throw NativeSurfaceError("permission_denied") }
            now = sampled.now
        }
        guard generation == token, owner == expectedOwner, epoch == expectedEpoch else { throw NativeSurfaceError("cancelled") }
        guard deadline > now else { throw NativeSurfaceError("deadline_exceeded") }
    }
    /// Bounded, non-text diagnostics; late callbacks never log transcripts.
    public private(set) var discardedRecognitionUpdates: UInt64 = 0
    public private(set) var recognitionFailures: UInt64 = 0
    public init(audio: any FaceDuplexIO, synthesis: any FaceSynthesis, renderer: FaceExpressionRenderer,
                sttRoute: String = "local", ttsRoute: String = "local") {
        self.audio = audio; self.synthesis = synthesis; self.renderer = renderer
        self.sttRoute = sttRoute; self.ttsRoute = ttsRoute
    }
    /// Install only after the SDK constructor's synchronous safety stop. Never
    /// expose persisted records belonging to a previous observation epoch.
    func configureHistory(store: (any SurfaceLedgerStore)?) throws {
        guard history.isAvailable else { throw NativeSurfaceError("unavailable") }
        let configured = try FaceHeardHistory(store: store)
        try configured.revokeAll()
        history = configured
    }
    /// Trusted host calls after opening an SDK native observation session. The
    /// emitter closes over that immutable token, never the newest session token.
    public func activate(owner: String, epoch: UInt64, remoteAudioConsent: Bool = false, emit: @escaping (String, CBORValue) throws -> Void) {
        stop(owner: nil); self.owner = owner; self.epoch = epoch; self.emit = emit
        self.remoteAudioConsent = remoteAudioConsent
    }
    func retainClosedStreams(from actions: [CBORValue]) {
        for action in actions where action["capability"] == .text("speech.speak_chunk") || action["capability"] == .text("speech.backchannel") {
            if let stream = action["args"]?["stream"]?.faceText { closedStreams.insert(stream) }
        }
    }
    func startDuplex() throws {
        guard history.isAvailable else { throw NativeSurfaceError("unavailable") }
        guard owner != nil else { throw NativeSurfaceError("permission_denied") }
        guard !sttRoute.isEmpty, !ttsRoute.isEmpty, synthesis.route == ttsRoute else { throw NativeSurfaceError("unavailable") }
        guard !synthesis.usesRemoteAudio || remoteAudioConsent else { throw NativeSurfaceError("permission_denied") }
        _ = try read(operation: "speech.config", args: .map([]), owner: owner!, now: 0)
        try audio.start(); graph = audio.generation
    }
    public func beginCapture(recognition: any FaceRecognition, utterance: String, positionMS: UInt64, now: UInt64? = nil) throws {
        guard owner != nil else { throw NativeSurfaceError("permission_denied") }
        guard recognition.route == sttRoute else { throw NativeSurfaceError("unavailable") }
        guard !recognition.usesRemoteAudio || remoteAudioConsent else { throw NativeSurfaceError("permission_denied") }
        try CBORCodec.validate("common::correlation-id", .text(utterance))
        try closeCapture()
        captureGeneration = UUID(); let custody = captureGeneration
        captureNormalizer = SpeechObservations()
        try captureNormalizer.start(utterance: utterance)
        try speechStart(utterance: utterance, positionMS: positionMS)
        self.utterance = utterance; captureRevision = 0; captureStartMS = positionMS
        let started = now ?? lastTick
        guard started <= UInt64.max - 60_000_000 else { throw NativeSurfaceError("resource_exhausted") }
        captureDeadline = started + 60_000_000
        let emitter = emit // immutable owner/session closure, not a future binding
        captureOpen = true
        do {
            let input = try recognition.start(utterance: utterance, receive: { [weak self] update in
                guard let self else { return }
                guard self.captureGeneration == custody, self.owner != nil, self.utterance == utterance else {
                    self.discardedRecognitionUpdates = self.discardedRecognitionUpdates == .max ? .max : self.discardedRecognitionUpdates + 1
                    return
                }
                // Native callbacks can run ahead of the timer after a stalled
                // MainActor. Recheck the capture/finalization cutoff here.
                if let deadline = self.captureDeadline,
                   max(self.lastTick, self.captureClock?() ?? self.lastTick) >= deadline {
                    try self.closeCapture(); return
                }
                // Reserve one revision for an empty final on local failure.
                guard update.revision < UInt64.max, !update.final || update.positionMS >= self.captureStartMS else {
                    try self.closeCapture(); return
                }
                guard try self.captureNormalizer.transcript(utterance: utterance, revision: update.revision,
                    text: update.text, final: update.final) else {
                    self.discardedRecognitionUpdates = self.discardedRecognitionUpdates == .max ? .max : self.discardedRecognitionUpdates + 1
                    return
                }
                self.captureRevision = update.revision
                var fields: [(String, CBORValue)] = [("utterance", .text(utterance)),
                    ("revision", .unsigned(update.revision)), ("text", .text(update.text))]
                if update.final {
                    fields.append(("position_ms", .unsigned(update.positionMS)))
                    self.retireCapture()
                }
                try emitter?(update.final ? "speech.end" : "speech.partial", .record(fields))
            }, failed: { [weak self] in
                guard let self, self.captureGeneration == custody, self.utterance == utterance else { return }
                try self.closeCapture()
            })
            // Synchronous final/failure may retire before start returns its handle.
            if captureGeneration == custody, self.utterance == utterance { recognitionInput = input }
            else { input.cancel() }
        } catch {
            try closeCapture()
            throw error
        }
    }
    private func retireCapture() {
        captureGeneration = UUID(); captureOpen = false; utterance = nil; captureDeadline = nil
        let old = recognitionInput; recognitionInput = nil; old?.cancel()
    }
    /// Failure/replacement closes with an empty transcript, never a guessed final
    /// from the last partial. Fence before invoking potentially synchronous cancel.
    private func closeCapture() throws {
        guard let utterance else { return }
        let revision = captureRevision + 1, position = max(captureStartMS, capturePositionMS), emitter = emit
        recognitionFailures = recognitionFailures == .max ? .max : recognitionFailures + 1
        retireCapture()
        try emitter?("speech.end", .record([("utterance", .text(utterance)), ("revision", .unsigned(revision)),
            ("text", .text("")), ("position_ms", .unsigned(position))]))
    }
    func setVoiceActivityHook(_ hook: ((Bool) -> Void)?) { audio.onVoiceActivity = hook }
    public func finishCapture(now: UInt64? = nil) {
        guard captureOpen else { return }
        captureOpen = false
        let time = now ?? lastTick
        captureDeadline = min(captureDeadline ?? .max, time > UInt64.max - 5_000_000 ? .max : time + 5_000_000)
        recognitionInput?.finish()
    }
    public func pollCapture() throws {
        let samples = audio.readCapture()
        guard capturedFrames <= UInt64.max - UInt64(samples.count) else { throw NativeSurfaceError("resource_exhausted") }
        capturedFrames += UInt64(samples.count)
        if captureOpen, !samples.isEmpty {
            do { try recognitionInput?.append(samples) }
            catch { try closeCapture(); throw error }
        }
    }
    public func takeCompletions() -> [FaceCompletion] {
        let result = completions; completions = []; return result
    }
    public func preflight(action: CBORValue, now: UInt64) throws {
        guard history.isAvailable else { throw NativeSurfaceError("unavailable") }
        guard action["coglet"] == owner.map(CBORValue.text), let operation = action["capability"]?.faceText,
              let args = action["args"] else { throw NativeSurfaceError("permission_denied") }
        switch operation {
        case "expression.set_expression":
            try CBORCodec.validate("expression::set-expression", args)
            guard let duration = args["duration_ms"]?.faceUInt, now <= UInt64.max - duration * 1000 else {
                throw NativeSurfaceError("invalid_argument")
            }
        case "speech.speak_chunk":
            try validateSynthesisRoute()
            try CBORCodec.validate("speech::speak-chunk", args)
            guard let text = args["text"]?.faceText, text.utf8.count <= 4096 else { throw NativeSurfaceError("invalid_argument") }
            let id = args["stream"]!.faceText!, chunk = args["chunk"]!.faceUInt!
            guard !closedStreams.contains(id), (stream == nil && chunk == 0 || stream == id && !inputClosed && chunk == nextChunk),
                  chunk < 64 else { throw NativeSurfaceError("conflict") }
            guard closedStreams.count < 1024 else { throw NativeSurfaceError("resource_exhausted") }
        case "speech.backchannel":
            try validateSynthesisRoute()
            try CBORCodec.validate("speech::backchannel", args)
            guard stream == nil, !closedStreams.contains(args["stream"]!.faceText!) else { throw NativeSurfaceError("conflict") }
            guard closedStreams.count < 1024 else { throw NativeSurfaceError("resource_exhausted") }
        case "speech.stop":
            try CBORCodec.validate("speech::stop", args)
            let id = args["stream"]!.faceText!
            guard stream == id || closedStreams.contains(id) else { throw NativeSurfaceError("not_found") }
        default: throw NativeSurfaceError("unavailable")
        }
    }
    public func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        try preflight(action: action, now: now)
        guard epoch == self.epoch, let id = action["id"]?.faceText, let deadline = action["deadline"]?.faceUInt,
              deadline > now else { throw NativeSurfaceError("deadline_exceeded") }
        let args = action["args"]!
        let custody = generation, custodyOwner = owner!
        if action["capability"] == .text("expression.set_expression") {
            try renderer.set(expression: args["expression"]!.faceText!, intensity: args["intensity"]!.faceUInt!,
                             durationMS: args["duration_ms"]!.faceUInt!, now: now)
            return .succeeded(.map([]))
        }
        let streamID = args["stream"]!.faceText!
        if action["capability"] == .text("speech.stop") {
            if stream == streamID {
                var snapshot = items.first
                if let base = snapshot?.base, let data = snapshot?.audio, audio.renderedFrames >= base {
                    snapshot?.playedMS = min(UInt64(data.samples.count), audio.renderedFrames - base) / 24
                }
                failStream(code: "cancelled")
                guard history.isAvailable else { throw NativeSurfaceError("unavailable") }
                if let snapshot, snapshot.base != nil { try report(item: snapshot, state: "stopped") }
            }
            return .succeeded(.map([]))
        }
        let backchannel = action["capability"] == .text("speech.backchannel")
        let chunk = backchannel ? 0 : args["chunk"]!.faceUInt!
        let cues = ["ack": "mm-hm", "thinking": "working", "agree": "yes"]
        let text = backchannel ? cues[args["cue"]!.faceText!]! : args["text"]!.faceText!
        try history.register(item: id, owner: owner!, epoch: epoch)
        // Registration saved synchronously; resample before mutating or synthesizing.
        try reauthorize(generation: custody, owner: custodyOwner, epoch: epoch, deadline: deadline, fallbackNow: now)
        if stream == nil { stream = streamID; streamPosition = 0 }
        nextChunk = chunk + 1; inputClosed = backchannel || args["final"] == .boolean(true); idleSince = now
        items.append(Item(owner: owner!, epoch: epoch, id: id, stream: streamID, chunk: chunk,
                          text: text, deadline: deadline))
        if items.count == 1 { startSynthesis(recheck: false) }
        return nil
    }
    private func startSynthesis(recheck: Bool = true) {
        guard let item = items.first else { return }
        let token = generation, id = item.id
        if recheck {
            // Preceding playback/history saves may have blocked past expiry.
            do { try reauthorize(generation: token, owner: item.owner, epoch: item.epoch, deadline: item.deadline, fallbackNow: lastTick) }
            catch {
                if generation == token, items.first?.id == id {
                    failStream(code: (error as? NativeSurfaceError)?.code ?? "unavailable")
                }
                return
            }
            guard generation == token, items.first?.id == id else { return }
        }
        do { try validateSynthesisRoute() } catch { failStream(code: "unavailable"); return }
        let cancel = synthesis.synthesize(text: item.text) { [weak self] result in
            guard let self, self.generation == token, self.items.first?.id == id,
                  self.items.first?.audio == nil else { return }
            switch result {
            case .success(let value): self.items[0].audio = value
            case .failure: self.failStream(code: "unavailable")
            }
        }
        // A provider may fail synchronously before returning its handle. Never
        // install that obsolete handle over a newer request or after failure.
        if generation == token, items.first?.id == id { cancellation = cancel }
        else { cancel() }
    }
    public func tick(now: UInt64) throws {
        guard history.isAvailable else { throw NativeSurfaceError("unavailable") }
        lastTick = now
        renderer.tick(now: now)
        if let captureDeadline, now >= captureDeadline { try closeCapture() }
        if stream != nil, !inputClosed, now >= idleSince, now - idleSince >= 60_000_000 {
            failStream(code: "cancelled")
        }
        if let graph, (!audio.isRunning || audio.captureFailed || graph != audio.generation) {
            stop(owner: nil)
            throw NativeSurfaceError("unavailable")
        }
        guard !items.isEmpty else { return }
        if items.contains(where: { $0.deadline <= now }) { failStream(code: "deadline_exceeded"); return }
        guard let data = items[0].audio else { return }
        if items[0].base == nil {
            guard !data.samples.isEmpty, data.samples.count <= 24_000 * 30,
                  data.spans.allSatisfy({ $0.endMS <= UInt64(data.samples.count) / 24 }),
                  data.spans.map(\.text).joined().utf8.count <= items[0].text.utf8.count,
                  items[0].text.hasPrefix(data.spans.map(\.text).joined()) else { failStream(code: "invalid_argument"); return }
            let token = generation, id = items[0].id
            do {
                try history.append(item: items[0].id, owner: items[0].owner, epoch: items[0].epoch, spans: data.spans)
                // Alignment save may block: no audio start/enqueue without fresh authority.
                do { try reauthorize(generation: token, owner: items[0].owner, epoch: items[0].epoch, deadline: items[0].deadline, fallbackNow: now) }
                catch let denied as NativeSurfaceError {
                    if generation == token, items.first?.id == id { failStream(code: denied.code) }
                    return
                }
                guard generation == token, items.first?.id == id else { return }
                try audio.start(); graph = audio.generation
                items[0].base = audio.renderedFrames
                try audio.enqueue(data.samples)
            } catch {
                failStream(code: "unavailable")
                if !history.isAvailable { throw error }
                return
            }
        }
        let item = items[0], base = item.base!
        guard audio.renderedFrames >= base else { failStream(code: "unavailable"); return }
        let frames = min(UInt64(data.samples.count), audio.renderedFrames - base)
        let played = frames / 24
        try history.playback(item: item.id, owner: item.owner, epoch: item.epoch, playedMS: played, stopped: frames == UInt64(data.samples.count))
        items[0].playedMS = played
        try report(item: items[0], state: frames == UInt64(data.samples.count) ? "finished" : "playing")
        if frames == UInt64(data.samples.count) {
            completions.append(.init(owner: item.owner, id: item.id, epoch: item.epoch, result: .succeeded(.map([]))))
            streamPosition += played; items.removeFirst(); cancellation = nil
            if !items.isEmpty { startSynthesis() }
            else if inputClosed { closedStreams.insert(item.stream); stream = nil }
        }
    }
    private func report(item: Item, state: String) throws {
        try emit?("speech.position", .record([("stream", .text(item.stream)), ("chunk", .unsigned(item.chunk)),
            ("item", .text(item.id)), ("played_ms", .unsigned(item.playedMS)),
            ("position_ms", .unsigned(streamPosition + item.playedMS)), ("state", .text(state))]))
    }
    private func validateSynthesisRoute() throws {
        guard !ttsRoute.isEmpty, synthesis.route == ttsRoute else { throw NativeSurfaceError("unavailable") }
        guard !synthesis.usesRemoteAudio || remoteAudioConsent else { throw NativeSurfaceError("permission_denied") }
    }
    private func failStream(code: String) {
        // Snapshot before flush and before a provider's synchronous cancel hook.
        if !items.isEmpty, let base = items[0].base, let data = items[0].audio, audio.renderedFrames >= base {
            items[0].playedMS = min(UInt64(data.samples.count), audio.renderedFrames - base) / 24
        }
        generation = UUID(); let cancel = cancellation; cancellation = nil
        audio.flush(); cancel?()
        for (index, item) in items.enumerated() {
            do {
                try history.playback(item: item.id, owner: item.owner, epoch: item.epoch, playedMS: item.playedMS, stopped: true)
            } catch {
                // A native counter/projection violation must not leave readable
                // stale evidence. Purge rather than infer a new heard prefix.
                try? history.revokeAll()
            }
            completions.append(.init(owner: item.owner, id: item.id, epoch: item.epoch,
                                     result: index == 0 && code != "cancelled" ? .failed(.init(code)) : .cancelled))
        }
        items = []; if let stream { closedStreams.insert(stream) }; stream = nil
    }
    /// Native reflex is applied before publishing any wire observation. Called
    /// from trusted capture/VAD only; no runtime action or guest step is awaited.
    public func speechStart(utterance: String, positionMS: UInt64) throws {
        try CBORCodec.validate("common::correlation-id", .text(utterance))
        guard owner != nil else { throw NativeSurfaceError("permission_denied") }
        var interrupted: Item?
        if !items.isEmpty {
            if let base = items[0].base, let data = items[0].audio {
                guard audio.renderedFrames >= base else { throw NativeSurfaceError("unavailable") }
                items[0].playedMS = min(UInt64(data.samples.count), audio.renderedFrames - base) / 24
                interrupted = items[0]
            }
            // Invalidate callbacks BEFORE a synchronous cancellation hook.
            generation = UUID(); let cancel = cancellation; cancellation = nil
            audio.flush(); cancel?()
            for item in items {
                try history.playback(item: item.id, owner: item.owner, epoch: item.epoch, playedMS: item.playedMS, stopped: true)
                completions.append(.init(owner: item.owner, id: item.id, epoch: item.epoch, result: .cancelled))
            }
            if let stream { closedStreams.insert(stream) }; stream = nil; items = []
        }
        try emit?("speech.start", .record([("utterance", .text(utterance)), ("position_ms", .unsigned(positionMS))]))
        if let item = interrupted {
            try emit?("speech.barge_in", .record([("utterance", .text(utterance)), ("stream", .text(item.stream)),
                ("item", .text(item.id)), ("played_ms", .unsigned(item.playedMS)),
                ("position_ms", .unsigned(streamPosition + item.playedMS)), ("reflex", .text("stopped"))]))
            try report(item: item, state: "stopped")
        }
    }
    public func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        guard owner == self.owner else { throw NativeSurfaceError("permission_denied") }
        if operation == "speech.config" {
            try CBORCodec.validate("surface::empty-map", args)
            let config = CBORValue.record([
                ("stt", .record([("placement", .text("surface")), ("route", .text(sttRoute))])),
                ("tts", .record([("placement", .text("surface")), ("route", .text(ttsRoute))])),
                ("barge", .text("stop")), ("session_mode", .text("text"))])
            try CBORCodec.validate("speech::speech-config", config)
            return config
        }
        if operation == "expression.state" {
            try CBORCodec.validate("surface::empty-map", args)
            let value = renderer.snapshot(now: now)
            return .record([("expression", .text(value.expression)), ("intensity", .unsigned(value.intensity)),
                            ("remaining_ms", .unsigned(value.remainingMS))])
        }
        if operation == "speech.heard" {
            try CBORCodec.validate("speech::heard-query", args)
            return try history.heard(item: args["item"]!.faceText!, owner: owner, epoch: epoch)
        }
        throw NativeSurfaceError("unavailable")
    }
    public func stop(owner: String?) {
        guard owner == nil || owner == self.owner else { return }
        // Native safety must precede even a blocking or failing history save.
        audio.stop(); renderer.reset()
        failStream(code: "cancelled")
        self.owner = nil; emit = nil
        remoteAudioConsent = false
        retireCapture() // privacy revocation suppresses terminal transcript delivery
        nextChunk = 0; inputClosed = false; graph = nil
        try? history.revokeAll()
    }
}

extension CBORValue {
    var faceText: String? { if case .text(let value) = self { return value }; return nil }
    var faceUInt: UInt64? { if case .unsigned(let value) = self { return value }; return nil }
}
