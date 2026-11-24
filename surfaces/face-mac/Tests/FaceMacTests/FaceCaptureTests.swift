import XCTest
import CogletTypes
import CogletSurface
@testable import FaceMac

@MainActor final class TestRecognitionInput: FaceRecognitionInput {
    var received: [[Int16]] = []
    var finished = false
    var cancelled = false
    func append(_ samples: [Int16]) throws { received.append(samples) }
    func finish() { finished = true }
    func cancel() { cancelled = true }
}
@MainActor final class TestRecognition: FaceRecognition {
    var inputs: [TestRecognitionInput] = []
    var callbacks: [@MainActor (FaceRecognitionUpdate) throws -> Void] = []
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void) throws -> any FaceRecognitionInput {
        let input = TestRecognitionInput(); inputs.append(input); callbacks.append(receive); return input
    }
}
final class FaceCaptureTests: XCTestCase {
    func testRecognitionNormalizesFinalAndFencesOldGenerationAfterReactivation() async throws {
        try await MainActor.run {
            let io = TestDuplex(), recognition = TestRecognition()
            let face = FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in })
            var observed: [String] = []
            face.activate(owner: "coglet:" + String(repeating: "a", count: 64), epoch: 1) { kind, _ in observed.append(kind) }
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 0)
            io.captured = [1, 2, 3]; try face.pollCapture()
            XCTAssertEqual(recognition.inputs[0].received, [[1, 2, 3]])
            face.finishCapture(); XCTAssertTrue(recognition.inputs[0].finished)
            try recognition.callbacks[0](.init(revision: 1, text: "hi", final: false, positionMS: 10))
            try recognition.callbacks[0](.init(revision: 2, text: "hi there", final: true, positionMS: 20))
            try recognition.callbacks[0](.init(revision: 3, text: "late", final: false, positionMS: 30))
            XCTAssertEqual(observed, ["speech.start", "speech.partial", "speech.end"])
            face.stop(owner: nil)
            face.activate(owner: "coglet:" + String(repeating: "a", count: 64), epoch: 2) { kind, _ in observed.append(kind) }
            try recognition.callbacks[0](.init(revision: 4, text: "stale", final: true, positionMS: 30))
            XCTAssertEqual(observed.count, 3)
            XCTAssertTrue(recognition.inputs[0].cancelled)
        }
    }
}
