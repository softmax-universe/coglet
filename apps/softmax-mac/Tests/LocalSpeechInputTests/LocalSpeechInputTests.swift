import XCTest
import Speech
@testable import SoftmaxCore
#if SWIFT_PACKAGE
import SpeechCallbackFixture
#endif

/// Uses the real app source (the adjacent symlink), not a Swift closure imitation:
/// the imported legacy Objective-C block thunk is what trapped in the signed RC.
@MainActor final class LocalSpeechInputTests: XCTestCase {
    private func waitFor(_ predicate: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(3)
        while !predicate() {
            guard ContinuousClock.now < deadline else {
                XCTFail("Speech callback was not registered"); throw CancellationError()
            }
            try await Task.sleep(for: .milliseconds(5))
        }
    }
    private var utterance: Utterance { Utterance(operationID: 1, frames: [], startedAt: 0, endedAt: 1) }
    private func authorize(_ index: UInt, _ status: SFSpeechRecognizerAuthorizationStatus) async {
        await withCheckedContinuation { continuation in
            SpeechFixtureAuthorize(index, status) { @Sendable in continuation.resume() }
        }
    }
    private func recognize(_ index: UInt, _ text: String, final: Bool = true, failed: Bool = false) async {
        // Acknowledge actual background invocation/return, not just enqueueing.
        await withCheckedContinuation { continuation in
            SpeechFixtureRecognize(index, text, final, failed) { @Sendable in continuation.resume() }
        }
    }
    private func assertUnavailable(_ task: Task<String, any Error>, file: StaticString = #filePath, line: UInt = #line) async {
        do { _ = try await task.value; XCTFail("Unavailable speech must fail", file: file, line: line) }
        catch { XCTAssertTrue(error.localizedDescription.contains("No audio was uploaded"), file: file, line: line) }
    }

    func testBackgroundAuthorizationDeniedCompletesWithoutRecognition() async throws {
        SpeechFixtureInstall()
        let input = LocalSpeechInput(); defer { input.cancel() }
        let task = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        await authorize(0, .denied)
        await assertUnavailable(task)
        XCTAssertEqual(SpeechFixtureRecognitionCount(), 0)
    }

    func testBackgroundAuthorizationAuthorizedAndRecognitionFinalComplete() async throws {
        SpeechFixtureInstall()
        _ = try XCTUnwrap(SFSpeechRecognizer(), "Native Speech test host must support its default locale")
        let input = LocalSpeechInput(); defer { input.cancel() }
        let task = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        await authorize(0, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 1 }
        await recognize(0, "local final")
        let text = try await task.value
        XCTAssertEqual(text, "local final")
        XCTAssertEqual(SpeechFixtureCancellationCount(), 1)
    }

    func testBackgroundRecognitionErrorFailsAndCancelsExactlyOnce() async throws {
        SpeechFixtureInstall()
        let input = LocalSpeechInput(); defer { input.cancel() }
        let task = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        await authorize(0, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 1 }
        await recognize(0, "partial", final: false, failed: true)
        await assertUnavailable(task)
        XCTAssertEqual(SpeechFixtureCancellationCount(), 1)
    }

    func testBackgroundPartialIsNotDeliveredAsFinal() async throws {
        SpeechFixtureInstall()
        let input = LocalSpeechInput(); defer { input.cancel() }
        let task = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        await authorize(0, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 1 }
        await recognize(0, "not final", final: false)
        await recognize(0, "complete")
        let text = try await task.value
        XCTAssertEqual(text, "complete")
        XCTAssertEqual(SpeechFixtureCancellationCount(), 1)
    }

    func testCancelledAuthorizationCannotStartRecognitionForNewRevision() async throws {
        SpeechFixtureInstall()
        let input = LocalSpeechInput(); defer { input.cancel() }
        let old = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        input.cancel()
        let current = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 2 }
        await authorize(0, .authorized)
        await assertUnavailable(old)
        XCTAssertEqual(SpeechFixtureRecognitionCount(), 0)
        await authorize(1, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 1 }
        await recognize(0, "current")
        let text = try await current.value
        XCTAssertEqual(text, "current")
    }

    func testCancelledRecognitionLateFinalAndErrorCannotFinishNewRevision() async throws {
        SpeechFixtureInstall()
        let input = LocalSpeechInput(); defer { input.cancel() }
        let old = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 1 }
        await authorize(0, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 1 }
        input.cancel()
        await assertUnavailable(old)
        XCTAssertEqual(SpeechFixtureCancellationCount(), 1)
        let current = Task { try await input.transcribe(utterance) }
        try await waitFor { SpeechFixtureAuthorizationCount() == 2 }
        await authorize(1, .authorized)
        try await waitFor { SpeechFixtureRecognitionCount() == 2 }
        await recognize(0, "stale final")
        await recognize(0, "stale error", final: false, failed: true)
        await recognize(1, "current")
        let text = try await current.value
        XCTAssertEqual(text, "current")
        XCTAssertEqual(SpeechFixtureCancellationCount(), 2)
    }
}
