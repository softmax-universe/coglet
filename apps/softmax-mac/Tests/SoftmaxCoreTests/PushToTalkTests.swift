import XCTest
@testable import SoftmaxCore
import FaceMac

@MainActor
final class PushToTalkTests: XCTestCase, @unchecked Sendable {
    func testEmptyAudioIsDiscarded() async {
        let keys = FakeKeys(); let audio = FakeAudio(); audio.result = []
        var time = 10.0
        let ptt = PushToTalk(keys: keys, audio: audio, now: { time })
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted)
        ptt.start(); keys.send(.press); time = 11; keys.send(.release)
        XCTAssertEqual(ptt.state, .idle)
    }
    func testToggleIgnoresReleaseAndRepeat() async {
        let (ptt, keys, audio, clock) = fixture()
        ptt.mode = .toggle
        keys.send(.press); keys.send(.press)
        XCTAssertEqual(audio.starts, 1)
        clock.time = 11; keys.send(.release)
        XCTAssertEqual(ptt.state, .listening)
        keys.send(.press)
        XCTAssertEqual(ptt.state, .thinking)
        XCTAssertEqual(audio.stops, 1)
        ptt.shutdown()
    }
    func testShortPressAndBoundary() async {
        let (ptt, keys, audio, clock) = fixture()
        keys.send(.press); clock.time = 10.249; keys.send(.release)
        XCTAssertEqual(ptt.state, .idle)
        clock.time = 10; keys.send(.press); clock.time = 10.25; keys.send(.release)
        XCTAssertEqual(ptt.state, .thinking)
        XCTAssertEqual(audio.stops, 2)
    }
    func testMaxLengthCapAndLateRelease() async {
        let (ptt, keys, audio, clock) = fixture()
        var events = ptt.speechInput.makeAsyncIterator()
        audio.result = [PCMFrame(samples: [1, 2, 3, 4], sampleRate: 2, timestamp: 129)]
        keys.send(.press); clock.time = 135; ptt.tick()
        XCTAssertEqual(ptt.elapsed, 120)
        XCTAssertEqual(ptt.state, .thinking)
        guard case .utterance(let value) = await events.next() else { return XCTFail() }
        XCTAssertEqual(value.endedAt, 130)
        XCTAssertEqual(value.frames.first?.samples, [1, 2])
        keys.send(.press); keys.send(.release)
        XCTAssertEqual(audio.starts, 1)
        XCTAssertEqual(audio.stops, 1)
    }
    func testBargeInStopsPlaybackBeforeCapture() async {
        let (ptt, keys, _, _) = fixture()
        var stopped = false
        ptt.stopPlayback = { stopped = true }
        ptt.setState(.speaking)
        var events = ptt.speechInput.makeAsyncIterator()
        keys.send(.press)
        XCTAssertTrue(stopped)
        let event = await events.next()
        XCTAssertEqual(event, .bargeIn(timestamp: 10))
        XCTAssertEqual(ptt.state, .listening)
        ptt.shutdown()
    }
    func testDeniedPermissionsRouteWithoutCapture() async {
        let (ptt, keys, audio, _) = fixture()
        var routes = 0
        ptt.routeToPermissions = { routes += 1 }
        ptt.permissions = .init(microphone: .denied, inputMonitoring: .granted)
        keys.send(.press)
        XCTAssertEqual(routes, 1); XCTAssertEqual(audio.starts, 0)
        keys.send(.release)
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .notGranted)
        keys.send(.press)
        XCTAssertEqual(routes, 2); XCTAssertEqual(audio.starts, 0)
    }
    func testKeyTapDeniedAndRevocationCancel() async {
        let (ptt, keys, audio, _) = fixture()
        var routes = 0
        ptt.routeToPermissions = { routes += 1 }
        keys.send(.press); keys.send(.permissionDenied)
        XCTAssertEqual(routes, 1); XCTAssertEqual(audio.stops, 1)
        XCTAssertEqual(ptt.state, .idle)
        keys.send(.press); XCTAssertEqual(audio.starts, 1)
        keys.allowed = false; ptt.start()
        XCTAssertEqual(routes, 2)
    }
    func testPermissionLossAndShutdownDiscard() async {
        let (ptt, keys, audio, _) = fixture()
        keys.send(.press)
        ptt.permissions = .init(microphone: .denied, inputMonitoring: .granted)
        XCTAssertEqual(audio.stops, 1); XCTAssertEqual(ptt.state, .idle)
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted)
        keys.send(.press); ptt.shutdown(); keys.send(.release)
        XCTAssertEqual(audio.stops, 2); XCTAssertEqual(ptt.state, .idle)
    }
    func testCaptureFailureAndCancellation() async {
        let (ptt, keys, audio, _) = fixture()
        audio.fail = true; keys.send(.press)
        XCTAssertEqual(ptt.state, .error)
        keys.send(.release); audio.fail = false; keys.send(.press)
        keys.send(.cancelled)
        XCTAssertEqual(ptt.state, .idle); XCTAssertEqual(audio.stops, 2)
    }
    private func fixture() -> (PushToTalk, FakeKeys, FakeAudio, FakeClock) {
        let keys = FakeKeys(); let audio = FakeAudio(); let clock = FakeClock()
        let ptt = PushToTalk(keys: keys, audio: audio, now: { clock.time })
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted)
        ptt.start()
        return (ptt, keys, audio, clock)
    }
    func testHoldReleaseEmitsTimestampedPCM() async {
        let keys = FakeKeys()
        let audio = FakeAudio()
        var time = 10.0
        let ptt = PushToTalk(keys: keys, audio: audio, now: { time })
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted)
        ptt.start()
        var events = ptt.speechInput.makeAsyncIterator()
        keys.send(.press)
        XCTAssertEqual(ptt.state, .listening)
        time = 11
        keys.send(.release)
        XCTAssertEqual(ptt.state, .thinking)
        guard case .utterance(let utterance) = await events.next() else { return XCTFail("No utterance") }
        XCTAssertEqual(utterance.startedAt, 10)
        XCTAssertEqual(utterance.endedAt, 11)
        XCTAssertEqual(utterance.frames.first?.samples, [0.5, -0.5])
        XCTAssertEqual(audio.stops, 1)
    }
}

@MainActor private final class FakeClock { var time = 10.0 }

@MainActor
private final class FakeKeys: PushToTalkKeySource {
    var onEvent: ((PushToTalkKeyEvent) -> Void)?
    var allowed = true
    func start() -> Bool { allowed }
    func stop() {}
    func send(_ event: PushToTalkKeyEvent) { onEvent?(event) }
}

@MainActor
private final class FakeAudio: AudioCapture {
    let frames = AsyncStream<PCMFrame> { _ in }
    var inputLevel: Float { 0.5 }
    var stops = 0
    var starts = 0
    var fail = false
    var result = [PCMFrame(samples: [0.5, -0.5], sampleRate: 16000, timestamp: 10)]
    func start() throws { starts += 1; if fail { throw CocoaError(.fileReadUnknown) } }
    func stop() -> [PCMFrame] {
        stops += 1
        return result
    }
}
