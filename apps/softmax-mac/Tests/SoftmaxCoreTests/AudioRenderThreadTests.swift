#if os(macOS)
import XCTest
import AVFoundation
@testable import FaceMac

@MainActor
final class AudioRenderThreadTests: XCTestCase, @unchecked Sendable {
    func testProductionTapCreatedOnMainActorRunsOnRenderExecutor() async throws {
        let pair = AsyncStream<PCMFrame>.makeStream(bufferingPolicy: .bufferingNewest(2))
        let storage = CaptureBuffer(pair.continuation)
        storage.reset()
        let callback = makeCaptureTap(storage)
        try await Task.detached {
            let format = try XCTUnwrap(AVAudioFormat(standardFormatWithSampleRate: 16000, channels: 1))
            let pcm = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 2))
            pcm.frameLength = 2
            let data = try XCTUnwrap(pcm.floatChannelData)
            data[0][0] = 0.5; data[0][1] = -0.5
            callback(pcm, AVAudioTime(sampleTime: 0, atRate: 16000))
        }.value
        XCTAssertEqual(storage.meter(), 0.5, accuracy: 0.001)
        let frames = storage.drain()
        XCTAssertEqual(frames.count, 1)
        XCTAssertEqual(frames.first?.samples, [0.5, -0.5])
        XCTAssertEqual(frames.first?.sampleRate, 16000)
        XCTAssertEqual(storage.meter(), 0)
        XCTAssertTrue(storage.drain().isEmpty)
    }
}
#endif
