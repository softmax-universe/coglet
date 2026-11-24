#if os(macOS)
import XCTest
import CAudioRing
@testable import FaceAudioKit
@testable import FaceMac

@MainActor private final class InjectedGraph: DuplexEngine {
    var playback: PCMQueue?
    var starts = 0
    var stops = 0
    func start(configuration: AudioConfiguration, capture: PCMQueue, playback: PCMQueue,
               changed: @escaping @MainActor @Sendable () -> Void) throws {
        self.playback = playback; starts += 1
    }
    func stop() { stops += 1 }
    func render(_ frames: Int) -> Int {
        var samples = [Float](repeating: 0, count: frames)
        return samples.withUnsafeMutableBufferPointer { fa_render(playback!.raw, $0.baseAddress, frames, 1) }
    }
}
final class NativeDuplexTests: XCTestCase {
    func testAdapterUsesOneSharedGraphAndExactRenderedFrameCounter() async throws {
        try await MainActor.run {
            let graph = InjectedGraph()
            let kit = FaceAudioKit(factory: { graph }, permission: { true })
            let duplex = NativeFaceDuplex(kit: kit)
            XCTAssertEqual(duplex.renderedFrames, 0)
            try duplex.start(); try duplex.start()
            XCTAssertEqual(graph.starts, 1)
            try duplex.enqueue([1, 2, 3]); XCTAssertEqual(duplex.renderedFrames, 0)
            XCTAssertEqual(graph.render(2), 2); XCTAssertEqual(duplex.renderedFrames, 2)
            duplex.flush(); XCTAssertEqual(graph.render(2), 0)
            XCTAssertEqual(duplex.renderedFrames, 2)
            let generation = duplex.generation
            duplex.stop(); XCTAssertEqual(graph.stops, 1)
            XCTAssertEqual(duplex.renderedFrames, 2); XCTAssertGreaterThan(duplex.generation, generation)
            try duplex.start(); try duplex.enqueue([1, 2])
            XCTAssertEqual(graph.render(2), 2); XCTAssertEqual(duplex.renderedFrames, 4)
            XCTAssertEqual(kit.renderedPlaybackFrames, 4)
            duplex.stop()
        }
    }
}
#endif
