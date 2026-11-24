import XCTest
import FaceMac
@testable import SoftmaxCore

@MainActor final class BridgeTestSynthesis: LocalSpeechSynthesis {
    var completions: [@MainActor (Result<[PCMFrame], Error>) -> Void] = []
    var cancels = 0
    func synthesize(_ text: String, completion: @escaping @MainActor (Result<[PCMFrame], Error>) -> Void) -> (@MainActor () -> Void) {
        completions.append(completion)
        return { self.cancels += 1 }
    }
}
@MainActor final class BridgeTestDevice: FacePlaybackDevice {
    var renderedFrames: Int64 = 0
    var callbacks: [@MainActor () -> Void] = []
    var scheduled = 0
    var stopped = 0
    func schedule(_ frame: PCMFrame, completion: @escaping @MainActor () -> Void) throws {
        scheduled += 1; callbacks.append(completion)
    }
    func stop() { stopped += 1; renderedFrames = 0 }
}
@MainActor private final class BridgeKeys: PushToTalkKeySource {
    var onEvent: ((PushToTalkKeyEvent) -> Void)?
    func start() -> Bool { true }
    func stop() {}
}
@MainActor private final class BridgeCapture: AudioCapture {
    let frames = AsyncStream<PCMFrame> { $0.finish() }
    var inputLevel: Float { 0 }
    func start() throws {}
    func stop() -> [PCMFrame] { [] }
}
final class RuntimeSpeechPlaybackTests: XCTestCase {
    @MainActor func testExpiredSynthesisCannotStartNativePlayback() throws {
        let ptt = PushToTalk(keys: BridgeKeys(), audio: BridgeCapture())
        let synth = BridgeTestSynthesis(); let device = BridgeTestDevice()
        var clock: UInt64 = 10
        let player = RuntimeSpeechPlayback(session: FacePlaybackSession(ptt: ptt, speech: FaceSpeechSink(device: device)), synthesis: synth, now: { clock })
        var result: String?
        player.enqueue(text: "late", stream: "late", chunk: 0, final: true, deadline: 20) { result = $0 }
        clock = 20
        synth.completions[0](.success([PCMFrame(samples: [0.1], sampleRate: 16000, timestamp: 0)]))
        XCTAssertEqual(result, "deadline_exceeded")
        XCTAssertEqual(device.scheduled, 0)
    }
    @MainActor func testSynthesisIsNotSuccessAndBargeFencesOldNativeCompletion() throws {
        let keys = BridgeKeys(); let ptt = PushToTalk(keys: keys, audio: BridgeCapture())
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted); ptt.start()
        let device = BridgeTestDevice(); let synth = BridgeTestSynthesis()
        let sink = FaceSpeechSink(device: device)
        let player = RuntimeSpeechPlayback(session: FacePlaybackSession(ptt: ptt, speech: sink), synthesis: synth)
        ptt.stopPlayback = { player.stop() }
        var outcomes: [String?] = []
        let frame = PCMFrame(samples: [0.1], sampleRate: 16000, timestamp: 0)
        player.enqueue(text: "hello", stream: "one", chunk: 0, final: true) { outcomes.append($0) }
        XCTAssertEqual(ptt.state, .speaking)
        synth.completions[0](.success([frame]))
        XCTAssertEqual(device.scheduled, 1)
        XCTAssertTrue(outcomes.isEmpty, "synthesis/scheduling is not playback")
        let old = device.callbacks[0]
        keys.onEvent?(.press)
        XCTAssertEqual(outcomes, ["cancelled"])
        XCTAssertEqual(ptt.state, .listening)
        XCTAssertFalse(sink.active)
        old(); synth.completions[0](.success([frame]))
        XCTAssertEqual(device.scheduled, 1)
        XCTAssertEqual(outcomes.count, 1)
        ptt.cancel()
        player.enqueue(text: "new", stream: "two", chunk: 0, final: true) { outcomes.append($0) }
        synth.completions[1](.success([frame]))
        old()
        XCTAssertEqual(ptt.state, .speaking)
        device.renderedFrames = 1; device.callbacks[1]()
        XCTAssertEqual(outcomes.count, 2)
        XCTAssertNil(outcomes.last!)
        XCTAssertEqual(ptt.state, .idle)
        player.stop(); ptt.shutdown()
    }
    @MainActor func testStreamOrderingCapacityAndFailureCancelSuccessors() throws {
        let ptt = PushToTalk(keys: BridgeKeys(), audio: BridgeCapture())
        let synth = BridgeTestSynthesis(); let device = BridgeTestDevice()
        let player = RuntimeSpeechPlayback(session: FacePlaybackSession(ptt: ptt, speech: FaceSpeechSink(device: device)), synthesis: synth)
        var results: [String?] = []
        player.enqueue(text: "gap", stream: "a", chunk: 1, final: false) { results.append($0) }
        XCTAssertEqual(results, ["conflict"])
        results = []
        for chunk in 0..<16 { player.enqueue(text: "chunk", stream: "a", chunk: UInt64(chunk), final: false) { results.append($0) } }
        XCTAssertEqual(synth.completions.count, 1)
        player.enqueue(text: "full", stream: "a", chunk: 16, final: false) { results.append($0) }
        XCTAssertEqual(results, ["resource_exhausted"])
        synth.completions[0](.failure(LocalSpeechSynthesisError.unavailable))
        XCTAssertEqual(results.count, 17)
        XCTAssertEqual(results[1], "unavailable")
        XCTAssertEqual(results.dropFirst(2).compactMap { $0 }, Array(repeating: "cancelled", count: 15))
        player.enqueue(text: "reuse", stream: "a", chunk: 16, final: true) { results.append($0) }
        XCTAssertEqual(results.last!, "conflict")
    }
    @MainActor func testNativeQueueBackpressureRetainsUnscheduledPCM() async throws {
        let ptt = PushToTalk(keys: BridgeKeys(), audio: BridgeCapture())
        let synth = BridgeTestSynthesis(); let device = BridgeTestDevice()
        let player = RuntimeSpeechPlayback(session: FacePlaybackSession(ptt: ptt, speech: FaceSpeechSink(device: device)), synthesis: synth)
        let second = PCMFrame(samples: Array(repeating: 0.1, count: 8000), sampleRate: 8000, timestamp: 0)
        var finished = false
        player.enqueue(text: "long", stream: "a", chunk: 0, final: true) { _ in finished = true }
        synth.completions[0](.success(Array(repeating: second, count: 12)))
        XCTAssertEqual(device.scheduled, 10)
        XCTAssertFalse(finished)
        device.renderedFrames = 16000
        for _ in 0..<20 where device.scheduled < 12 { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(device.scheduled, 12)
        XCTAssertFalse(finished)
        player.stop()
        XCTAssertTrue(finished)
    }
}
