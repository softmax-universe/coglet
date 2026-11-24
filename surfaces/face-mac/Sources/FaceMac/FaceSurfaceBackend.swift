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
    private var emit: ((String, CBORValue) throws -> Void)?
    private var history = SpeechObservations()
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
    private var captureNormalizer = SpeechObservations()
    public init(audio: any FaceDuplexIO, synthesis: any FaceSynthesis, renderer: FaceExpressionRenderer,
                sttRoute: String = "local", ttsRoute: String = "local") {
        self.audio = audio; self.synthesis = synthesis; self.renderer = renderer
        self.sttRoute = sttRoute; self.ttsRoute = ttsRoute
    }
    /// Trusted host calls after opening an SDK native observation session. The
    /// emitter closes over that immutable token, never the newest session token.
    public func activate(owner: String, epoch: UInt64, emit: @escaping (String, CBORValue) throws -> Void) {
        stop(owner: nil); self.owner = owner; self.epoch = epoch; self.emit = emit
    }
    func retainClosedStreams(from actions: [CBORValue]) {
        for action in actions where action["capability"] == .text("speech.speak_chunk") || action["capability"] == .text("speech.backchannel") {
            if let stream = action["args"]?["stream"]?.faceText { closedStreams.insert(stream) }
        }
    }
    func startDuplex() throws {
        guard owner != nil else { throw NativeSurfaceError("permission_denied") }
        try audio.start(); graph = audio.generation
    }
    public func beginCapture(recognition: any FaceRecognition, utterance: String, positionMS: UInt64) throws {
        guard owner != nil else { throw NativeSurfaceError("permission_denied") }
        captureGeneration = UUID(); let custody = captureGeneration
        let old = recognitionInput; recognitionInput = nil; captureOpen = false; old?.cancel()
        captureNormalizer = SpeechObservations()
        try captureNormalizer.start(utterance: utterance)
        try speechStart(utterance: utterance, positionMS: positionMS)
        let emitter = emit // immutable owner/session closure, not a future binding
        captureOpen = true
        let input = try recognition.start(utterance: utterance) { [weak self] update in
            guard let self, self.captureGeneration == custody, self.owner != nil else { return }
            guard try self.captureNormalizer.transcript(utterance: utterance, revision: update.revision,
                text: update.text, final: update.final) else { return }
            var fields: [(String, CBORValue)] = [("utterance", .text(utterance)),
                ("revision", .unsigned(update.revision)), ("text", .text(update.text))]
            if update.final { fields.append(("position_ms", .unsigned(update.positionMS))); self.captureOpen = false }
            try emitter?(update.final ? "speech.end" : "speech.partial", .record(fields))
        }
        if captureGeneration == custody { recognitionInput = input } else { input.cancel() }
    }
    public func finishCapture() { captureOpen = false; recognitionInput?.finish() }
    public func pollCapture() throws {
        let samples = audio.readCapture()
        if captureOpen, !samples.isEmpty { try recognitionInput?.append(samples) }
    }
    public func takeCompletions() -> [FaceCompletion] {
        let result = completions; completions = []; return result
    }
    public func preflight(action: CBORValue, now: UInt64) throws {
        guard action["coglet"] == owner.map(CBORValue.text), let operation = action["capability"]?.faceText,
              let args = action["args"] else { throw NativeSurfaceError("permission_denied") }
        switch operation {
        case "expression.set_expression":
            try CBORCodec.validate("expression::set-expression", args)
            guard let duration = args["duration_ms"]?.faceUInt, now <= UInt64.max - duration * 1000 else {
                throw NativeSurfaceError("invalid_argument")
            }
        case "speech.speak_chunk":
            try CBORCodec.validate("speech::speak-chunk", args)
            guard let text = args["text"]?.faceText, text.utf8.count <= 4096 else { throw NativeSurfaceError("invalid_argument") }
            let id = args["stream"]!.faceText!, chunk = args["chunk"]!.faceUInt!
            guard !closedStreams.contains(id), (stream == nil && chunk == 0 || stream == id && !inputClosed && chunk == nextChunk),
                  chunk < 64 else { throw NativeSurfaceError("conflict") }
            guard closedStreams.count < 1024 else { throw NativeSurfaceError("resource_exhausted") }
        case "speech.backchannel":
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
                if let snapshot, snapshot.base != nil { try report(item: snapshot, state: "stopped") }
            }
            return .succeeded(.map([]))
        }
        if stream == nil { stream = streamID; streamPosition = 0 }
        let backchannel = action["capability"] == .text("speech.backchannel")
        let chunk = backchannel ? 0 : args["chunk"]!.faceUInt!
        let cues = ["ack": "mm-hm", "thinking": "working", "agree": "yes"]
        let text = backchannel ? cues[args["cue"]!.faceText!]! : args["text"]!.faceText!
        nextChunk = chunk + 1; inputClosed = backchannel || args["final"] == .boolean(true); idleSince = now
        try history.register(item: id, owner: owner!, epoch: epoch)
        items.append(Item(owner: owner!, epoch: epoch, id: id, stream: streamID, chunk: chunk,
                          text: text, deadline: deadline))
        if items.count == 1 { startSynthesis() }
        return nil
    }
    private func startSynthesis() {
        guard let item = items.first else { return }
        let token = generation, id = item.id
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
        renderer.tick(now: now)
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
            do {
                try history.append(item: items[0].id, owner: items[0].owner, epoch: items[0].epoch, spans: data.spans)
                try audio.start(); graph = audio.generation
                items[0].base = audio.renderedFrames
                try audio.enqueue(data.samples)
            } catch { failStream(code: "unavailable"); return }
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
    private func failStream(code: String) {
        // Snapshot before flush and before a provider's synchronous cancel hook.
        if !items.isEmpty, let base = items[0].base, let data = items[0].audio, audio.renderedFrames >= base {
            items[0].playedMS = min(UInt64(data.samples.count), audio.renderedFrames - base) / 24
        }
        generation = UUID(); let cancel = cancellation; cancellation = nil
        audio.flush(); cancel?()
        for (index, item) in items.enumerated() {
            // All values were validated when admitted/aligned; a projection error
            // fails the host on its next read rather than inventing heard text.
            try? history.playback(item: item.id, owner: item.owner, epoch: item.epoch, playedMS: item.playedMS, stopped: true)
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
        if operation == "speech.heard", let item = args["item"]?.faceText {
            return try history.heard(item: item, owner: owner, epoch: epoch)
        }
        throw NativeSurfaceError("unavailable")
    }
    public func stop(owner: String?) {
        guard owner == nil || owner == self.owner else { return }
        failStream(code: "cancelled")
        captureGeneration = UUID(); captureOpen = false
        let input = recognitionInput; recognitionInput = nil
        self.owner = nil; emit = nil
        input?.cancel()
        nextChunk = 0; inputClosed = false; history = SpeechObservations(); graph = nil
        audio.stop(); renderer.reset()
    }
}

extension CBORValue {
    var faceText: String? { if case .text(let value) = self { return value }; return nil }
    var faceUInt: UInt64? { if case .unsigned(let value) = self { return value }; return nil }
}
