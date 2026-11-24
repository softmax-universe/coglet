import XCTest
@testable import FaceMac
#if os(macOS)
import AVFoundation
#endif

final class LocalSpeechTests: XCTestCase {
    func testCanonicalPCMQuantizationAndNonfiniteFailClosed() throws {
        XCTAssertEqual(try [-2, -1, -0.5, 0, 0.5, 0.999999, 1, 2].map { try LocalSpeechPCM.quantize(Float($0)) },
                       [.min, .min, -16384, 0, 16384, .max, .max, .max])
        XCTAssertThrowsError(try LocalSpeechPCM.quantize(.nan))
        XCTAssertThrowsError(try LocalSpeechPCM.quantize(.infinity))
    }
    func testResamplingKeepsPhaseAcrossCallbackBoundaries() throws {
        let input = (0..<1000).map { Float($0 % 71) / 71 - 0.5 }
        for rate in [16_000.0, 22_050, 24_000, 44_100, 48_000] {
            var whole = LocalSpeechPCM(), chunks = LocalSpeechPCM()
            try whole.append(mono: input, rate: rate)
            for start in stride(from: 0, to: input.count, by: 37) {
                try chunks.append(mono: Array(input[start..<min(start + 37, input.count)]), rate: rate)
            }
            XCTAssertEqual(whole.samples, chunks.samples)
            XCTAssertFalse(whole.samples.isEmpty)
        }
    }
    func testConversionBoundAndFormatChangesFailClosed() throws {
        var pcm = LocalSpeechPCM(limit: 3)
        try pcm.append(mono: [0, 0.5, -0.5], rate: 24_000)
        XCTAssertEqual(pcm.samples, [0, 16384, -16384])
        XCTAssertThrowsError(try pcm.append(mono: [0], rate: 24_000))
        XCTAssertEqual(pcm.samples.count, 3)
        XCTAssertThrowsError(try pcm.append(mono: [0], rate: 48_000))
        XCTAssertThrowsError(try pcm.append(mono: [0], rate: .nan))
        XCTAssertThrowsError(try pcm.append(mono: [.nan], rate: 24_000))
    }
    func testReplacementAndTerminalCustodyAreSingleUse() {
        var custody = LocalSpeechCustody()
        let old = custody.replace(), current = custody.replace()
        XCTAssertFalse(custody.accepts(old))
        XCTAssertFalse(custody.retire(old))
        XCTAssertTrue(custody.accepts(current))
        XCTAssertTrue(custody.retire(current))
        XCTAssertFalse(custody.retire(current))
        XCTAssertFalse(custody.accepts(old))
    }

    #if os(macOS)
    func testNativePlanarAndInterleavedStereoConversion() throws {
        for interleaved in [true, false] {
            let format = try XCTUnwrap(AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000,
                                                     channels: 2, interleaved: interleaved))
            let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4))
            buffer.frameLength = 4
            let data = try XCTUnwrap(buffer.floatChannelData)
            for frame in 0..<4 {
                if interleaved { data[0][frame * 2] = 0.25; data[0][frame * 2 + 1] = 0.75 }
                else { data[0][frame] = 0.25; data[1][frame] = 0.75 }
            }
            XCTAssertEqual(try AppleSpeechCollector.copyMono(buffer), [0.5, 0.5, 0.5, 0.5])
            let collector = AppleSpeechCollector()
            XCTAssertNil(collector.receive(buffer))
            buffer.frameLength = 0
            let result = try XCTUnwrap(collector.receive(buffer))
            XCTAssertEqual(try result.get(), [16384, 16384])
            XCTAssertNil(collector.receive(buffer))
        }
    }
    func testNativeCollectorCancellationAndMalformedAudio() throws {
        let format = try XCTUnwrap(AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1))
        let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1))
        buffer.frameLength = 1; buffer.floatChannelData![0][0] = .nan
        let collector = AppleSpeechCollector()
        XCTAssertThrowsError(try XCTUnwrap(collector.receive(buffer)).get())
        XCTAssertNil(collector.receive(buffer))
        let cancelled = AppleSpeechCollector(); cancelled.cancel()
        XCTAssertNil(cancelled.receive(buffer))
    }
    func testWriteCallbackHopsFromBackgroundToMainActor() async throws {
        let complete = expectation(description: "terminal callback")
        let collector = AppleSpeechCollector()
        let callback = AppleSpeechCollector.callback(collector) { result in
            MainActor.assertIsolated()
            if case .success = result { XCTFail("empty synthesis must fail closed") }
            complete.fulfill()
        }
        DispatchQueue.global().async {
            let format = AVAudioFormat(standardFormatWithSampleRate: 24_000, channels: 1)!
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 1)!
            buffer.frameLength = 0; callback(buffer)
        }
        await fulfillment(of: [complete], timeout: 3)
    }
    #endif
}
