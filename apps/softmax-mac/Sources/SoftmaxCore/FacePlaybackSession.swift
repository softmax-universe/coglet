import FaceMac
import Foundation

/// The shared PTT/playback integration; the app forwards stopPlayback to stop().
/// Owns no synthesis, grants or surface action admission.
@MainActor public final class FacePlaybackSession {
    public let speech: FaceSpeechSink
    private let ptt: PushToTalk
    private var operation: UInt64?
    public var onFinish: (() -> Void)?
    public init(ptt: PushToTalk, speech: FaceSpeechSink) {
        self.ptt = ptt; self.speech = speech
        speech.onFinish = { [weak self] _ in
            guard let self else { return }
            let completed = operation; operation = nil
            if let completed { ptt.complete(operation: completed, state: .speaking) }
            onFinish?()
        }
    }
    public func begin() -> UUID {
        // Flush old audio and its completion BEFORE creating a new token.
        speech.stop()
        ptt.setState(.speaking)
        operation = ptt.operationID
        return speech.begin()
    }
    public func stop() { speech.stop() }
}
