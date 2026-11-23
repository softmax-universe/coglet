import AppKit

/// Short synthesized cues; retained for playback, system volume/mute are respected.
/// The controller invokes start only after audio.start succeeds, never on a render.
@MainActor final class SoundCues {
    enum Cue { case start, submit, error }
    private var current: NSSound?
    func play(_ cue: Cue) {
        current?.stop()
        let duration = cue == .start ? 0.07 : cue == .submit ? 0.08 : 0.10
        let count = Int(44_100 * duration)
        var samples = Data()
        for i in 0..<count {
            let t = Double(i) / 44_100
            let fraction = Double(i) / Double(count)
            let frequency = cue == .start ? 420 + 320 * fraction : cue == .submit ? (fraction < 0.5 ? 520.0 : 380.0) : 220.0
            let envelope = sin(.pi * fraction)
            var sample = Int16(sin(2 * .pi * frequency * t) * envelope * 1800).littleEndian
            withUnsafeBytes(of: &sample) { samples.append(contentsOf: $0) }
        }
        var wav = Data("RIFF".utf8)
        func append<T: FixedWidthInteger>(_ number: T) { var n = number.littleEndian; withUnsafeBytes(of: &n) { wav.append(contentsOf: $0) } }
        append(UInt32(36 + samples.count)); wav.append(Data("WAVEfmt ".utf8))
        append(UInt32(16)); append(UInt16(1)); append(UInt16(1)); append(UInt32(44_100)); append(UInt32(88_200))
        append(UInt16(2)); append(UInt16(16)); wav.append(Data("data".utf8)); append(UInt32(samples.count)); wav.append(samples)
        current = NSSound(data: wav); current?.volume = 0.35; current?.play()
    }
}
