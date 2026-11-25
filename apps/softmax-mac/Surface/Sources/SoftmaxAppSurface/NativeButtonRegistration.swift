#if canImport(AppKit)
import AppKit

/// Registers an existing native button, not an accessibility-driven substitute.
/// User, accessibility and adapter paths all use its existing target/action.
/// A presentation owner retains this object and explicitly invalidates it on
/// orderOut/route replacement; native close/minimize retire it synchronously.
@MainActor
public final class NativeButtonRegistration: NSObject {
    private let registry: ControlRegistry
    private var mount: ControlRegistry.Mount?

    public init(button: NSButton, registry: ControlRegistry, id: String, window: String,
                route: String, sensitive: Bool = false) throws {
        guard let native = button.window else { throw ControlRegistryFailure.unavailable }
        self.registry = registry
        super.init()
        mount = try registry.mount(id: id, window: window, route: route, role: "button",
            label: button.accessibilityLabel() ?? button.title, sensitive: sensitive,
            verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
            enabled: { [weak button, weak native] in
                guard let button, let native else { return false }
                return button.window === native && button.isEnabled
            }, visible: { [weak button, weak native] in
                guard let button, let native else { return false }
                return button.window === native && native.isVisible && !native.isMiniaturized
                    && !button.isHiddenOrHasHiddenAncestor
            }, invoke: { [weak button, weak native] _ in
                guard let button, let native, button.window === native else {
                    throw ControlRegistryFailure.unavailable
                }
                button.performClick(nil)
            })
        button.setAccessibilityIdentifier(id)
        NotificationCenter.default.addObserver(self, selector: #selector(retirePresentation(_:)),
            name: NSWindow.willCloseNotification, object: native)
        NotificationCenter.default.addObserver(self, selector: #selector(retirePresentation(_:)),
            name: NSWindow.willMiniaturizeNotification, object: native)
    }

    public func invalidate() throws {
        guard let mount else { return }
        try registry.unmount(mount)
        self.mount = nil
        NotificationCenter.default.removeObserver(self)
    }

    @objc private func retirePresentation(_ notification: Notification) {
        // AppKit posts window lifecycle on its native owner. No actor hop may
        // leave a queued old request usable after a close/reopen presentation.
        do { try invalidate() }
        catch { /* Revision exhaustion already permanently fences registry effects. */ }
    }
}
#endif
