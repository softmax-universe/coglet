import Foundation

/// Route failures intentionally carry no transcript, PCM or native error detail.
public enum LocalSpeechError: Error, Sendable {
    case permissionDenied, unavailable, invalidFormat, resourceExhausted, cancelled
}

/// Streaming, bounded conversion seam. Linear interpolation preserves phase across
/// native callback boundaries. The route deliberately supplies no guessed alignment.
struct LocalSpeechPCM {
    static let rate = 24_000.0
    static let maximumFrames = 24_000 * 30
    private var sourceRate: Double?
    private var sourceFrames = 0
    private var nextPosition = 0.0
    private var previous: Float?
    private(set) var samples: [Int16] = []
    let limit: Int
    init(limit: Int = maximumFrames) { self.limit = max(0, min(limit, Self.maximumFrames)) }

    static func quantize(_ value: Float) throws -> Int16 {
        guard value.isFinite else { throw LocalSpeechError.invalidFormat }
        if value >= 1 { return .max }
        if value <= -1 { return .min }
        return Int16(clamping: Int((value * 32768).rounded()))
    }

    mutating func append(mono: [Float], rate: Double) throws {
        guard rate.isFinite, (8_000...192_000).contains(rate), mono.count <= 65_536,
              sourceRate == nil || sourceRate == rate, mono.allSatisfy({ $0.isFinite }) else {
            throw LocalSpeechError.invalidFormat
        }
        sourceRate = rate
        let step = rate / Self.rate
        for value in mono {
            let index = Double(sourceFrames)
            // Need both surrounding samples before interpolating; the final tail
            // is conservatively omitted rather than invented or padded.
            while nextPosition <= index {
                let interpolated: Float
                if nextPosition == index { interpolated = value }
                else if let previous {
                    let fraction = Float(nextPosition - (index - 1))
                    interpolated = previous + (value - previous) * fraction
                } else { break }
                guard samples.count < limit else { throw LocalSpeechError.resourceExhausted }
                samples.append(try Self.quantize(interpolated))
                nextPosition += step
            }
            previous = value; sourceFrames += 1
        }
    }
}

/// A fresh token on every replacement; stale cancellation cannot retire a new job.
struct LocalSpeechCustody {
    private(set) var token: UUID?
    mutating func replace() -> UUID { let value = UUID(); token = value; return value }
    func accepts(_ value: UUID) -> Bool { token == value }
    @discardableResult mutating func retire(_ value: UUID) -> Bool {
        guard accepts(value) else { return false }; token = nil; return true
    }
}
