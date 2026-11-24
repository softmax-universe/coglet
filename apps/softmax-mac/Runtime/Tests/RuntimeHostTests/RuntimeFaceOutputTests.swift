import Foundation
import XCTest
import CogletTypes
import CogletSurface
import SoftmaxCore
import FaceMac
@testable import SoftmaxRuntime

@MainActor final class RuntimeNativePlaybackDevice: FacePlaybackDevice {
    var renderedFrames: Int64 = 0
    var callbacks: [@MainActor () -> Void] = []
    func schedule(_ frame: PCMFrame, completion: @escaping @MainActor () -> Void) throws { callbacks.append(completion) }
    func stop() { renderedFrames = 0 }
}
@MainActor final class RuntimeNativeKeys: PushToTalkKeySource {
    var onEvent: ((PushToTalkKeyEvent) -> Void)?
    func start() -> Bool { true }
    func stop() {}
}
@MainActor final class RuntimeNativeCapture: AudioCapture {
    let frames = AsyncStream<PCMFrame> { $0.finish() }
    var inputLevel: Float { 0 }
    func start() throws {}
    func stop() -> [PCMFrame] { [] }
}
@MainActor final class RuntimeNativeSynthesis: LocalSpeechSynthesis {
    func synthesize(_ text: String, completion: @escaping @MainActor (Result<[PCMFrame], Swift.Error>) -> Void) -> (@MainActor () -> Void) {
        completion(.success([PCMFrame(samples: [0.1, -0.1], sampleRate: 16000, timestamp: 0)])); return {}
    }
}

final class RuntimeFaceOutputTests: XCTestCase {
    @MainActor func testPlayedBackReceiptSurvivesStopBeforeReconciliation() throws {
        let output = RuntimeFaceOutput {}
        output.resume()
        let request = action(0)
        try output.admit(request)
        output.complete(request, error: nil)
        output.suspend()
        let receipts = output.drain()
        XCTAssertEqual(receipts.count, 1)
        let receipt = try XCTUnwrap(receipts.first)
        guard case .succeeded = receipt.result else { return XCTFail("Native success is immutable") }
    }
    private func action(_ index: Int, epoch: UInt64 = 1) -> RuntimeAction {
        RuntimeAction(action: .record([("capability", .text("speech.speak_chunk")),
            ("coglet", .text("coglet:echo")), ("id", .text(String(format: "%032x", index))),
            ("args", .record([("text", .text("hello")), ("stream", .text("s")),
                             ("chunk", .unsigned(0)), ("final", .boolean(true))]))]), epoch: epoch)
    }
    @MainActor func testBoundedAdmissionExactlyOnceAndStaleCompletion() throws {
        let output = RuntimeFaceOutput {}
        XCTAssertThrowsError(try output.admit(action(0)))
        output.resume()
        let requests = (0..<16).map { action($0) }
        for request in requests { try output.admit(request) }
        XCTAssertThrowsError(try output.preflight())
        XCTAssertThrowsError(try output.admit(action(16)))
        output.complete(requests[0], error: nil)
        output.complete(requests[0], error: "unavailable")
        XCTAssertFalse(output.isLive(requests[0]))
        let receipts = output.drain()
        XCTAssertEqual(receipts.count, 1)
        guard case .succeeded = receipts[0].result else { return XCTFail("First receipt must win") }
        output.suspend()
        output.complete(requests[1], error: nil)
        let cancelled = output.drain()
        XCTAssertEqual(cancelled.count, 15)
        for receipt in cancelled {
            guard case .failed(let error) = receipt.result else { return XCTFail("Stopped output cannot succeed") }
            XCTAssertEqual(error.code, "cancelled")
        }
        output.resume()
        let replacement = action(0, epoch: 2)
        try output.admit(replacement)
        output.complete(requests[0], error: nil)
        XCTAssertTrue(output.drain().isEmpty)
        XCTAssertTrue(output.isLive(replacement))
        output.suspend()
    }
}
