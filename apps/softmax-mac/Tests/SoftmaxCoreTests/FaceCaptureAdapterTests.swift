import XCTest
@testable import FaceMac

@MainActor
final class FaceCaptureAdapterTests: XCTestCase, @unchecked Sendable {
    func testAdapterDrainsPCMAndStopsKit() async {
        let source = FakePCMSource()
        let capture = EngineAudioCapture(source: source, automaticPolling: false, now: { 10 })
        try? capture.start()
        source.samples = [16384, -16384]
        capture.poll()
        XCTAssertEqual(capture.inputLevel, 0.5)
        source.samples = [8192]
        let frames = capture.stop()
        XCTAssertEqual(frames.count, 2)
        XCTAssertEqual(frames.first?.samples, [0.5, -0.5])
        XCTAssertEqual(frames.first?.sampleRate, 24000)
        XCTAssertEqual(frames[1].timestamp, 10 + 2.0 / 24000, accuracy: 0.000001)
        XCTAssertFalse(source.isRunning)
        XCTAssertEqual(capture.inputLevel, 0)
        XCTAssertTrue(capture.stop().isEmpty)
    }
    func testGraphChangeDiscardsInsteadOfSwitchingMicrophone() async {
        let source = FakePCMSource()
        let capture = EngineAudioCapture(source: source, automaticPolling: false)
        try? capture.start(); source.samples = [1, 2]; capture.poll()
        source.graphGeneration += 1
        capture.poll()
        XCTAssertTrue(capture.captureFailed)
        XCTAssertFalse(source.isRunning)
        XCTAssertTrue(capture.stop().isEmpty)
    }
    func testCaptureQueueFailureDiscards() async {
        let source = FakePCMSource()
        let capture = EngineAudioCapture(source: source, automaticPolling: false)
        try? capture.start(); source.captureFailed = true
        XCTAssertTrue(capture.stop().isEmpty)
        XCTAssertTrue(capture.captureFailed)
    }
}

@MainActor private final class FakePCMSource: CapturePCMSource {
    var isRunning = false
    var graphGeneration: UInt64 = 0
    var captureFailed = false
    var inputPeak: Float { 0.5 }
    var samples: [Int16] = []
    func startCapture() throws { isRunning = true; graphGeneration += 1 }
    func stopCapture() { isRunning = false; samples = [] }
    func readCapture(maximumFrames: Int) -> [Int16] {
        let result = Array(samples.prefix(maximumFrames))
        samples.removeFirst(result.count)
        return result
    }
}
