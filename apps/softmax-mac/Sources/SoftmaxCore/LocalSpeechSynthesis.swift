import Foundation
import FaceMac

/// Product-local text-to-PCM generation. No playback or remote fallback.
@MainActor
public protocol LocalSpeechSynthesis: AnyObject {
    func synthesize(_ text: String,
                    completion: @escaping @MainActor (Result<[PCMFrame], Error>) -> Void) -> (() -> Void)
}

public enum LocalSpeechSynthesisError: Error, Equatable, Sendable {
    case unavailable
    case invalidText
    case invalidPCM
    case emptyPCM
    case limitExceeded
    case cancelled
}

#if os(macOS)
import AVFoundation

/// Uses an explicitly selected, already-installed English system voice.
@MainActor
public final class AppleSpeechSynthesis: LocalSpeechSynthesis {
    private final class Request {
        let token = UUID()
        let synthesizer = AVSpeechSynthesizer()
        let collector = SpeechPCMCollector(timestamp: ProcessInfo.processInfo.systemUptime)
        let completion: @MainActor (Result<[PCMFrame], Error>) -> Void

        init(completion: @escaping @MainActor (Result<[PCMFrame], Error>) -> Void) {
            self.completion = completion
        }
    }

    private var current: Request?

    public init() {}

    public func synthesize(
        _ text: String,
        completion: @escaping @MainActor (Result<[PCMFrame], Error>) -> Void
    ) -> (() -> Void) {
        let request = Request(completion: completion)
        let previous = current
        // Reserve ownership before invoking a possibly reentrant old completion.
        current = request
        if let previous {
            previous.collector.cancel()
            previous.synthesizer.stopSpeaking(at: .immediate)
            previous.completion(.failure(LocalSpeechSynthesisError.cancelled))
        }
        let token = request.token
        let cancel: () -> Void = { [weak self] in
            // The protocol's cancellation closure is used by its MainActor owner.
            MainActor.assumeIsolated { self?.cancel(token: token) }
        }
        guard current === request else { return cancel }
        guard text.utf8.count <= 4096,
              !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            finish(token: token, result: .failure(LocalSpeechSynthesisError.invalidText))
            return cancel
        }
        // speechVoices enumerates installed voices; never ask the OS to resolve
        // an unspecified/default language voice or download one implicitly.
        guard let voice = AVSpeechSynthesisVoice.speechVoices()
            .filter({ $0.language.lowercased() == "en" || $0.language.lowercased().hasPrefix("en-") })
            .sorted(by: { $0.identifier < $1.identifier }).first else {
            finish(token: token, result: .failure(LocalSpeechSynthesisError.unavailable))
            return cancel
        }
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = voice
        let collector = request.collector
        request.synthesizer.write(utterance) { [weak self] buffer in
            // Everything borrowed from AVFoundation is consumed under the lock,
            // on this callback's stack. Only the terminal owned result hops.
            guard let result = collector.consume(buffer) else { return }
            Task { @MainActor [weak self] in
                self?.finish(token: token, result: result)
            }
        }
        return cancel
    }

    private func cancel(token: UUID) {
        guard let request = current, request.token == token else { return }
        current = nil
        request.collector.cancel()
        request.synthesizer.stopSpeaking(at: .immediate)
        request.completion(.failure(LocalSpeechSynthesisError.cancelled))
    }

    private func finish(token: UUID, result: Result<[PCMFrame], Error>) {
        guard let request = current, request.token == token else { return }
        current = nil
        request.collector.cancel()
        // Also stop native production on an early validation/limit failure.
        request.synthesizer.stopSpeaking(at: .immediate)
        request.completion(result)
    }
}

/// Lock-owned storage is the only mutable state touched by native callbacks.
/// At most 4,096 callbacks are admitted, including the end marker; after sealing,
/// subsequent callbacks allocate nothing and cannot schedule another actor hop.
final class SpeechPCMCollector: @unchecked Sendable {
    private let lock = NSLock()
    private var sealed = false
    private var callbacks = 0
    private var sampleCount = 0
    private var sampleRate: Double?
    private var frames: [PCMFrame] = []
    private let timestamp: TimeInterval
    private static let sampleLimit = 1_920_000

