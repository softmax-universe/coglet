#if os(macOS)
import AVFoundation
import Foundation

/// Owns callback-thread state; never passes AVAudioPCMBuffer across isolation domains.
private final class CaptureBuffer: @unchecked Sendable {
    private let lock = NSLock()
    private var captured: [PCMFrame] = []
    private var level: Float = 0
    private var active = false
    private var start: TimeInterval = 0
    private var duration: Double = 0
    let continuation: AsyncStream<PCMFrame>.Continuation
    init(_ continuation: AsyncStream<PCMFrame>.Continuation) { self.continuation = continuation }
    func reset() {
        lock.lock(); defer { lock.unlock() }
        captured = []; level = 0; duration = 0
        start = ProcessInfo.processInfo.systemUptime; active = true
    }
    func append(_ buffer: AVAudioPCMBuffer) {
        guard let channels = buffer.floatChannelData else { return }
        lock.lock(); defer { lock.unlock() }
        guard active, duration < 120, buffer.format.sampleRate > 0 else { return }
        let rate = buffer.format.sampleRate
        let count = min(Int(buffer.frameLength), Int((120 - duration) * rate))
        guard count > 0 else { return }
        let channelCount = Int(buffer.format.channelCount)
        guard channelCount > 0 else { return }
        var samples = [Float](repeating: 0, count: count)
        for channel in 0..<channelCount {
            for index in 0..<count { samples[index] += channels[channel][index] / Float(channelCount) }
        }
        level = min(1, sqrt(samples.reduce(0) { $0 + $1 * $1 } / Float(count)))
        let frame = PCMFrame(samples: samples, sampleRate: rate, timestamp: start + duration)
        duration += Double(count) / rate
        captured.append(frame); continuation.yield(frame)
    }
    func meter() -> Float { lock.lock(); defer { lock.unlock() }; return level }
    func drain() -> [PCMFrame] {
        lock.lock(); defer { lock.unlock() }
        active = false; level = 0
        let result = captured; captured = []; return result
    }
}

@MainActor public final class EngineAudioCapture: AudioCapture {
    public let frames: AsyncStream<PCMFrame>
    private let buffer: CaptureBuffer
    private var engine: AVAudioEngine?
    public var inputLevel: Float { buffer.meter() }
    public init() {
        let pair = AsyncStream<PCMFrame>.makeStream(bufferingPolicy: .bufferingNewest(8))
        frames = pair.stream; buffer = CaptureBuffer(pair.continuation)
    }
    public func start() throws {
        _ = stop()
        guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else {
            throw CocoaError(.userCancelled)
        }
        let engine = AVAudioEngine()
        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(true)
        let format = input.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else { throw CocoaError(.fileReadUnknown) }
        buffer.reset()
        let storage = buffer
        input.installTap(onBus: 0, bufferSize: 1024, format: format) { pcm, _ in storage.append(pcm) }
        self.engine = engine
        do { engine.prepare(); try engine.start() }
        catch { _ = stop(); throw error }
    }
    public func stop() -> [PCMFrame] {
        // Close buffer before stopping so any in-flight callback is discarded.
        let frames = buffer.drain()
        if let engine { engine.stop(); engine.inputNode.removeTap(onBus: 0) }
        engine = nil
        return frames
    }
}
#endif
