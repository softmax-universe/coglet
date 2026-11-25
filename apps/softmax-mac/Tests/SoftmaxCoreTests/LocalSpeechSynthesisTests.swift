import XCTest
import FaceMac
@testable import SoftmaxCore
#if os(macOS)
import AVFoundation

final class LocalSpeechSynthesisTests: XCTestCase {
    func testCollectorCopiesAndSplitsBeforeNativeStorageIsReused() throws {
        let collector = SpeechPCMCollector(timestamp: 10)
        let buffer = try makeBuffer(rate: 8000, count: 20000)
        for index in 0..<20000 { buffer.floatChannelData![0][index] = Float(index) / 20000 }
        XCTAssertNil(collector.consume(buffer))
        buffer.floatChannelData![0][0] = -1
        let frames = try XCTUnwrap(collector.consume(try makeBuffer(rate: 8000, count: 0))).get()
        XCTAssertEqual(frames.map { $0.samples.count }, [8000, 8000, 4000])
        XCTAssertEqual(frames[0].samples[0], 0)
        XCTAssertEqual(frames.map(\.timestamp), [10, 11, 12])
        XCTAssertNil(collector.consume(buffer))
    }

    func testCollectorRejectsEmptyNonfiniteOversizedAndChangingRate() throws {
        let empty = SpeechPCMCollector(timestamp: 0)
        assertFailure(empty.consume(try makeBuffer(rate: 24_000, count: 0)), .emptyPCM)
        let nonfinite = SpeechPCMCollector(timestamp: 0)
        let buffer = try makeBuffer(rate: 24_000, count: 1)
        buffer.floatChannelData![0][0] = .nan
        assertFailure(nonfinite.consume(buffer), .invalidPCM)
        XCTAssertNil(nonfinite.consume(buffer))
        assertFailure(SpeechPCMCollector(timestamp: 0).consume(
            try makeBuffer(rate: 24_000, count: 65_537)), .limitExceeded)
        assertFailure(SpeechPCMCollector(timestamp: 0).consume(
            try makeBuffer(rate: 96_000, count: 1)), .invalidPCM)
        let changing = SpeechPCMCollector(timestamp: 0)
        XCTAssertNil(changing.consume(try makeBuffer(rate: 24_000, count: 1)))
        assertFailure(changing.consume(try makeBuffer(rate: 48_000, count: 1)), .invalidPCM)
    }

    func testCollectorBoundsTotalSamplesAndCallbackCount() throws {
        let full = SpeechPCMCollector(timestamp: 0)
        let second = try makeBuffer(rate: 48_000, count: 48_000)
        for _ in 0..<40 { XCTAssertNil(full.consume(second)) }
        assertFailure(full.consume(second), .limitExceeded)
        let callbacks = SpeechPCMCollector(timestamp: 0)
        let tiny = try makeBuffer(rate: 48_000, count: 1)
        for _ in 0..<4_096 { XCTAssertNil(callbacks.consume(tiny)) }
        assertFailure(callbacks.consume(tiny), .limitExceeded)
        XCTAssertNil(callbacks.consume(tiny))
    }

    func testCollectorAcceptsExactSampleLimit() throws {
        let collector = SpeechPCMCollector(timestamp: 0)
        let second = try makeBuffer(rate: 48_000, count: 48_000)
        for _ in 0..<40 { XCTAssertNil(collector.consume(second)) }
        let frames = try XCTUnwrap(collector.consume(try makeBuffer(rate: 48_000, count: 0))).get()
        XCTAssertEqual(frames.reduce(0) { $0 + $1.samples.count }, 1_920_000)
        XCTAssertTrue(frames.allSatisfy { Double($0.samples.count) <= $0.sampleRate })
    }

    func testCollectorBoundsMetadataForPathologicalSampleRates() throws {
        let collector = SpeechPCMCollector(timestamp: 0)
        assertFailure(collector.consume(try makeBuffer(rate: 1, count: 8_193)), .invalidPCM)
    }

