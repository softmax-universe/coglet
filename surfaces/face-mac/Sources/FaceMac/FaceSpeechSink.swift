import Foundation

/// Device boundary. Completion must mean rendered, not merely scheduled.
@MainActor public protocol FacePlaybackDevice: AnyObject {
    var renderedFrames: Int64 { get }
    func schedule(_ frame: PCMFrame, completion: @escaping @MainActor () -> Void) throws
    /// Synchronous native stop and queue flush; never awaits a runtime action.
    func stop()
}
public enum FacePlaybackError: Error { case stale, invalidPCM, capacity }

/// TTS PCM consumer. The caller owns synthesis, grants, action outcomes and deadlines.
/// A token fences every synthesis callback; stop is terminal for that token.
@MainActor public final class FaceSpeechSink {
    private let device: any FacePlaybackDevice
    private var token: UUID?
    private var pending = 0
    private var ended = false
    private var rate: Double?
    private var scheduled: Int64 = 0
    private var envelopes: [(end: Int64, level: Double)] = []
    public private(set) var level: Double = 0
    public var active: Bool { token != nil }
    public var onChange: (() -> Void)?
    public var onFinish: ((_ cancelled: Bool) -> Void)?
    public init(device: any FacePlaybackDevice) { self.device = device }
    public func begin() -> UUID {
        stop()
        let value = UUID(); token = value; return value
    }
    public func enqueue(_ frame: PCMFrame, token: UUID) throws {
        guard self.token == token, !ended else { throw FacePlaybackError.stale }
        guard frame.sampleRate.isFinite, (8000...192000).contains(frame.sampleRate),
              !frame.samples.isEmpty, frame.samples.allSatisfy({ $0.isFinite && abs($0) <= 1 }),
              rate == nil || rate == frame.sampleRate else { throw FacePlaybackError.invalidPCM }
        // Bound both memory and native scheduled audio; producers must apply backpressure.
        let unheard = scheduled - max(0, device.renderedFrames)
        guard frame.samples.count <= Int(frame.sampleRate),
              unheard + Int64(frame.samples.count) <= Int64(frame.sampleRate * 10),
              envelopes.count < 512 else { throw FacePlaybackError.capacity }
        rate = frame.sampleRate
        scheduled += Int64(frame.samples.count)
        let rms = sqrt(frame.samples.reduce(0.0) { $0 + Double($1) * Double($1) } / Double(frame.samples.count))
        envelopes.append((scheduled, rms)); pending += 1
        do {
            try device.schedule(frame) { [weak self] in
                guard let self, self.token == token else { return }
                self.pending -= 1
                self.tick(); self.completeIfDrained()
            }
        } catch { stop(); throw error }
    }
    public func finish(token: UUID) throws {
        guard self.token == token else { throw FacePlaybackError.stale }
        ended = true; completeIfDrained()
    }
    public func tick() {
        let position = max(0, device.renderedFrames)
        envelopes.removeAll { $0.end <= position }
        level = position > 0 ? envelopes.first?.level ?? 0 : 0
        onChange?()
    }
    public func stop() {
        let wasActive = active
        token = nil // Fence callbacks before native stop can deliver them.
        device.stop(); clear()
        onChange?()
        if wasActive { onFinish?(true) }
    }
    private func clear() {
        pending = 0; ended = false; rate = nil; scheduled = 0; envelopes.removeAll(); level = 0
    }
    private func completeIfDrained() {
        guard active, ended, pending == 0 else { return }
        token = nil; device.stop(); clear(); onChange?(); onFinish?(false)
    }
}

#if os(macOS)
@preconcurrency import AVFoundation

/// AVAudioPlayerNode stop() flushes all scheduled buffers locally.
@MainActor public final class EngineFacePlayback: FacePlaybackDevice {
    private let engine = AVAudioEngine()
    private let player = AVAudioPlayerNode()
    private var rate: Double?
    public init() { engine.attach(player) }
    public var renderedFrames: Int64 {
        guard let time = player.lastRenderTime, let position = player.playerTime(forNodeTime: time) else { return 0 }
        return max(0, position.sampleTime)
    }
    public func schedule(_ frame: PCMFrame, completion: @escaping @MainActor () -> Void) throws {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: frame.sampleRate, channels: 1),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(frame.samples.count)),
              let data = buffer.floatChannelData else { throw FacePlaybackError.invalidPCM }
        if rate != frame.sampleRate {
            guard !player.isPlaying else { throw FacePlaybackError.invalidPCM }
            engine.connect(player, to: engine.mainMixerNode, format: format); rate = frame.sampleRate
        }
        buffer.frameLength = buffer.frameCapacity
        frame.samples.withUnsafeBufferPointer { source in data[0].update(from: source.baseAddress!, count: source.count) }
        if !engine.isRunning { try engine.start() }
        player.scheduleBuffer(buffer, completionCallbackType: .dataPlayedBack) { _ in
            Task { @MainActor in completion() }
        }
        if !player.isPlaying { player.play() }
    }
    public func stop() { player.stop(); engine.stop() }
}
#endif
