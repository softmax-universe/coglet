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
    }
    public required init?(coder: NSCoder) {
        super.init(coder: coder)
        target = self
        action = #selector(press)
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
    public override func rightMouseDown(with event: NSEvent) { onVoice?() }
    public override func accessibilityPerformPress() -> Bool {
        guard isEnabled else { return false }
        performClick(nil)
        return true
    }
}
#endif
