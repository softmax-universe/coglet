import Foundation

/// Mono, non-interleaved Float32 PCM; timestamp is monotonic seconds (system uptime).
public struct PCMFrame: Sendable, Equatable {
    public let samples: [Float]
    public let sampleRate: Double
    public let timestamp: TimeInterval
    public init(samples: [Float], sampleRate: Double, timestamp: TimeInterval) {
        self.samples = samples; self.sampleRate = sampleRate; self.timestamp = timestamp
    }
}

/// One capture session at a time. stop synchronously drains all captured frames;
/// the bounded async stream is for live observers, not authoritative utterance assembly.
@MainActor
public protocol AudioCapture: AnyObject {
    var frames: AsyncStream<PCMFrame> { get }
    var inputLevel: Float { get }
    func start() throws
    func stop() -> [PCMFrame]
}
