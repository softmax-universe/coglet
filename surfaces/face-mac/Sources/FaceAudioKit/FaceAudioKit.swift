import AVFoundation
import CAudioRing

public struct AudioConfiguration: Equatable, Sendable {
    public var inputUID: String?
    public var outputUID: String?
    public var echoCancellation = true
    public init(inputUID: String? = nil, outputUID: String? = nil, echoCancellation: Bool = true) {
        self.inputUID = inputUID; self.outputUID = outputUID
        self.echoCancellation = echoCancellation
    }
}

@MainActor protocol DuplexEngine: AnyObject {
    func start(configuration: AudioConfiguration, capture: PCMQueue, playback: PCMQueue,
               changed: @escaping @MainActor @Sendable () -> Void) throws
    func stop()
}

/// Public calls are serialized on the main actor; audio callbacks never enter it.
/// Enqueue/capture PCM is always mono signed PCM16 at 24 kHz.
@MainActor public final class FaceAudioKit {
    public private(set) var configuration: AudioConfiguration
    public private(set) var isRunning = false
    public private(set) var graphGeneration: UInt64 = 0
    public private(set) var lastError: Error?
    /// Invoked on the main actor when readCapture detects a voice-state change.
    public var onVoiceActivity: ((Bool) -> Void)?
    public var duckingGain: Float = 0.25
    public var duckingEnabled = false
    private var vad = EnergyVAD()
    private var speaking = false
    private var capture: PCMQueue?
    private var playback: PCMQueue?
    private var engine: (any DuplexEngine)?
    private var completedFrames: UInt64 = 0
    private let factory: () -> any DuplexEngine
    private let permission: () -> Bool

    public convenience init(configuration: AudioConfiguration = .init()) {
        self.init(configuration: configuration, factory: { AVDuplexEngine() },
                  permission: { MicrophonePermission.status == .authorized })
    }
    init(configuration: AudioConfiguration = .init(), factory: @escaping () -> any DuplexEngine,
         permission: @escaping () -> Bool) {
        self.configuration = configuration; self.factory = factory; self.permission = permission
    }
    public func startCapture() throws {
        guard !isRunning else { return }
        guard permission() else { throw FaceAudioError.permissionDenied }
        let input = try PCMQueue(capacity: 24000 * 4)
        let output = try PCMQueue(capacity: 24000 * 30)
        let graph = factory()
        graphGeneration &+= 1
        let generation = graphGeneration
        do {
            try graph.start(configuration: configuration, capture: input, playback: output) { [weak self] in
                guard let self, self.isRunning, generation == self.graphGeneration else { return }
                self.stopCapture()
                do { try self.startCapture() } catch { self.lastError = error }
            }
            capture = input; playback = output; engine = graph
            isRunning = true; lastError = nil
        } catch { graph.stop(); lastError = error; throw error }
    }
    /// Stops both I/O directions and discards unheard playback. No microphone remains active.
    public func stopCapture() {
        graphGeneration &+= 1
        playback?.flush()
        engine?.stop()
        if let playback { completedFrames += fa_played(playback.raw) }
        engine = nil; capture = nil; playback = nil; isRunning = false
        vad = EnergyVAD()
        if speaking { speaking = false; onVoiceActivity?(false) }
    }
    /// Changing a route/AEC setting rebuilds the graph and drops queued audio.
    public func selectDevices(_ configuration: AudioConfiguration) throws {
        let restart = isRunning
        stopCapture(); self.configuration = configuration
        if restart { try startCapture() }
    }
    public func enqueuePlayback(_ samples: [Int16]) throws {
        guard let playback else { throw FaceAudioError.invalidFormat }
        guard samples.withUnsafeBufferPointer({ playback.write($0) }) else { throw FaceAudioError.queueFull }
    }
    public func flushPlayback() { playback?.flush() }
    /// Frames actually handed to the output graph (silence/underruns excluded),
    /// not a hardware acoustic timestamp. Monotonic over graph rebuilds/flushes.
    public var playbackPosition: TimeInterval {
        Double(completedFrames + (playback.map { fa_played($0.raw) } ?? 0)) / 24000
    }
    public var inputPeak: Float { capture.map { fa_peak($0.raw) } ?? 0 }
    public var outputPeak: Float { playback.map { fa_peak($0.raw) } ?? 0 }
    public var captureFailed: Bool { capture.map { fa_failed($0.raw) } ?? false }
    public func readCapture(maximumFrames: Int = 2400) -> [Int16] {
        guard let capture, maximumFrames > 0 else { return [] }
        var samples = [Int16](repeating: 0, count: min(maximumFrames, 96000))
        let n = samples.withUnsafeMutableBufferPointer { fa_read(capture.raw, $0.baseAddress, $0.count) }
        samples.removeLast(samples.count - n)
        let active = vad.process(samples)
        if let playback { fa_gain(playback.raw, duckingEnabled && active ? duckingGain : 1) }
        if active != speaking { speaking = active; onVoiceActivity?(active) }
        return samples
    }
}

