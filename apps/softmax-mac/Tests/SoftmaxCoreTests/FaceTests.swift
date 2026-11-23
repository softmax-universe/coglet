import XCTest
import FaceMac
@testable import SoftmaxCore

final class FaceTests: XCTestCase, @unchecked Sendable {
    @MainActor
    func testBargeInSilencesSinkWithin100MSAndFencesLateFrames() async throws {
        let device = FakeFacePlayback()
        let sink = FaceSpeechSink(device: device)
        let token = sink.begin()
        try sink.enqueue(PCMFrame(samples: [0.5, -0.5], sampleRate: 16000, timestamp: 0), token: token)
        device.renderedFrames = 1
        sink.tick()
        XCTAssertEqual(sink.level, 0.5)
        let clock = TestClock()
        device.onStop = { clock.time += 0.004 }
        let keys = FaceTestKeys(); let audio = FaceTestAudio()
        let ptt = PushToTalk(keys: keys, audio: audio, now: { clock.time })
        ptt.permissions = .init(microphone: .granted, inputMonitoring: .granted)
        ptt.start(); ptt.setState(.speaking)
        ptt.stopPlayback = { sink.stop() }
        let before = clock.time
        keys.send(.press)
        XCTAssertLessThanOrEqual(clock.time - before, 0.100)
        XCTAssertFalse(device.playing)
        XCTAssertEqual(sink.level, 0)
        XCTAssertThrowsError(try sink.enqueue(PCMFrame(samples: [1], sampleRate: 16000, timestamp: 0), token: token))
        device.completions.first?()
        XCTAssertEqual(ptt.state, .listening)
        XCTAssertFalse(sink.active)
        ptt.shutdown()
    }

    @MainActor
    func testExpressionsExpireResetAndRejectInvalidArguments() async throws {
        var time = 10.0
        let driver = LocalFaceDriver(now: { time })
        for expression in FaceExpression.allCases {
            try driver.setExpression(expression.rawValue, intensity: 1000, durationMS: 100)
            XCTAssertEqual(driver.expression, expression)
            let p = driver.parameters(reduceMotion: false)
            switch expression {
            case .neutral: XCTAssertEqual(p, FaceParameters(state: .idle))
            case .listening: XCTAssertEqual(p.headScale, 1.025)
            case .thinking: XCTAssertEqual(p.eyeHeight, 26)
            case .speaking: XCTAssertEqual(p.mouthHeight, 40)
            case .happy: XCTAssertEqual(p.happy, 1)
            case .concerned: XCTAssertEqual(p.concerned, 1)
            }
        }
        time = 10.05
        try driver.setExpression("happy", intensity: 500, durationMS: 200)
        time = 10.11; driver.tick()
        XCTAssertEqual(driver.expression, .happy, "old expiry cannot erase replacement")
        XCTAssertEqual(driver.parameters(reduceMotion: false).happy, 0.5)
        time = 10.26; driver.tick()
        XCTAssertEqual(driver.expression, .neutral)
        XCTAssertThrowsError(try driver.setExpression("unknown", intensity: 1000, durationMS: 1))
        XCTAssertThrowsError(try driver.setExpression("happy", intensity: -1, durationMS: 1))
        XCTAssertThrowsError(try driver.setExpression("happy", intensity: 1, durationMS: 60001))
        try driver.setExpression("happy", intensity: 999, durationMS: 0)
        XCTAssertEqual(driver.expression, .neutral)
        driver.setState(.listening(level: 0.8))
        XCTAssertEqual(driver.parameters(reduceMotion: true).headScale, 1)
        driver.reset()
        XCTAssertEqual(driver.state, .idle)
    }

    func testStatesMapToDesignParametersAndClampLevels() {
        XCTAssertEqual(FaceParameters(state: .idle).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .listening(level: 2)).headScale, 1.025)
        XCTAssertEqual(FaceParameters(state: .thinking).eyeHeight, 26)
        XCTAssertEqual(FaceParameters(state: .speaking(level: 1)).mouthHeight, 88)
        XCTAssertEqual(FaceParameters(state: .speaking(level: -.infinity)).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .speaking(level: .nan)).mouthHeight, 16)
        XCTAssertEqual(FaceParameters(state: .listening(level: 1), reduceMotion: true).headScale, 1)
    }
}