    func testCollectorRejectsStereoAndFloat64() throws {
        for format in [
            AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 24_000, channels: 2, interleaved: false),
            AVAudioFormat(commonFormat: .pcmFormatFloat64, sampleRate: 24_000, channels: 1, interleaved: false)
        ] {
            let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: XCTUnwrap(format), frameCapacity: 1))
            buffer.frameLength = 1
            assertFailure(SpeechPCMCollector(timestamp: 0).consume(buffer), .invalidPCM)
        }
    }

    func testCollectorCancellationFencesNativeCallbacks() throws {
        let collector = SpeechPCMCollector(timestamp: 0)
        let buffer = try makeBuffer(rate: 24_000, count: 1)
        XCTAssertNil(collector.consume(buffer))
        collector.cancel()
        XCTAssertNil(collector.consume(buffer))
        XCTAssertNil(collector.consume(try makeBuffer(rate: 24_000, count: 0)))
    }

    func testIntegerAndFloatPCMInBothLayouts() throws {
        for interleaved in [true, false] {
            for common in [AVAudioCommonFormat.pcmFormatFloat32, .pcmFormatInt16, .pcmFormatInt32] {
                let format = try XCTUnwrap(AVAudioFormat(commonFormat: common,
                    sampleRate: 24_000, channels: 1, interleaved: interleaved))
                let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 2))
                buffer.frameLength = 2
                let data = try XCTUnwrap(UnsafeMutableAudioBufferListPointer(buffer.mutableAudioBufferList)[0].mData)
                switch common {
                case .pcmFormatFloat32:
                    data.storeBytes(of: Float(-1), as: Float.self)
                    data.advanced(by: 4).storeBytes(of: Float(0.5), as: Float.self)
                case .pcmFormatInt16:
                    data.storeBytes(of: Int16.min, as: Int16.self)
                    data.advanced(by: 2).storeBytes(of: Int16(16_384), as: Int16.self)
                case .pcmFormatInt32:
                    data.storeBytes(of: Int32.min, as: Int32.self)
                    data.advanced(by: 4).storeBytes(of: Int32(1_073_741_824), as: Int32.self)
                default: XCTFail("Unexpected test format")
                }
                let collector = SpeechPCMCollector(timestamp: 0)
                XCTAssertNil(collector.consume(buffer))
                let frames = try XCTUnwrap(collector.consume(try makeBuffer(rate: 24_000, count: 0))).get()
                XCTAssertEqual(frames.flatMap(\.samples), [-1, 0.5])
            }
        }
    }

    @MainActor
    func testEmptyTextCompletesOnceAndReturnedCancellationIsStale() {
        let synthesis = AppleSpeechSynthesis()
        var calls = 0
        let cancel = synthesis.synthesize(" ") { result in
            calls += 1
            guard case .failure(let error) = result else { return XCTFail("Expected invalid text") }
            XCTAssertEqual(error as? LocalSpeechSynthesisError, .invalidText)
        }
        cancel()
        cancel()
        XCTAssertEqual(calls, 1)
    }

    private func assertFailure(_ result: Result<[PCMFrame], Error>?,
                               _ expected: LocalSpeechSynthesisError,
                               file: StaticString = #filePath, line: UInt = #line) {
        guard case .failure(let error)? = result else {
            return XCTFail("Expected terminal failure", file: file, line: line)
        }
        XCTAssertEqual(error as? LocalSpeechSynthesisError, expected, file: file, line: line)
    }

    private func makeBuffer(rate: Double, count: AVAudioFrameCount) throws -> AVAudioPCMBuffer {
        let format = try XCTUnwrap(AVAudioFormat(commonFormat: .pcmFormatFloat32,
                                               sampleRate: rate, channels: 1, interleaved: false))
        let buffer = try XCTUnwrap(AVAudioPCMBuffer(pcmFormat: format, frameCapacity: max(1, count)))
        buffer.frameLength = count
        for index in 0..<Int(count) { buffer.floatChannelData![0][index] = 0 }
        return buffer
    }
}
#endif