// Each graph owns its queues/converter. Old in-flight callbacks can never write
// into a replacement graph, even after an asynchronous configuration notification.
private final class CapturePath: @unchecked Sendable {
    let queue: PCMQueue
    let converter: CaptureConverter
    init(queue: PCMQueue, converter: CaptureConverter) { self.queue = queue; self.converter = converter }
    func receive(_ buffer: AVAudioPCMBuffer) {
        do {
            let samples = try converter.convert(buffer)
            fa_measure(queue.raw, samples.baseAddress, samples.count)
            if !queue.write(samples) { fa_fail(queue.raw) }
        } catch { fa_fail(queue.raw) }
    }
}

// RAII owns hardware/observer cleanup even if the caller releases the kit
// without explicitly stopping. Queue/converter captures outlive in-flight calls.
private final class GraphLifetime: @unchecked Sendable {
    let engine = AVAudioEngine()
    var observer: NSObjectProtocol?
    var tapped = false
    func stop() {
        if let observer { NotificationCenter.default.removeObserver(observer); self.observer = nil }
        engine.stop()
        if tapped { engine.inputNode.removeTap(onBus: 0); tapped = false }
        engine.reset()
    }
    deinit { stop() }
}

@MainActor private final class AVDuplexEngine: DuplexEngine {
    private let lifetime = GraphLifetime()
    private var engine: AVAudioEngine { lifetime.engine }
    func start(configuration: AudioConfiguration, capture: PCMQueue, playback: PCMQueue,
               changed: @escaping @MainActor @Sendable () -> Void) throws {
        try AudioDevices.select(configuration.inputUID, input: true, engine: engine)
        try AudioDevices.select(configuration.outputUID, input: false, engine: engine)
        let input = engine.inputNode
        try input.setVoiceProcessingEnabled(configuration.echoCancellation)
        if configuration.echoCancellation && (!input.isVoiceProcessingEnabled || !engine.outputNode.isVoiceProcessingEnabled) {
            throw FaceAudioError.aecUnavailable
        }
        let deviceFormat = input.outputFormat(forBus: 0)
        let path = CapturePath(queue: capture, converter: try CaptureConverter(format: deviceFormat))
        guard let speechFormat = AVAudioFormat(standardFormatWithSampleRate: 24000, channels: 1) else {
            throw FaceAudioError.invalidFormat
        }
        let source = AVAudioSourceNode(format: speechFormat) { silence, _, frames, buffers in
            let list = UnsafeMutableAudioBufferListPointer(buffers)
            guard let data = list[0].mData else { return noErr }
            let n = fa_render(playback.raw, data.assumingMemoryBound(to: Float.self), Int(frames), 1)
            silence.pointee = ObjCBool(n == 0)
            return noErr
        }
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: speechFormat)
        if configuration.echoCancellation {
            engine.connect(engine.mainMixerNode, to: engine.outputNode, format: deviceFormat)
            guard deviceFormat == engine.outputNode.inputFormat(forBus: 0) else { throw FaceAudioError.aecUnavailable }
        }
        input.installTap(onBus: 0, bufferSize: 1024, format: deviceFormat) { buffer, _ in path.receive(buffer) }
        lifetime.tapped = true
        engine.prepare()
        try engine.start()
        lifetime.observer = NotificationCenter.default.addObserver(forName: .AVAudioEngineConfigurationChange,
                                                          object: engine, queue: nil) { _ in
            Task { @MainActor in changed() }
        }
    }
    func stop() { lifetime.stop() }
}
