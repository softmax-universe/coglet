#if canImport(AppKit)
import AppKit
import XCTest
@testable import SoftmaxAppSurface

@MainActor
final class NativeButtonRegistrationTests: XCTestCase {
    private final class Receiver: NSObject {
        var calls = 0
        @objc func press(_ sender: Any?) { calls += 1 }
    }
    private func args(_ id: String, window: String = "settings") -> CBORValue {
        .map([.init("window", .text(window)), .init("control", .text(id)), .init("verb", .text("press"))])
    }
    func testOrbUserAccessibilityAndAdapterShareActualButtonCommand() throws {
        _ = NSApplication.shared
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 68, height: 68),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let button = OrbButton(frame: NSRect(x: 0, y: 0, width: 68, height: 68))
        var calls = 0
        button.onClick = { calls += 1 }
        button.setAccessibilityLabel("Open Softmax chat")
        panel.contentView = button
        panel.orderFrontRegardless()
        defer { panel.close() }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "orb" })
        let registration = try NativeButtonRegistration(button: button, registry: registry,
            id: "orb.open_chat", window: "orb", route: "orb")
        let old = try registry.prepare(args("orb.open_chat", window: "orb"), sensitive: false)
        button.performClick(nil)
        XCTAssertTrue(button.accessibilityPerformPress())
        try registry.perform(old, authorized: { true })
        XCTAssertEqual(calls, 3)
        button.isEnabled = false
        XCTAssertFalse(button.accessibilityPerformPress())
        XCTAssertThrowsError(try registry.perform(old, authorized: { true }))
        button.isEnabled = true
        try registration.invalidate()
        panel.orderOut(nil)
        panel.orderFrontRegardless()
        let successor = try NativeButtonRegistration(button: button, registry: registry,
            id: "orb.open_chat", window: "orb", route: "orb")
        defer { try? successor.invalidate() }
        XCTAssertThrowsError(try registry.perform(old, authorized: { true }))
        XCTAssertEqual(calls, 3)
        try registry.perform(registry.prepare(args("orb.open_chat", window: "orb"), sensitive: false), authorized: { true })
        XCTAssertEqual(calls, 4)
    }

    func testOrbSecondaryMouseAccessibilityAndAdapterShareNativeCommand() throws {
        _ = NSApplication.shared
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 68, height: 68),
            styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        panel.isReleasedWhenClosed = false
        let button = OrbButton(frame: NSRect(x: 0, y: 0, width: 68, height: 68))
        var calls = 0
        button.onVoice = { calls += 1 }
        panel.contentView = button; panel.orderFrontRegardless()
        defer { panel.close() }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "orb" })
        let owner = try OrbVoiceRegistration(button: button, registry: registry)
        let request = try registry.prepare(args("orb.open_voice", window: "orb"), sensitive: false)
        let event = try XCTUnwrap(NSEvent.mouseEvent(with: .rightMouseDown, location: .zero,
            modifierFlags: [], timestamp: 0, windowNumber: panel.windowNumber, context: nil,
            eventNumber: 1, clickCount: 1, pressure: 1))
        button.rightMouseDown(with: event)
        let action = try XCTUnwrap(button.accessibilityCustomActions()?.first)
        XCTAssertEqual(action.name, "Open Voice")
        XCTAssertTrue(action.target === button)
        let selector = try XCTUnwrap(action.selector)
        XCTAssertEqual(selector, #selector(OrbButton.performVoice))
        // Exercise the ACTUAL AX target/BOOL selector, not a shadow callback.
        typealias AXCommand = @convention(c) (AnyObject, Selector) -> Bool
        let invokeAX = unsafeBitCast(button.method(for: selector), to: AXCommand.self)
        XCTAssertTrue(invokeAX(button, selector))
        try registry.perform(request, authorized: { true })
        XCTAssertEqual(calls, 3)
        button.isEnabled = false
        button.rightMouseDown(with: event)
        XCTAssertFalse(invokeAX(button, selector))
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        XCTAssertEqual(calls, 3)
        button.isEnabled = true
        try owner.invalidate(); panel.orderOut(nil); panel.orderFrontRegardless()
        let successor = try OrbVoiceRegistration(button: button, registry: registry)
        defer { try? successor.invalidate() }
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        try registry.perform(registry.prepare(args("orb.open_voice", window: "orb"), sensitive: false), authorized: { true })
        XCTAssertEqual(calls, 4)
    }

    func testNativePressUsesExistingTargetActionAndRefusesHiddenDisabledControls() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 160),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let receiver = Receiver()
        let button = NSButton(title: "Native command", target: receiver, action: #selector(Receiver.press(_:)))
        window.contentView = button
        window.orderFront(nil)
        defer { window.close() }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let registration = try NativeButtonRegistration(button: button, registry: registry,
            id: "settings.native", window: "settings", route: "general")
        defer { try? registration.invalidate() }
        XCTAssertEqual(button.accessibilityIdentifier(), "settings.native")
        button.performClick(nil)
        let request = try registry.prepare(args("settings.native"), sensitive: false)
        try registry.perform(request, authorized: { true })
        XCTAssertEqual(receiver.calls, 2, "User and adapter must execute the very same native target/action")
        button.isEnabled = false
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        button.isEnabled = true; button.isHidden = true
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        button.isHidden = false; window.orderOut(nil)
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        XCTAssertEqual(receiver.calls, 2)
    }
    func testWindowChromeOwnerPreservesMountUntilRouteOrPresentationChanges() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 160),
            styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        defer { window.close() }
        var route = "general"
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in route })
        let owner = NativeWindowControlRegistration(registry: registry)
        try owner.synchronize(window: window, id: "settings", route: route)
        let old = try registry.prepare(args("settings.titlebar.close"), sensitive: false)
        let revision = registry.revision
        try owner.synchronize(window: window, id: "settings", route: route)
        XCTAssertEqual(registry.revision, revision, "A passive refresh must not replace live mounts")
        route = "audio"
        try owner.synchronize(window: window, id: "settings", route: route)
        XCTAssertThrowsError(try registry.perform(old, authorized: { true }))
        let audio = try registry.prepare(args("settings.titlebar.close"), sensitive: false)
        window.orderOut(nil)
        try owner.synchronize(window: nil, id: "settings", route: nil)
        window.orderFront(nil)
        try owner.synchronize(window: window, id: "settings", route: route)
        XCTAssertThrowsError(try registry.perform(audio, authorized: { true }))
        XCTAssertTrue(window.isVisible)
        try registry.perform(registry.prepare(args("settings.titlebar.close"), sensitive: false), authorized: { true })
        XCTAssertFalse(window.isVisible)
        try owner.invalidate()
    }

    func testNativeCloseRetiresExactMountBeforeReopen() throws {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 240, height: 160),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFront(nil)
        defer { window.close() }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let button = try XCTUnwrap(window.standardWindowButton(.closeButton))
        let registration = try NativeButtonRegistration(button: button, registry: registry,
            id: "settings.titlebar.close", window: "settings", route: "general")
        let old = try registry.prepare(args("settings.titlebar.close"), sensitive: false)
        try registry.perform(old, authorized: { true })
        XCTAssertFalse(window.isVisible)
        window.orderFront(nil)
        let replacement = try NativeButtonRegistration(button: button, registry: registry,
            id: "settings.titlebar.close", window: "settings", route: "general")
        defer { try? replacement.invalidate(); try? registration.invalidate() }
        XCTAssertThrowsError(try registry.perform(old, authorized: { true }))
        XCTAssertTrue(window.isVisible, "An old native close cannot target a reopened presentation")
        try registry.perform(registry.prepare(args("settings.titlebar.close"), sensitive: false), authorized: { true })
        XCTAssertFalse(window.isVisible)
    }
}
#endif
