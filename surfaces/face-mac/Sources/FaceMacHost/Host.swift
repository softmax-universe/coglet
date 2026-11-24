import Foundation
import FaceMac
import CogletSurface
import CogletTypes

// Explicit simulator executable: real Face adapter/session, injected media I/O.
// The command protocol is test-only local stdio, NOT a new wire capability.
private final class MemoryLedger: SurfaceLedgerStore {
    private var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ value: [UInt8]) throws { bytes = value }
}
@MainActor private final class Media: FaceDuplexIO {
    var renderedFrames: UInt64 = 0
    var isRunning = false
    var generation: UInt64 = 0
    var captureFailed = false
    var onVoiceActivity: ((Bool) -> Void)?
    var queued: UInt64 = 0
    func start() throws { isRunning = true }
    func stop() { isRunning = false; queued = 0 }
    func enqueue(_ samples: [Int16]) throws { queued += UInt64(samples.count) }
    func flush() { queued = 0 }
    func readCapture() -> [Int16] { [] }
    func render(_ frames: UInt64) { let n = min(queued, frames); renderedFrames += n; queued -= n }
}
@MainActor private final class Synthesis: FaceSynthesis {
    let route = "simulator"
    let usesRemoteAudio = false
    var duration: UInt64 = 3000
    var spans: [SpeechObservations.Span] = []
    func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, Swift.Error>) -> Void) -> @MainActor () -> Void {
        completion(.success(.init(samples: [Int16](repeating: 1, count: Int(duration) * 24), spans: spans)))
        return {}
    }
}
@MainActor private final class Recognition: FaceRecognition, FaceRecognitionInput {
    let route = "simulator"
    let usesRemoteAudio = false
    var receive: (@MainActor (FaceRecognitionUpdate) throws -> Void)?
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
               failed: @escaping @MainActor () throws -> Void) throws -> any FaceRecognitionInput {
        self.receive = receive; return self
    }
    func append(_ samples: [Int16]) throws {}
    func finish() {}
    // Retain the callback deliberately: the adapter, not a cooperative fake,
    // must discard late recognition after cancellation/finalization.
    func cancel() {}
}
@main struct FaceMacHost {
    @MainActor static func main() throws {
        guard CommandLine.arguments == [CommandLine.arguments[0], "--simulator"] else {
            FileHandle.standardError.write(Data("usage: face-mac --simulator\nNo native route is configured by this test driver.\n".utf8))
            return
        }
        let media = Media(), synthesis = Synthesis(), recognition = Recognition()
        var now: UInt64 = 0
        let owner = "coglet:" + String(repeating: "b", count: 64)
        let backend = FaceSurfaceBackend(audio: media, synthesis: synthesis, renderer: .init { _ in }, sttRoute: "simulator", ttsRoute: "simulator")
        let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "a", count: 64), owner: owner,
            store: MemoryLedger(), backend: backend,
            grants: [.init(coglet: owner, operations: Set(FaceSurfaceHost.operations.map(\.name)),
                resources: ["audio.mic", "audio.speaker", "display.face"], expires: 1_000_000_000)], clock: { now })
        try host.start()
        defer { host.disconnect() }
        var active = false
        while let line = readLine() {
            guard line.utf8.count <= 2_000_000,
                  let command = try JSONSerialization.jsonObject(with: Data(line.utf8)) as? [String: Any]
            else { throw NativeSurfaceError("invalid_argument") }
            if let time = command["now"] as? UInt64 { guard time >= now else { throw NativeSurfaceError("invalid_argument") }; now = time }
            if let setup = command["synthesis"] as? [String: Any] {
                guard let duration = setup["duration_ms"] as? UInt64, duration > 0, duration <= 30_000 else { throw NativeSurfaceError("invalid_argument") }
                synthesis.duration = duration
                synthesis.spans = try (setup["spans"] as? [[String: Any]] ?? []).map {
                    guard let end = $0["end_ms"] as? UInt64, let text = $0["text"] as? String else { throw NativeSurfaceError("invalid_argument") }
                    return .init(endMS: end, text: text)
                }
            }
            if let wire = command["wire"] as? [UInt8] { try host.receive(wire) }
            if command["activate"] as? Bool == true { try host.activateDuplex(localConsent: true); active = true }
            if let frames = command["render"] as? UInt64 { media.render(frames) }
            if command["start"] as? Bool == true { guard active else { throw NativeSurfaceError("permission_denied") }; _ = try host.beginCapture(recognition: recognition, positionMS: command["position_ms"] as? UInt64 ?? 0) }
            if let update = command["recognition"] as? [String: Any] {
                guard let revision = update["revision"] as? UInt64, let text = update["text"] as? String, let final = update["final"] as? Bool else { throw NativeSurfaceError("invalid_argument") }
                try recognition.receive?(.init(revision: revision, text: text, final: final, positionMS: update["position_ms"] as? UInt64 ?? 0))
            }
            try host.pump()
            let response: [String: Any] = ["frames": host.drain(), "discarded": backend.discardedRecognitionUpdates,
                "rendered": media.renderedFrames, "queued": media.queued]
            let data = try JSONSerialization.data(withJSONObject: response, options: [.sortedKeys])
            FileHandle.standardOutput.write(data + Data([10]))
        }
        try host.close()
    }
}
