#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxAppSurface

@MainActor
final class CogletRadioGroupTests: XCTestCase {
    private let appearance = [
        CogletRadioGroup.Choice(value: "System", label: "System", id: "settings.general.appearance.system"),
        CogletRadioGroup.Choice(value: "Light", label: "Light", id: "settings.general.appearance.light"),
        CogletRadioGroup.Choice(value: "Dark", label: "Dark", id: "settings.general.appearance.dark")
    ]
    private func args(_ id: String, value: String? = nil) -> CBORValue {
        var fields: [CBORMapEntry] = [.init("window", .text("settings")), .init("control", .text(id)),
            .init("verb", .text(value == nil ? "press" : "select"))]
        if let value { fields.append(.init("value", .text(value))) }
        return .map(fields)
    }
    private func window() -> NSWindow {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 540, height: 180),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFrontRegardless()
        return window
    }
    private func configure(_ view: CogletRadioGroup.NativeView, registry: ControlRegistry,
                           choices: [CogletRadioGroup.Choice]? = nil, id: String = "settings.general.appearance",
                           enabled: Bool = true, selection: Binding<String>,
                           failure: @escaping (ControlRegistryFailure) -> Void = { XCTFail("Registration failed: \($0)") }) {
        view.configure(title: "Appearance", id: id, registry: registry, window: "settings", route: "general",
            choices: choices ?? appearance, selection: selection, enabled: enabled, failure: failure)
    }
    private func field(_ value: CBORValue, _ key: String) -> CBORValue? {
        guard case .map(let fields) = value else { return nil }
        return fields.first { $0.key == key }?.value
    }
    private func controls(_ registry: ControlRegistry) throws -> [String: CBORValue] {
        let page = try registry.page(window: "settings", limit: 128)
        guard case .array(let controls) = field(page, "controls") else { XCTFail("Missing controls"); return [:] }
        return Dictionary(uniqueKeysWithValues: controls.compactMap { control in
            guard case .text(let id) = field(control, "id") else { return nil }
            return (id, control)
        })
    }
    private func assertNativeAXValues(_ group: CogletRadioGroup.NativeView, file: StaticString = #filePath, line: UInt = #line) {
        for button in group.buttons {
            XCTAssertTrue(button.isAccessibilityElement(), file: file, line: line)
            XCTAssertEqual(button.accessibilityRole(), .radioButton, file: file, line: line)
            XCTAssertEqual(button.accessibilityValue() as? NSNumber, NSNumber(value: button.state == .on ? 1 : 0), file: file, line: line)
        }
    }
    func testAllSixChoicesAreActualNativeRadiosWithParentCatalogOwnership() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "System"
        let binding = Binding(get: { selected }, set: { selected = $0 })
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: binding)
        window.contentView?.addSubview(group)
        let sizes = ["standard", "large", "largest"].map {
            CogletRadioGroup.Choice(value: $0, label: $0.capitalized, id: "settings.general.text_size.\($0)")
        }
        var size = "standard"
        let sizeGroup = CogletRadioGroup.NativeView(frame: .zero)
        configure(sizeGroup, registry: registry, choices: sizes, id: "settings.general.text_size",
            selection: Binding(get: { size }, set: { size = $0 }))
        window.contentView?.addSubview(sizeGroup)
        let catalog = try controls(registry)
        XCTAssertEqual(catalog.count, 8)
        assertNativeAXValues(group)
        assertNativeAXValues(sizeGroup)
        for (view, choices, parent) in [(group, appearance, "settings.general.appearance"),
                                       (sizeGroup, sizes, "settings.general.text_size")] {
            XCTAssertTrue(view.window === window)
            XCTAssertEqual(view.accessibilityRole(), .radioGroup)
            XCTAssertEqual(view.accessibilityIdentifier(), parent)
            XCTAssertEqual(view.accessibilityChildren()?.count, 3)
            XCTAssertEqual(field(try XCTUnwrap(catalog[parent]), "role"), .text("picker"))
            XCTAssertEqual(view.buttons.count, 3)
            for (button, choice) in zip(view.buttons, choices) {
                XCTAssertTrue(button.window === window)
                XCTAssertTrue(button.superview === view)
                XCTAssertEqual(button.accessibilityRole(), .radioButton)
                XCTAssertEqual(button.accessibilityIdentifier(), choice.id)
                XCTAssertEqual(button.title, choice.label)
                XCTAssertEqual(button.font?.pointSize, 14)
                XCTAssertEqual(field(try XCTUnwrap(catalog[choice.id]), "parent"), .text(parent))
                XCTAssertEqual(field(try XCTUnwrap(catalog[choice.id]), "role"), .text("button"))
            }
        }
        for choice in appearance {
            try registry.perform(registry.prepare(args("settings.general.appearance", value: choice.value), sensitive: false), authorized: { true })
            XCTAssertEqual(selected, choice.value)
            XCTAssertEqual(group.buttons.filter { $0.state == .on }.count, 1)
            assertNativeAXValues(group)
        }
        for choice in sizes {
            try registry.perform(registry.prepare(args(choice.id), sensitive: false), authorized: { true })
            XCTAssertEqual(size, choice.value)
            XCTAssertEqual(sizeGroup.buttons.filter { $0.state == .on }.count, 1)
            assertNativeAXValues(sizeGroup)
        }
    }
    func testSharedNativeTargetActionValuesEventsAndFocus() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "System", writes = 0
        let binding = Binding(get: { selected }, set: { selected = $0; writes += 1 })
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: binding)
        window.contentView?.addSubview(group)
        var events: [(String, CBORValue)] = []
        registry.onChange = { events.append(($0, $1)) }
        let light = group.buttons[1], dark = group.buttons[2]
        XCTAssertTrue(light.target === group)
        XCTAssertEqual(light.action, dark.action)
        light.performClick(nil)
        XCTAssertEqual(selected, "Light")
        XCTAssertTrue(dark.accessibilityPerformPress())
        XCTAssertEqual(selected, "Dark")
        let event = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: " ", charactersIgnoringModifiers: " ", isARepeat: false, keyCode: 49))
        group.buttons[0].keyDown(with: event)
        XCTAssertEqual(selected, "System")
        try registry.perform(registry.prepare(args(appearance[1].id), sensitive: false), authorized: { true })
        XCTAssertEqual(selected, "Light")
        try registry.perform(registry.prepare(args("settings.general.appearance", value: "Dark"), sensitive: false), authorized: { true })
        XCTAssertEqual(selected, "Dark")
        XCTAssertEqual(writes, 5, "Every path must execute the shared Binding setter exactly once")
        let page = try controls(registry)
        XCTAssertEqual(field(try XCTUnwrap(page["settings.general.appearance"]), "value"),
            .map([.init("kind", .text("selection")), .init("value", .text("Dark"))]))
        XCTAssertEqual(field(try XCTUnwrap(page[appearance[2].id]), "value"),
            .map([.init("kind", .text("boolean")), .init("value", .boolean(true))]))
        let changed = events.filter { $0.0 == "softmax.app.control_changed" }.compactMap { field($0.1, "control") }
        for id in ["settings.general.appearance"] + appearance.map(\.id) { XCTAssertTrue(changed.contains(.text(id))) }
        let focus = try registry.prepareFocus(.map([.init("window", .text("settings")), .init("control", .text(appearance[1].id))]))
        try registry.setFocus(focus, authorized: { true })
        XCTAssertTrue(window.firstResponder === light)
        XCTAssertEqual(try registry.focusedControl(), .map([.init("control", .text(appearance[1].id)), .init("window", .text("settings"))]))
        XCTAssertTrue(events.contains { $0.0 == "softmax.app.focus_changed" && field($0.1, "control") == .text(appearance[1].id) })
        // External model updates must refresh both native state and observations.
        selected = "System"
        // No representable update yet: project the old actual native UI, not
        // the new binding mixed with old child states; reads remain available.
        let pending = try controls(registry)
        XCTAssertEqual(field(try XCTUnwrap(pending["settings.general.appearance"]), "value"),
            .map([.init("kind", .text("selection")), .init("value", .text("Dark"))]))
        XCTAssertEqual(field(try XCTUnwrap(pending[appearance[2].id]), "value"),
            .map([.init("kind", .text("boolean")), .init("value", .boolean(true))]))
        configure(group, registry: registry, selection: binding)
        XCTAssertEqual(group.buttons[0].state, .on)
        XCTAssertEqual(group.buttons[1].state, .off)
        XCTAssertEqual(group.buttons[2].state, .off)
    }

    func testTwoGroupFocusTransferNeverPublishesFalseClear() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "System", size = "standard"
        let first = CogletRadioGroup.NativeView(frame: .zero)
        configure(first, registry: registry, selection: Binding(get: { selected }, set: { selected = $0 }))
        window.contentView?.addSubview(first)
        let second = CogletRadioGroup.NativeView(frame: .zero)
        let choices = ["standard", "large", "largest"].map {
            CogletRadioGroup.Choice(value: $0, label: $0, id: "settings.general.text_size.\($0)")
        }
        configure(second, registry: registry, choices: choices, id: "settings.general.text_size",
            selection: Binding(get: { size }, set: { size = $0 }))
        window.contentView?.addSubview(second)
        var events: [CBORValue] = []
        registry.onChange = { if $0 == "softmax.app.focus_changed" { events.append($1) } }
        for id in [appearance[0].id, choices[1].id] {
            let token = try registry.prepareFocus(.map([.init("window", .text("settings")), .init("control", .text(id))]))
            try registry.setFocus(token, authorized: { true })
            // Drain deferred native responder observations from BOTH groups.
            RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        }
        XCTAssertTrue(window.firstResponder === second.buttons[1])
        first.dismantle()
        RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        XCTAssertFalse(events.contains { field($0, "control") == .null })
        XCTAssertEqual(field(try XCTUnwrap(events.last), "control"), .text(choices[1].id))
        let arrow = try XCTUnwrap(NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: [],
            timestamp: 0, windowNumber: window.windowNumber, context: nil,
            characters: "", charactersIgnoringModifiers: "", isARepeat: false, keyCode: 124))
        second.buttons[1].keyDown(with: arrow)
        XCTAssertEqual(size, "largest")
        XCTAssertTrue(window.firstResponder === second.buttons[2])
        RunLoop.main.run(until: Date().addingTimeInterval(0.02))
        events.removeAll()
        second.dismantle()
        XCTAssertEqual(field(try XCTUnwrap(events.last), "control"), .null,
            "Retiring the actually focused group must publish a real clear")
    }

    func testOffGridSelectionRemainsObservableAndHumanCanRepairIt() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "unavailable"
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: Binding(get: { selected }, set: { selected = $0 }))
        window.contentView?.addSubview(group)
        let page = try controls(registry)
        XCTAssertEqual(page.count, 4)
        XCTAssertEqual(field(try XCTUnwrap(page["settings.general.appearance"]), "value"),
            .map([.init("kind", .text("none"))]))
        XCTAssertTrue(group.buttons.allSatisfy { $0.state == .off })
        assertNativeAXValues(group)
        group.buttons[1].performClick(nil)
        XCTAssertEqual(selected, "Light")
        XCTAssertEqual(group.buttons[1].state, .on)
        assertNativeAXValues(group)
        // An already registered group must also survive a later off-grid
        // model update, publishing none rather than leaking the unknown value.
        selected = "unavailable"
        configure(group, registry: registry, selection: Binding(get: { selected }, set: { selected = $0 }))
        let updated = try controls(registry)
        XCTAssertEqual(updated.count, 4)
        XCTAssertEqual(field(try XCTUnwrap(updated["settings.general.appearance"]), "value"),
            .map([.init("kind", .text("none"))]))
        XCTAssertTrue(group.buttons.allSatisfy { $0.state == .off })
        assertNativeAXValues(group)
        XCTAssertNoThrow(try registry.synchronizeNativeState(windows: ["settings"]))
    }

    func testInvalidChoiceStillRefusedBeforeRegistration() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "System"
        var failures: [ControlRegistryFailure] = []
        let group = CogletRadioGroup.NativeView(frame: .zero)
        let invalid = [CogletRadioGroup.Choice(value: "unavailable", label: "Invalid", id: "settings.general.appearance.invalid")]
        configure(group, registry: registry, choices: invalid,
            selection: Binding(get: { selected }, set: { selected = $0 }), failure: { failures.append($0) })
        window.contentView?.addSubview(group)
        XCTAssertEqual(failures, [.invalidRegistration])
        XCTAssertTrue(group.buttons.isEmpty)
        XCTAssertTrue(try controls(registry).isEmpty)
        XCTAssertEqual(selected, "System")
    }

    func testDisabledHiddenAndWrongRouteCannotMutateOrFocus() throws {
        var route = "general"
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in route })
        let window = window(); defer { window.close() }
        var selected = "System"
        let binding = Binding(get: { selected }, set: { selected = $0 })
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: binding)
        window.contentView?.addSubview(group)
        let request = try registry.prepare(args(appearance[1].id), sensitive: false)
        let parent = try registry.prepare(args("settings.general.appearance", value: "Dark"), sensitive: false)
        let focus = try registry.prepareFocus(.map([.init("window", .text("settings")), .init("control", .text(appearance[1].id))]))
        configure(group, registry: registry, enabled: false, selection: binding)
        XCTAssertTrue(group.buttons.allSatisfy { !$0.isEnabled })
        group.buttons[1].performClick(nil)
        XCTAssertFalse(group.buttons[1].accessibilityPerformPress())
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        XCTAssertThrowsError(try registry.perform(parent, authorized: { true }))
        XCTAssertThrowsError(try registry.setFocus(focus, authorized: { true }))
        XCTAssertEqual(selected, "System")
        configure(group, registry: registry, selection: binding)
        group.isHidden = true
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        XCTAssertFalse(group.buttons[1].accessibilityPerformPress())
        group.isHidden = false
        route = "audio"
        XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        route = "general"
        window.orderOut(nil)
        XCTAssertThrowsError(try registry.perform(parent, authorized: { true }))
        XCTAssertEqual(selected, "System")
    }

    func testDetachDismantleAndReopenRetireExactParentAndChildMounts() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        var selected = "System"
        let binding = Binding(get: { selected }, set: { selected = $0 })
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: binding)
        window.contentView?.addSubview(group)
        let old = try registry.prepare(args(appearance[1].id), sensitive: false)
        let oldParent = try registry.prepare(args("settings.general.appearance", value: "Dark"), sensitive: false)
        group.removeFromSuperview()
        XCTAssertTrue(registry.registrationInventory().isEmpty)
        window.contentView?.addSubview(group)
        XCTAssertEqual(registry.registrationInventory().count, 4)
        XCTAssertThrowsError(try registry.perform(old, authorized: { true }))
        XCTAssertThrowsError(try registry.perform(oldParent, authorized: { true }))
        let beforeOrderOut = try registry.prepare(args(appearance[1].id), sensitive: false)
        window.orderOut(nil)
        XCTAssertTrue(registry.registrationInventory().isEmpty, "orderOut must synchronously retire the presentation")
        window.orderFrontRegardless()
        configure(group, registry: registry, selection: binding)
        XCTAssertThrowsError(try registry.perform(beforeOrderOut, authorized: { true }))
        let beforeClose = try registry.prepare(args(appearance[1].id), sensitive: false)
        window.close()
        XCTAssertTrue(registry.registrationInventory().isEmpty)
        window.orderFrontRegardless()
        configure(group, registry: registry, selection: binding)
        XCTAssertEqual(registry.registrationInventory().count, 4)
        XCTAssertThrowsError(try registry.perform(beforeClose, authorized: { true }))
        let beforeDismantle = try registry.prepare(args(appearance[1].id), sensitive: false)
        group.dismantle()
        XCTAssertTrue(registry.registrationInventory().isEmpty)
        let successor = CogletRadioGroup.NativeView(frame: .zero)
        configure(successor, registry: registry, selection: binding)
        window.contentView?.addSubview(successor)
        group.dismantle() // An old teardown must never remove its successor.
        XCTAssertEqual(registry.registrationInventory().count, 4)
        XCTAssertThrowsError(try registry.perform(beforeDismantle, authorized: { true }))
        try registry.perform(registry.prepare(args(appearance[1].id), sensitive: false), authorized: { true })
        XCTAssertEqual(selected, "Light")
    }

    func testSwiftUIEnvironmentDisablesActualNativeButtons() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        let host = NSHostingView(rootView: CogletRadioGroup("Appearance", id: "settings.general.appearance",
            registry: registry, window: "settings", route: "general", choices: appearance,
            selection: .constant("System"), failure: { XCTFail("Registration failed: \($0)") }).disabled(true))
        window.contentView = host
        host.layoutSubtreeIfNeeded()
        // Let NSHostingView complete its first native representable update.
        RunLoop.main.run(until: Date().addingTimeInterval(0.05))
        func find(_ view: NSView) -> CogletRadioGroup.NativeView? {
            if let group = view as? CogletRadioGroup.NativeView { return group }
            return view.subviews.lazy.compactMap { find($0) }.first
        }
        let group = try XCTUnwrap(find(host))
        XCTAssertTrue(group.buttons.allSatisfy { !$0.isEnabled })
        XCTAssertEqual(registry.registrationInventory().count, 4)
        XCTAssertTrue(registry.registrationInventory().allSatisfy { !$0.enabled })
        XCTAssertFalse(group.buttons[1].accessibilityPerformPress())
        XCTAssertThrowsError(try registry.perform(registry.prepare(args(appearance[1].id), sensitive: false), authorized: { true }))
    }

    func testRouteAndExactWindowReplacementFenceQueuedRequests() throws {
        var route = "general"
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in route })
        let first = window(), second = window()
        defer { first.close(); second.close() }
        var selected = "System"
        let binding = Binding(get: { selected }, set: { selected = $0 })
        let group = CogletRadioGroup.NativeView(frame: .zero)
        configure(group, registry: registry, selection: binding)
        first.contentView?.addSubview(group)
        let original = try registry.prepare(args(appearance[1].id), sensitive: false)
        route = "audio"
        group.configure(title: "Appearance", id: "settings.general.appearance", registry: registry,
            window: "settings", route: "audio", choices: appearance, selection: binding, enabled: true,
            failure: { XCTFail("Registration failed: \($0)") })
        XCTAssertThrowsError(try registry.perform(original, authorized: { true }))
        let beforeMove = try registry.prepare(args(appearance[1].id), sensitive: false)
        group.removeFromSuperview()
        second.contentView?.addSubview(group)
        XCTAssertTrue(group.buttons.allSatisfy { $0.window === second })
        XCTAssertThrowsError(try registry.perform(beforeMove, authorized: { true }))
        first.close()
        XCTAssertEqual(registry.registrationInventory().count, 4, "The old window cannot retire the new window's mounts")
        try registry.perform(registry.prepare(args(appearance[1].id), sensitive: false), authorized: { true })
        XCTAssertEqual(selected, "Light")
    }

    func testDuplicateChildRollsBackOnlyOurPartialParentMount() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        let existing = try registry.mount(id: appearance[1].id, window: "settings", route: "general", role: "button",
            label: "Already registered", verbs: ["press"], value: { .map([.init("kind", .text("none"))]) }, invoke: { _ in })
        defer { try? registry.unmount(existing) }
        var selected = "System"
        let group = CogletRadioGroup.NativeView(frame: .zero)
        var failures: [ControlRegistryFailure] = []
        configure(group, registry: registry, selection: Binding(get: { selected }, set: { selected = $0 }), failure: { failures.append($0) })
        window.contentView?.addSubview(group)
        XCTAssertEqual(failures, [.duplicate])
        XCTAssertEqual(registry.registrationInventory().map(\.id), [appearance[1].id])
        XCTAssertThrowsError(try registry.prepare(args(appearance[2].id), sensitive: false))
        XCTAssertThrowsError(try registry.prepare(args("settings.general.appearance", value: "Dark"), sensitive: false))
        group.buttons[2].performClick(nil)
        XCTAssertEqual(selected, "Dark", "Human UI remains usable; only partial adapter registration fails closed")
        XCTAssertEqual(registry.registrationInventory().map(\.id), [appearance[1].id])
        group.dismantle()
        XCTAssertEqual(registry.registrationInventory().map(\.id), [appearance[1].id])
    }

    func testDuplicateChoiceIDsAndValuesFailClosedBeforeMounting() {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let window = window(); defer { window.close() }
        for choices in [[appearance[0], appearance[0]],
                        [appearance[0], .init(value: "System", label: "Other", id: "settings.general.appearance.other")]] {
            let group = CogletRadioGroup.NativeView(frame: .zero)
            var failures: [ControlRegistryFailure] = []
            configure(group, registry: registry, choices: choices, selection: .constant("System"), failure: { failures.append($0) })
            window.contentView?.addSubview(group)
            XCTAssertEqual(failures, [.invalidRegistration])
            XCTAssertTrue(registry.registrationInventory().isEmpty)
            XCTAssertTrue(group.buttons.isEmpty)
            group.dismantle()
        }
    }
}
#endif
