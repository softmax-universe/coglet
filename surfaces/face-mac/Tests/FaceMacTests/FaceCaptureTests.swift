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
    var route = "local"
    var usesRemoteAudio = false
    var inputs: [TestRecognitionInput] = []
    var callbacks: [@MainActor (FaceRecognitionUpdate) throws -> Void] = []
    var failures: [@MainActor () throws -> Void] = []
    var synchronousUpdate: FaceRecognitionUpdate?
    var synchronousFailure = false
    func start(utterance: String, receive: @escaping @MainActor (FaceRecognitionUpdate) throws -> Void,
               failed: @escaping @MainActor () throws -> Void) throws -> any FaceRecognitionInput {
        let input = TestRecognitionInput(); inputs.append(input); callbacks.append(receive); failures.append(failed)
        if let synchronousUpdate { try receive(synchronousUpdate) }
        if synchronousFailure { try failed() }
        return input
    }
}
final class FaceCaptureTests: XCTestCase {
    func testReplacementClosesOldUtteranceBeforeStartingNewOneAndTimeoutClosesFinal() async throws {
        try await MainActor.run {
            let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
            let recognition = TestRecognition()
            var events: [(String, CBORValue)] = []
            face.activate(owner: "coglet:" + String(repeating: "a", count: 64), epoch: 1) { events.append(($0, $1)) }
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 0)
            try recognition.callbacks[0](.init(revision: 4, text: "unfinished", final: false, positionMS: 10))
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "2", count: 32), positionMS: 20)
            XCTAssertEqual(events.map(\.0), ["speech.start", "speech.partial", "speech.end", "speech.start"])
            XCTAssertEqual(events[2].1["revision"], .unsigned(5))
            XCTAssertEqual(events[2].1["text"], .text(""), "failure must not invent a final transcript")
            face.finishCapture()
            try face.tick(now: 5_000_000)
            XCTAssertEqual(events.last?.0, "speech.end")
            XCTAssertTrue(recognition.inputs[1].cancelled)
            let count = events.count
            try recognition.callbacks[1](.init(revision: 9, text: "late", final: true, positionMS: 30))
            XCTAssertEqual(events.count, count)
        }
    }
    func testSynchronousFinalAndFailureRetireReturnedHandleAndCannotPublishTwice() async throws {
        try await MainActor.run {
            for failed in [false, true] {
                let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
                let recognition = TestRecognition()
                recognition.synchronousFailure = failed
                if !failed { recognition.synchronousUpdate = .init(revision: 1, text: "done", final: true, positionMS: 10) }
                var events: [(String, CBORValue)] = []
                face.activate(owner: "coglet:" + String(repeating: "a", count: 64), epoch: 1) { events.append(($0, $1)) }
                try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 0)
                XCTAssertEqual(events.map(\.0), ["speech.start", "speech.end"])
                XCTAssertEqual(events.last?.1["text"], .text(failed ? "" : "done"))
                XCTAssertTrue(recognition.inputs[0].cancelled)
                try recognition.callbacks[0](.init(revision: 2, text: "duplicate", final: true, positionMS: 20))
                try recognition.failures[0]()
                XCTAssertEqual(events.count, 2)
                XCTAssertEqual(face.discardedRecognitionUpdates, 1)
            }
        }
    }
    func testCaptureAndFinalizationDeadlinesAndAsyncFailureCloseExactlyOnce() async throws {
        try await MainActor.run {
            let face = FaceSurfaceBackend(audio: TestDuplex(), synthesis: TestSynthesis(), renderer: .init { _ in })
            let recognition = TestRecognition()
            var events: [(String, CBORValue)] = []
            face.activate(owner: "coglet:" + String(repeating: "a", count: 64), epoch: 1) { events.append(($0, $1)) }
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 30, now: 100)
            try face.tick(now: 60_000_099)
            XCTAssertEqual(events.count, 1)
            try face.tick(now: 60_000_100)
            XCTAssertEqual(events.last?.0, "speech.end")
            XCTAssertEqual(events.last?.1["position_ms"], .unsigned(30))
            try recognition.failures[0]()
            XCTAssertEqual(events.count, 2)
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "2", count: 32), positionMS: 40, now: 60_000_100)
            face.finishCapture(now: 60_000_200)
            face.finishCapture(now: 99_000_000) // repeated release cannot extend timeout
            try face.tick(now: 65_000_199)
            XCTAssertEqual(events.count, 3)
            try face.tick(now: 65_000_200)
            XCTAssertEqual(events.count, 4)
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "3", count: 32), positionMS: 50)
            try recognition.failures[2]()
            XCTAssertEqual(events.count, 6)
            XCTAssertTrue(recognition.inputs[2].cancelled)
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "4", count: 32), positionMS: 60)
            face.stop(owner: nil)
            try recognition.failures[3]()
            try recognition.callbacks[3](.init(revision: 1, text: "private", final: true, positionMS: 60))
            XCTAssertEqual(events.count, 7, "revocation must not deliver terminal private observations")
        }
    }
    func testConfiguredRoutesAndSeparateRemoteConsentAreRequiredWithoutFallback() async throws {
        try await MainActor.run {
            let io = TestDuplex(), tts = TestSynthesis(), recognition = TestRecognition()
            let face = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }, sttRoute: "configured-stt", ttsRoute: "configured-tts")
            let owner = "coglet:" + String(repeating: "a", count: 64)
            var events: [String] = []
            face.activate(owner: owner, epoch: 1) { kind, _ in events.append(kind) }
            XCTAssertThrowsError(try face.startDuplex())
            XCTAssertFalse(io.isRunning)
            tts.route = "configured-tts"; tts.usesRemoteAudio = true
            XCTAssertThrowsError(try face.startDuplex())
            XCTAssertFalse(io.isRunning)
            face.activate(owner: owner, epoch: 1, remoteAudioConsent: true) { kind, _ in events.append(kind) }
            try face.startDuplex()
            XCTAssertTrue(io.isRunning)
            XCTAssertThrowsError(try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 0))
            XCTAssertTrue(recognition.inputs.isEmpty)
            XCTAssertTrue(events.isEmpty)
            recognition.route = "configured-stt"; recognition.usesRemoteAudio = true
            try face.beginCapture(recognition: recognition, utterance: String(repeating: "1", count: 32), positionMS: 0)
            XCTAssertEqual(events, ["speech.start"])
            let config = try face.read(operation: "speech.config", args: .map([]), owner: owner, now: 0)
            XCTAssertEqual(config["stt"]?["route"], .text("configured-stt"))
            XCTAssertEqual(config["tts"]?["route"], .text("configured-tts"))
            face.stop(owner: nil)
            face.activate(owner: owner, epoch: 2) { kind, _ in events.append(kind) }
            XCTAssertThrowsError(try face.beginCapture(recognition: recognition, utterance: String(repeating: "2", count: 32), positionMS: 0))
            XCTAssertEqual(recognition.inputs.count, 1, "remote consent cannot survive revocation")
        }
    }
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
