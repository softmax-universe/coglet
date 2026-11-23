import AVFoundation
import CAudioRing

public enum FaceAudioError: Error {
    case allocation, invalidFormat, conversion, permissionDenied, aecUnavailable
    case deviceUnavailable(String), coreAudio(OSStatus), queueFull
}

// Sole producer/consumer ownership is enforced by the owning graph. Atomic storage
// also makes a control-thread discard safe while the render consumer is in flight.
final class PCMQueue: @unchecked Sendable {
    let raw: OpaquePointer
    init(capacity: Int) throws {
        guard let raw = fa_create(capacity) else { throw FaceAudioError.allocation }
        self.raw = raw
    }
    deinit { fa_destroy(raw) }
    func write(_ samples: UnsafeBufferPointer<Int16>) -> Bool {
        fa_write(raw, samples.baseAddress, samples.count)
    }
    func flush() { fa_flush(raw) }
}

/// Stateful resampler. Use on one capture callback thread only.
final class CaptureConverter {
    let converter: AVAudioConverter
    let output: AVAudioPCMBuffer
    init(format: AVAudioFormat, maximumFrames: AVAudioFrameCount = 16384) throws {
        guard format.sampleRate > 0, format.channelCount > 0,
              let pcm = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 24000,
                                      channels: 1, interleaved: false),
              let converter = AVAudioConverter(from: format, to: pcm),
              let output = AVAudioPCMBuffer(pcmFormat: pcm, frameCapacity:
                AVAudioFrameCount(ceil(Double(maximumFrames) * 24000 / format.sampleRate)) + 256)
        else { throw FaceAudioError.invalidFormat }
        converter.primeMethod = .none
        self.converter = converter
        self.output = output
    }
    func convert(_ input: AVAudioPCMBuffer) throws -> UnsafeBufferPointer<Int16> {
        guard input.frameLength <= 16384 else { throw FaceAudioError.invalidFormat }
        var supplied = false
        var error: NSError?
        output.frameLength = 0
        let status = converter.convert(to: output, error: &error) { _, outStatus in
            if supplied { outStatus.pointee = .noDataNow; return nil }
            supplied = true
            outStatus.pointee = .haveData
            return input
        }
        guard status != .error, error == nil, let data = output.int16ChannelData else {
            throw FaceAudioError.conversion
        }
        return UnsafeBufferPointer(start: data[0], count: Int(output.frameLength))
    }
}

/// Energy gate only, not a speech classifier. Frame-based hangover is independent
/// of the caller's polling frequency; consume every captured frame exactly once.
public struct EnergyVAD {
    public var threshold: Float = 0.02
    public var hangoverFrames = 4800
    private var remaining = 0
    public init() {}
    public mutating func process(_ samples: [Int16]) -> Bool {
        guard !samples.isEmpty else { return remaining > 0 }
        let power = samples.reduce(0.0) { $0 + pow(Double($1) / 32768, 2) }
        if sqrt(power / Double(samples.count)) >= Double(threshold) {
            remaining = hangoverFrames
            return true
        }
        remaining = max(0, remaining - samples.count)
        return remaining > 0
    }
}
