#if canImport(AppKit)
import AppKit

/// Owns the real AppKit chrome registrations for ONE product window identity.
/// The presentation owner calls synchronize after show/hide and before capture.
@MainActor
public final class NativeWindowControlRegistration {
    private let registry: ControlRegistry
    private weak var window: NSWindow?
    private var route: String?
    private var id: String?
    private var buttons: [NativeButtonRegistration] = []

    public init(registry: ControlRegistry) { self.registry = registry }

    public func synchronize(window: NSWindow?, id: String, route: String?) throws {
        guard let window, let route, window.isVisible, !window.isMiniaturized else {
            try invalidate()
            return
        }
        if self.window === window, self.id == id, self.route == route,
           !buttons.isEmpty, buttons.allSatisfy({ $0.isActive }) { return }
        try invalidate()
        self.window = window
        self.route = route
        self.id = id
        do {
            for (kind, suffix) in [(NSWindow.ButtonType.closeButton, "close"),
                                   (.miniaturizeButton, "minimize"), (.zoomButton, "zoom")] {
                guard let button = window.standardWindowButton(kind) else { continue }
                buttons.append(try NativeButtonRegistration(button: button, registry: registry,
                    id: id + ".titlebar." + suffix, window: id, route: route,
                    preserveNativeAccessibilityIdentity: true))
            }
        } catch {
            // Never leave a partially registered presentation usable.
            try? invalidate()
            throw error
        }
    }

    public func invalidate() throws {
        for button in buttons { try button.invalidate() }
        buttons.removeAll()
        window = nil
        route = nil
        id = nil
    }
}
#endif
