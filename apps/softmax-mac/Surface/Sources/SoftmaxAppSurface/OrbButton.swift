#if canImport(AppKit)
import AppKit

/// The product orb's actual native button. Mouse, accessibility and adapter
/// presses all enter the same target/action; no parallel automation actuator.
@MainActor
public final class OrbButton: NSButton {
    public var onClick: (() -> Void)?
    public var onVoice: (() -> Void)?
    public var onDrop: (() -> Void)?
    private var dragStart: CGPoint?
    private var windowStart: CGPoint?
    private var moved = false

    public override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        target = self
        action = #selector(press)
        installVoiceAccessibilityAction()
    }
    public required init?(coder: NSCoder) {
        super.init(coder: coder)
        target = self
        action = #selector(press)
        installVoiceAccessibilityAction()
    }
    @objc private func press() { onClick?() }
    public override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }
    public override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        dragStart = window.convertPoint(toScreen: event.locationInWindow)
        windowStart = window.frame.origin
        moved = false
    }
    public override func mouseDragged(with event: NSEvent) {
        guard let window, let dragStart, let windowStart else { return }
        let pointer = window.convertPoint(toScreen: event.locationInWindow)
        let delta = CGPoint(x: pointer.x - dragStart.x, y: pointer.y - dragStart.y)
        if hypot(delta.x, delta.y) > 4 { moved = true }
        if moved { window.setFrameOrigin(CGPoint(x: windowStart.x + delta.x, y: windowStart.y + delta.y)) }
    }
    public override func mouseUp(with event: NSEvent) {
        guard dragStart != nil else { return }
        dragStart = nil
        windowStart = nil
        if moved { onDrop?() } else { performClick(nil) }
    }
    private func installVoiceAccessibilityAction() {
        setAccessibilityCustomActions([NSAccessibilityCustomAction(name: "Open Voice", target: self,
                                                                  selector: #selector(performVoice))])
    }
    /// One native secondary command, shared by right-click, AX and adapter.
    @objc public func performVoice() -> Bool {
        guard isEnabled, let window, window.isVisible, !window.isMiniaturized,
              !isHiddenOrHasHiddenAncestor, let onVoice else { return false }
        onVoice()
        return true
    }
    public override func rightMouseDown(with event: NSEvent) { _ = performVoice() }
    public override func accessibilityPerformPress() -> Bool {
        guard isEnabled else { return false }
        performClick(nil)
        return true
    }
}
/// Lifetime owner for the actual orb's secondary gesture; it is not a synthetic
/// button and cannot borrow a replacement panel's visibility or callback.
@MainActor
public final class OrbVoiceRegistration: NSObject {
    private let registry: ControlRegistry
    private var mount: ControlRegistry.Mount?
    public init(button: OrbButton, registry: ControlRegistry) throws {
        guard let panel = button.window else { throw ControlRegistryFailure.unavailable }
        self.registry = registry
        super.init()
        mount = try registry.mount(id: "orb.open_voice", window: "orb", route: "orb", role: "gesture",
            label: "Open Voice", verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
            enabled: { [weak button, weak panel] in button?.window === panel && button?.isEnabled == true },
            visible: { [weak button, weak panel] in
                guard let button, let panel else { return false }
                return button.window === panel && panel.isVisible && !panel.isMiniaturized && !button.isHiddenOrHasHiddenAncestor
            }, invoke: { [weak button, weak panel] _ in
                guard let button, let panel, button.window === panel, button.performVoice() else {
                    throw ControlRegistryFailure.unavailable
                }
            })
        NotificationCenter.default.addObserver(self, selector: #selector(retire(_:)), name: NSWindow.willCloseNotification, object: panel)
        NotificationCenter.default.addObserver(self, selector: #selector(retire(_:)), name: NSWindow.willMiniaturizeNotification, object: panel)
    }
    public var isActive: Bool { mount != nil }
    public func invalidate() throws {
        guard let mount else { return }
        try registry.unmount(mount)
        self.mount = nil
        NotificationCenter.default.removeObserver(self)
    }
    @objc private func retire(_ notification: Notification) { try? invalidate() }
}
#endif
