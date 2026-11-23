import Foundation

public enum FaceState: Equatable, Sendable {
    case idle, listening(level: Double), thinking, speaking(level: Double)
    public var label: String {
        switch self {
        case .idle: "Idle"
        case .listening: "Listening"
        case .thinking: "Thinking"
        case .speaking: "Speaking"
        }
    }
}

/// Master-unit geometry from Design/Face; motion bounds from motion.json.
public struct FaceParameters: Equatable, Sendable {
    public var eyeHeight: Double = 80
    public var mouthHeight: Double = 16
    public var headScale: Double = 1
    public var happy: Double = 0
    public var concerned: Double = 0
    public init(state: FaceState, reduceMotion: Bool = false) {
        func level(_ value: Double) -> Double { value.isFinite ? min(1, max(0, value)) : 0 }
        switch state {
        case .idle: break
        case .listening(let input): headScale = reduceMotion ? 1 : 1 + 0.025 * level(input)
        case .thinking: eyeHeight = 26
        case .speaking(let output): mouthHeight = 16 + 72 * level(output)
        }
    }
}