    init(timestamp: TimeInterval) { self.timestamp = timestamp }

    func cancel() {
        lock.lock()
        defer { lock.unlock() }
        sealed = true
        frames.removeAll(keepingCapacity: false)
    }

    func consume(_ buffer: AVAudioBuffer) -> Result<[PCMFrame], Error>? {
        lock.lock()
        defer { lock.unlock() }
        guard !sealed else { return nil }
        callbacks += 1
        guard callbacks <= 4_096 else { return fail(.limitExceeded) }
        guard let pcm = buffer as? AVAudioPCMBuffer else { return fail(.invalidPCM) }
        let count = Int(pcm.frameLength)
        guard count <= 65_536 else { return fail(.limitExceeded) }
        if count == 0 {
            guard sampleCount > 0 else { return fail(.emptyPCM) }
            sealed = true
            let output = frames
            frames = []
            return .success(output)
        }
        let format = pcm.format
        let rate = format.sampleRate
        guard rate.isFinite, rate >= 8000, rate <= 48_000,
              format.channelCount == 1,
              format.streamDescription.pointee.mFormatID == kAudioFormatLinearPCM,
              sampleRate == nil || sampleRate == rate else { return fail(.invalidPCM) }
        guard count <= Self.sampleLimit - sampleCount else { return fail(.limitExceeded) }
        let bytesPerSample: Int
        switch format.commonFormat {
        case .pcmFormatFloat32, .pcmFormatInt32: bytesPerSample = 4
        case .pcmFormatInt16: bytesPerSample = 2
        default: return fail(.invalidPCM)
        }
        let description = format.streamDescription.pointee
        guard description.mBytesPerFrame == UInt32(bytesPerSample),
              description.mBitsPerChannel == UInt32(bytesPerSample * 8),
              description.mFormatFlags & kAudioFormatFlagIsBigEndian == 0 else {
            return fail(.invalidPCM)
        }
        // Mono has exactly one data buffer for both interleaved and planar PCM.
        let buffers = UnsafeMutableAudioBufferListPointer(pcm.mutableAudioBufferList)
        guard buffers.count == 1, buffers[0].mNumberChannels == 1,
              Int(buffers[0].mDataByteSize) >= count * bytesPerSample,
              let data = buffers[0].mData else { return fail(.invalidPCM) }
        sampleRate = rate
        let chunkLimit = Int(rate.rounded(.down))
        var offset = 0
        while offset < count {
            // Bound metadata too, including pathological sub-audio sample rates.
            guard frames.count < 8_192 else { return fail(.limitExceeded) }
            let length = min(chunkLimit, count - offset)
            var samples: [Float] = []
            samples.reserveCapacity(length)
            for index in offset..<(offset + length) {
                let address = data.advanced(by: index * bytesPerSample)
                let value: Float
                switch format.commonFormat {
                case .pcmFormatFloat32: value = address.loadUnaligned(as: Float.self)
                case .pcmFormatInt16: value = Float(address.loadUnaligned(as: Int16.self)) / 32_768
                case .pcmFormatInt32: value = Float(address.loadUnaligned(as: Int32.self)) / 2_147_483_648
                default: return fail(.invalidPCM)
                }
                guard value.isFinite else { return fail(.invalidPCM) }
                samples.append(value)
            }
            frames.append(PCMFrame(samples: samples, sampleRate: rate,
                                   timestamp: timestamp + Double(sampleCount + offset) / rate))
            offset += length
        }
        sampleCount += count
        return nil
    }

    private func fail(_ error: LocalSpeechSynthesisError) -> Result<[PCMFrame], Error> {
        sealed = true
        frames.removeAll(keepingCapacity: false)
        return .failure(error)
    }
}
#endif
