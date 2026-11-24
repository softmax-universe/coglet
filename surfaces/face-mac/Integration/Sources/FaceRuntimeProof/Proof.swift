import Foundation
import FaceMac
import CogletSurface
import CRuntime

// These wrappers call the actual linked symbols; they neither emulate the peer
// nor replace scheduled guest execution. Every boundary asserts off-main entry.
private func offMain() { precondition(!Thread.isMainThread, "ABI on main thread") }
private func configure(_ p: UnsafePointer<UInt8>?, _ n: Int) -> Int32 { offMain(); return coglet_runtime_configure(p,n) }
private func start(_ p: UnsafePointer<UInt8>?, _ n: Int) -> Int32 { offMain(); return coglet_runtime_start(p,n) }
private func stop() -> Int32 { offMain(); return coglet_runtime_stop() }
private func status() -> Int32 { offMain(); return coglet_runtime_status() }
private func open(_ v: UInt32, _ p: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 { offMain(); return coglet_surface_open(v,p) }
private func send(_ p: OpaquePointer?, _ b: UnsafePointer<UInt8>?, _ n: Int) -> Int32 { offMain(); return coglet_surface_send(p,b,n) }
private func recv(_ p: OpaquePointer?, _ b: UnsafeMutablePointer<UInt8>?, _ n: Int, _ size: UnsafeMutablePointer<Int>?) -> Int32 { offMain(); return coglet_surface_recv(p,b,n,size) }
private func close(_ p: OpaquePointer?) { offMain(); coglet_surface_close(p) }

@MainActor private final class Media: FaceDuplexIO {
    var renderedFrames: UInt64 = 0
    var isRunning = false
    var generation: UInt64 = 0
    var captureFailed = false
    var onVoiceActivity: ((Bool) -> Void)?
    var enqueued = 0
    func start() throws { isRunning = true }
    func stop() { isRunning = false; generation += 1 }
    func enqueue(_ samples: [Int16]) throws { enqueued += samples.count }
    func flush() {}
    func readCapture() -> [Int16] { [] }
}
@MainActor private final class Voice: FaceSynthesis {
    let route = "local"
    let usesRemoteAudio = false
    func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, any Swift.Error>) -> Void) -> @MainActor () -> Void {
        precondition(text == "Hello world")
        completion(.success(.init(samples: Array(repeating: 1, count: 72_000),
            spans: [.init(endMS: 1200, text: "Hello"), .init(endMS: 3000, text: " world")])))
        return {}
    }
}
@MainActor private final class Input: FaceRecognitionInput {
    func append(_ samples: [Int16]) throws {}
    func finish() {}
    func cancel() {}
}
@MainActor private final class Recognition: FaceRecognition {
    let route = "local"
    let usesRemoteAudio = false
    var callback: (@MainActor (FaceRecognitionUpdate) throws -> Void)?
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
               failed: @escaping @MainActor () throws -> Void) throws -> any FaceRecognitionInput {
        callback = receive; return Input()
    }
}
private enum ProofFailure: Swift.Error { case timeout, failed }
@main struct Proof {
    @MainActor static func main() async throws {
        guard CommandLine.arguments.count == 2 else { throw ProofFailure.failed }
        let directory = URL(fileURLWithPath: CommandLine.arguments[1])
        let abi = FaceRuntimeABI(surface: .init(open: open, send: send, recv: recv, close: close),
            configure: configure, start: start, stop: stop, status: status)
        let link = try await FaceEmbeddedRuntimeLink.start(abi: abi,
            binding: Array(Data(contentsOf: directory.appendingPathComponent("binding.json"))),
            configuration: Array(Data(contentsOf: directory.appendingPathComponent("runtime.json"))))
        let io = Media(), recognition = Recognition()
        let clock: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds / 1000 }
        let owner = "coglet:" + String(repeating: "c", count: 64)
        let ops = Set(FaceSurfaceHost.operations.map(\.name)).union(["speech.heard"])
        let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "b", count: 64), owner: owner,
            store: FileSurfaceLedger(directory: directory.appendingPathComponent("adapter"), newIdentity: true),
            backend: FaceSurfaceBackend(audio: io, synthesis: Voice(), renderer: .init { _ in }),
            grants: [.init(coglet: owner, operations: ops, resources: ["audio.mic", "audio.speaker", "display.face"], expires: clock() + 60_000_000)],
            clock: clock, historyStore: FileSurfaceLedger(directory: directory.appendingPathComponent("history"), newIdentity: true))
        try host.attach(link: link); try host.start()
        // Give hello/caps a bounded asynchronous drain before activating custody.
        for _ in 0..<20 { try await Task.sleep(for: .milliseconds(10)); try host.pump() }
        let readyDeadline = ContinuousClock.now + .seconds(5)
        while try !link.flush() {
            guard ContinuousClock.now < readyDeadline else { throw ProofFailure.timeout }
            try await Task.sleep(for: .milliseconds(5)); try host.pump()
        }
        try host.activateDuplex(localConsent: true)
        _ = try host.beginCapture(recognition: recognition, positionMS: 0)
        for _ in 0..<20 { try await Task.sleep(for: .milliseconds(10)); try host.pump() }
        try recognition.callback?(.init(revision: 1, text: "Hello world", final: true, positionMS: 10))
        let deadline = ContinuousClock.now + .seconds(5)
        while io.enqueued == 0 {
            guard ContinuousClock.now < deadline else { throw ProofFailure.timeout }
            try await Task.sleep(for: .milliseconds(5)); try host.pump()
        }
        io.renderedFrames = 28_800
        _ = try host.beginCapture(recognition: recognition, positionMS: 1200)
        for _ in 0..<300 { try await Task.sleep(for: .milliseconds(5)); try host.pump() }
        guard host.lastFailure == nil else { throw ProofFailure.failed }
        try host.localOffSwitch()
        precondition(!io.isRunning)
        try await link.shutdown()
        try host.close()
        print("actual C ABI Face guest probe: off-main symbols, speech playback, 1200ms barge and native stop completed")
    }
}
