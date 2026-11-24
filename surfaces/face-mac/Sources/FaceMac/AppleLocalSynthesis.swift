#if os(macOS)
import Foundation
@preconcurrency import AVFoundation

/// AVSpeechSynthesizer.write only produces PCM: NativeFaceDuplex remains the
/// sole speaker engine. No speak() call, audio session or second output graph.
@MainActor public final class AppleLocalSynthesis: FaceSynthesis {
    public let route = "local"
    public let usesRemoteAudio = false
    private let voice: AVSpeechSynthesisVoice
    private let synthesizer = AVSpeechSynthesizer()
    private var custody = LocalSpeechCustody()
    private var collector: AppleSpeechCollector?
    private var completion: (@MainActor (Result<FaceSynthesizedAudio, Error>) -> Void)?

    /// Choose an already installed OS voice explicitly; never request a download.
    public init(voiceIdentifier: String) throws {
        guard let voice = AVSpeechSynthesisVoice.speechVoices().first(where: { $0.identifier == voiceIdentifier }) else {
            throw LocalSpeechError.unavailable
        }
        self.voice = voice
    }

    public func synthesize(text: String, completion: @escaping @MainActor (Result<FaceSynthesizedAudio, Error>) -> Void) -> @MainActor () -> Void {
        cancelCurrent()
        guard !text.isEmpty, text.utf8.count <= 4096 else {
            completion(.failure(LocalSpeechError.resourceExhausted)); return {}
        }
        let token = custody.replace()
        self.completion = completion
        let collector = AppleSpeechCollector()
        self.collector = collector
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = voice
        let deliver: @MainActor @Sendable (Result<[Int16], LocalSpeechError>) -> Void = { [weak self] result in
            guard let self, self.custody.retire(token) else { return }
            let completion = self.completion
            self.completion = nil; self.collector = nil
            self.synthesizer.stopSpeaking(at: .immediate)
            completion?(result.map { FaceSynthesizedAudio(samples: $0, spans: []) }.mapError { $0 as Error })
        }
        synthesizer.write(utterance, toBufferCallback: AppleSpeechCollector.callback(collector, deliver: deliver))
        return { [weak self] in
            guard let self, self.custody.accepts(token) else { return }
            self.cancelCurrent()
        }
    }

    private func cancelCurrent() {
        if let token = custody.token { custody.retire(token) }
        collector?.cancel(); collector = nil
        let completion = self.completion; self.completion = nil
        synthesizer.stopSpeaking(at: .immediate)
        completion?(.failure(LocalSpeechError.cancelled))
    }
}

/// The write callback may run on any queue. All borrowed native data is consumed
/// synchronously under this lock; ONLY a bounded Sendable terminal value hops to
/// MainActor. In particular there is no unbounded Task-per-audio-buffer backlog.
final class AppleSpeechCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var active = true
    private var pcm = LocalSpeechPCM()
    func cancel() { lock.lock(); defer { lock.unlock() }; active = false; pcm = LocalSpeechPCM() }

    nonisolated static func callback(_ collector: AppleSpeechCollector,
        deliver: @escaping @MainActor @Sendable (Result<[Int16], LocalSpeechError>) -> Void
    ) -> @Sendable (AVAudioBuffer) -> Void {
        { @Sendable buffer in
            if let result = collector.receive(buffer) {
                Task { @MainActor in deliver(result) }
            }
        }
    }

    func receive(_ buffer: AVAudioBuffer) -> Result<[Int16], LocalSpeechError>? {
        lock.lock(); defer { lock.unlock() }
        guard active else { return nil }
        do {
            guard let buffer = buffer as? AVAudioPCMBuffer else { throw LocalSpeechError.invalidFormat }
            if buffer.frameLength == 0 {
                active = false
                let samples = pcm.samples; pcm = LocalSpeechPCM()
                guard !samples.isEmpty else { return .failure(.unavailable) }
                return .success(samples)
            }
            try pcm.append(mono: Self.copyMono(buffer), rate: buffer.format.sampleRate)
            return nil
        } catch {
            active = false; pcm = LocalSpeechPCM()
            return .failure((error as? LocalSpeechError) ?? .invalidFormat)
        }
    }

    /// Copy interleaved or planar native buffers before returning to AVFoundation.
    static func copyMono(_ buffer: AVAudioPCMBuffer) throws -> [Float] {
        let frames = Int(buffer.frameLength), channels = Int(buffer.format.channelCount)
        guard frames <= 65_536, (1...8).contains(channels), buffer.frameLength <= buffer.frameCapacity else {
            throw LocalSpeechError.invalidFormat
        }
        let interleaved = buffer.format.isInterleaved
        var mono = [Float](repeating: 0, count: frames)
        for frame in 0..<frames {
            for channel in 0..<channels {
                let plane = interleaved ? 0 : channel
                let offset = interleaved ? frame * channels + channel : frame
                let value: Float
                switch buffer.format.commonFormat {
                case .pcmFormatFloat32:
                    guard let data = buffer.floatChannelData else { throw LocalSpeechError.invalidFormat }
                    value = data[plane][offset]
                case .pcmFormatInt16:
                    guard let data = buffer.int16ChannelData else { throw LocalSpeechError.invalidFormat }
                    value = Float(data[plane][offset]) / 32768
                case .pcmFormatInt32:
                    guard let data = buffer.int32ChannelData else { throw LocalSpeechError.invalidFormat }
                    value = Float(data[plane][offset]) / 2147483648
                default: throw LocalSpeechError.invalidFormat
                }
                guard value.isFinite else { throw LocalSpeechError.invalidFormat }
                mono[frame] += value / Float(channels)
            }
        }
        return mono
    }
}
#endif
