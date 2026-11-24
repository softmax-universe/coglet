#if os(macOS)
import FaceAudioKit

/// One FaceAudioKit owns BOTH capture and playback. Never combine with the
/// legacy standalone EngineFacePlayback, which owns a separate output engine.
@MainActor public final class NativeFaceDuplex: FaceDuplexIO {
    public let kit: FaceAudioKit
    public init(kit: FaceAudioKit) { self.kit = kit }
    public var renderedFrames: UInt64 { kit.renderedPlaybackFrames }
    public var isRunning: Bool { kit.isRunning }
    public var generation: UInt64 { kit.graphGeneration }
    public var captureFailed: Bool { kit.captureFailed || kit.lastError != nil }
    public func start() throws { try kit.startCapture() }
    public func stop() { kit.stopCapture() }
    public func enqueue(_ samples: [Int16]) throws { try kit.enqueuePlayback(samples) }
    public func flush() { kit.flushPlayback() }
    public func readCapture() -> [Int16] { kit.readCapture() }
}
#endif
