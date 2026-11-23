import Foundation
#if canImport(CoreGraphics)
import CoreGraphics
#endif

public final class OrbPreferences {
    private let defaults: UserDefaults
    public init(defaults: UserDefaults = .standard) { self.defaults = defaults }
    public var isEnabled: Bool {
        get { defaults.bool(forKey: "floatingOrbEnabled") }
        set { defaults.set(newValue, forKey: "floatingOrbEnabled") }
    }
}

/// Logical points in AppKit's bottom-left screen coordinate system.
public enum OrbGeometry {
    public static let size: CGFloat = 68
    public static let margin: CGFloat = 16

    public static func chatFrame(orb: CGRect, in screen: CGRect) -> CGRect {
        let width = min(408, screen.width)
        let height = min(548, screen.height)
        return CGRect(x: min(max(orb.maxX - width, screen.minX), screen.maxX - width),
                      y: min(max(orb.maxY + 12, screen.minY), screen.maxY - height),
                      width: width, height: height)
    }

    public static func snapped(_ origin: CGPoint, in visibleFrame: CGRect) -> CGPoint {
        let left = visibleFrame.minX + margin
        let right = max(left, visibleFrame.maxX - size - margin)
        let bottom = visibleFrame.minY + margin
        let top = max(bottom, visibleFrame.maxY - size - margin)
        return CGPoint(x: abs(origin.x - left) <= abs(origin.x - right) ? left : right,
                       y: min(max(origin.y, bottom), top))
    }
}
