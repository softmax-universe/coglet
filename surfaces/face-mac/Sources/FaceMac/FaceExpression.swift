import Foundation
import CogletTypes
import CogletSurface

public struct FaceExpression: Equatable, Sendable {
    public let expression: String
    public let intensity: UInt64
    public let remainingMS: UInt64
}

/// Semantic renderer shared by the standalone host and embedding apps. The host
/// ticks this on its native UI timer, independently of guest progress.
@MainActor public final class FaceExpressionRenderer {
    private var name = "neutral"
    private var intensity: UInt64 = 0
    private var expires: UInt64 = 0
    private let paint: (FaceExpression) -> Void
    public init(paint: @escaping (FaceExpression) -> Void) { self.paint = paint }
    public func set(expression: String, intensity: UInt64, durationMS: UInt64, now: UInt64) throws {
        guard ["neutral", "listening", "thinking", "speaking", "happy", "concerned"].contains(expression),
              intensity <= 1000, durationMS <= 60_000,
              now <= UInt64.max - durationMS * 1000 else { throw NativeSurfaceError("invalid_argument") }
        if durationMS == 0 { reset(); return }
        self.name = expression; self.intensity = intensity; expires = now + durationMS * 1000
        paint(snapshot(now: now))
    }
    public func tick(now: UInt64) { if expires > 0 && now >= expires { reset() } }
    public func snapshot(now: UInt64) -> FaceExpression {
        guard now < expires else { return .init(expression: "neutral", intensity: 0, remainingMS: 0) }
        return .init(expression: name, intensity: intensity, remainingMS: (expires - now + 999) / 1000)
    }
    public func reset() {
        name = "neutral"; intensity = 0; expires = 0
        paint(.init(expression: "neutral", intensity: 0, remainingMS: 0))
    }
}
