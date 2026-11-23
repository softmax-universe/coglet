import Foundation

public enum FaceExpression: String, CaseIterable, Sendable {
    case neutral, listening, thinking, speaking, happy, concerned
}
public enum FaceError: Error { case invalidArgument }

/// Local presentation seam, not an action-admission/grant authority.
@MainActor public protocol FaceDriver: AnyObject {
    var state: FaceState { get }
    var expression: FaceExpression { get }
    var onChange: (() -> Void)? { get set }
    func setState(_ state: FaceState)
    func setExpression(_ name: String, intensity: Int, durationMS: Int) throws
    func parameters(reduceMotion: Bool) -> FaceParameters
    func tick()
    func reset()
}

@MainActor public final class LocalFaceDriver: FaceDriver {
    public private(set) var state: FaceState = .idle
    public private(set) var expression: FaceExpression = .neutral
    public var onChange: (() -> Void)?
    private var intensity: Double = 0
    private var expiry: TimeInterval?
    private let now: () -> TimeInterval
    public init(now: @escaping () -> TimeInterval = { ProcessInfo.processInfo.systemUptime }) { self.now = now }
    public func setState(_ state: FaceState) { self.state = state; onChange?() }
    public func setExpression(_ name: String, intensity: Int, durationMS: Int) throws {
        guard let expression = FaceExpression(rawValue: name), (0...1000).contains(intensity),
              (0...60000).contains(durationMS) else { throw FaceError.invalidArgument }
        self.expression = durationMS == 0 ? .neutral : expression
        self.intensity = durationMS == 0 ? 0 : Double(intensity) / 1000
        expiry = durationMS == 0 ? nil : now() + Double(durationMS) / 1000
        onChange?()
    }
    public func tick() {
        if let expiry, now() >= expiry {
            self.expiry = nil; expression = .neutral; intensity = 0; onChange?()
        }
    }
    public func reset() { expiry = nil; expression = .neutral; intensity = 0; state = .idle; onChange?() }
    public func parameters(reduceMotion: Bool) -> FaceParameters {
        tick()
        var result = FaceParameters(state: state, reduceMotion: reduceMotion)
        switch expression {
        case .neutral: break
        case .listening: result.headScale = reduceMotion ? 1 : 1 + 0.025 * intensity
        case .thinking: result.eyeHeight += (26 - result.eyeHeight) * intensity
        case .speaking:
            // Semantic speaking may open a quiet mouth, but never invents playback level.
            result.mouthHeight = max(result.mouthHeight, 16 + 24 * intensity)
        case .happy: result.happy = intensity
        case .concerned: result.concerned = intensity
        }
        return result
    }
}

/// Explicitly simulated, deterministic loop. Does not request audio permissions.
@MainActor public final class DemoFaceDriver {
    public let driver: any FaceDriver
    public init(driver: any FaceDriver) { self.driver = driver }
    public func step(seconds: Double) {
        let phase = Int(max(0, seconds).truncatingRemainder(dividingBy: 12)) / 3
        let envelope = (sin(seconds * 5) + 1) / 2
        switch phase {
        case 0: driver.setState(.idle)
        case 1: driver.setState(.listening(level: envelope))
        case 2: driver.setState(.thinking)
        default: driver.setState(.speaking(level: envelope))
        }
    }
}
