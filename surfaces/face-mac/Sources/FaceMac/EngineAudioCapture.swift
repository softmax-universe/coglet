import Foundation
#if os(macOS)
import FaceAudioKit
#endif

/// Adapter seam for the Face lane's 24 kHz mono PCM16 duplex kit.
@MainActor protocol CapturePCMSource: AnyObject {
    var isRunning: Bool { get }
    var graphGeneration: UInt64 { get }
    var captureFailed: Bool { get }
    var inputPeak: Float { get }
    func startCapture() throws
    func stopCapture()
    func readCapture(maximumFrames: Int) -> [Int16]
}
#if os(macOS)
extension FaceAudioKit: CapturePCMSource {}
#endif

/// Buffers an utterance above the reusable FaceAudioKit; owns no AVAudioEngine.
/// Float32 frames preserve the kit's canonical mono 24 kHz PCM timebase.
@MainActor public final class EngineAudioCapture: AudioCapture {
    public let frames: AsyncStream<PCMFrame>
    private let continuation: AsyncStream<PCMFrame>.Continuation
    private let source: any CapturePCMSource
    private let now: () -> TimeInterval
    private let automaticPolling: Bool
    private var poller: Task<Void, Never>?
    private var captured: [PCMFrame] = []
    private var startedAt: TimeInterval = 0
    private var sampleCount = 0
    private var generation: UInt64?
    public private(set) var captureFailed = false
    public var inputLevel: Float { generation == nil ? 0 : source.inputPeak }

    #if os(macOS)
    public convenience init() {
        self.init(source: FaceAudioKit(configuration: .init(echoCancellation: true)))
    }
    #endif
    init(source: any CapturePCMSource, automaticPolling: Bool = true,
         now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) {
        self.source = source; self.automaticPolling = automaticPolling; self.now = now
        (frames, continuation) = AsyncStream.makeStream(bufferingPolicy: .bufferingNewest(8))
    }
    public func start() throws {
        _ = stop(); captured = []; sampleCount = 0; captureFailed = false
        try source.startCapture()
        startedAt = now(); generation = source.graphGeneration
        if automaticPolling {
            poller = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(20))
                    guard !Task.isCancelled, self != nil else { return }
                    self?.poll()
                }
            }
        }
    }
    func poll() {
        guard let generation else { return }
        // A graph rebuild can switch devices or drop samples: cancel rather than
        // silently stitch a different microphone into the same utterance.
        guard source.isRunning, !source.captureFailed, source.graphGeneration == generation else {
            captureFailed = true; source.stopCapture(); self.generation = nil
            poller?.cancel(); poller = nil; captured = []; return
        }
        while sampleCount < 24_000 * 120 {
            let samples = source.readCapture(maximumFrames: min(24_000, 24_000 * 120 - sampleCount))
            if samples.isEmpty { break }
            let frame = PCMFrame(samples: samples.map { Float($0) / 32768 }, sampleRate: 24000,
                timestamp: startedAt + Double(sampleCount) / 24000)
            sampleCount += samples.count; captured.append(frame); continuation.yield(frame)
        }
    }
    public func stop() -> [PCMFrame] {
        poller?.cancel(); poller = nil
        poll() // Drain the final queued frames before FaceAudioKit discards its queue.
        source.stopCapture(); generation = nil
        let result = captured; captured = []
        return captureFailed ? [] : result
    }
}
