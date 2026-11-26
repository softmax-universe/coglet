#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxAppSurface

/// Native tests for original SwiftUI Pickers hosted in a real NSHostingView.
/// External app AX census and strict snapshots remain separate qualification.
@MainActor
final class RegisteredPickerTests: XCTestCase {
    private final class Model: ObservableObject {
        @Published var appearance = "System"
        @Published var textSize = 1.0
        @Published var presented = true
        var appearanceWrites = 0, textSizeWrites = 0
    }
    private let appearanceChoices: [(value: String, wire: String)] = [("System", "System"), ("Light", "Light"), ("Dark", "Dark")]
    private let sizeChoices: [(value: Double, wire: String)] = [(1.0, "standard"), (1.3, "large"), (1.6, "largest")]
    private let pickerID = "settings.general.appearance", sizeID = "settings.general.text_size"

    private struct Fixture: View {
        @ObservedObject var model: Model
        let registry: ControlRegistry
        let route: String
        let appearance: [(value: String, wire: String)]
        let sizes: [(value: Double, wire: String)]
        let pickerID: String, sizeID: String
        let failure: (ControlRegistryFailure) -> Void
        var body: some View {
            let appearanceBinding = Binding(get: { model.appearance }, set: { model.appearance = $0; model.appearanceWrites += 1 })
            let sizeBinding = Binding(get: { model.textSize }, set: { model.textSize = $0; model.textSizeWrites += 1 })
            if model.presented {
            VStack(alignment: .leading) {
                Picker("Appearance", selection: appearanceBinding) {
                    ForEach(appearance, id: \.wire) { choice in
                        Text(choice.wire).tag(choice.value)
                            .cogletPickerChoice(id: pickerID + "." + choice.wire.lowercased(), registry: registry,
                                window: "settings", route: route, label: choice.wire,
                                selection: appearanceBinding, choice: choice.value, failure: failure)
                    }
                }
                .pickerStyle(.radioGroup)
                .cogletPickerSelection(id: pickerID, registry: registry, window: "settings", route: route,
                    label: "Appearance", selection: appearanceBinding, choices: appearance, failure: failure)
                Picker("Text size", selection: sizeBinding) {
                    ForEach(sizes, id: \.wire) { choice in
                        Text(choice.wire).tag(choice.value)
                            .cogletPickerChoice(id: sizeID + "." + choice.wire, registry: registry,
                                window: "settings", route: route, label: choice.wire,
                                selection: sizeBinding, choice: choice.value, failure: failure)
                    }
                }
                .pickerStyle(.radioGroup)
                .cogletPickerSelection(id: sizeID, registry: registry, window: "settings", route: route,
                    label: "Text size", selection: sizeBinding, choices: sizes, failure: failure)
            }
            }
        }
    }

    private struct Harness {
        let model: Model, registry: ControlRegistry, window: NSWindow, hosting: NSHostingView<Fixture>
        let routeBox: RouteBox
    }
    private final class RouteBox { var open: String? = "general" }

    private func pump() { RunLoop.main.run(until: Date().addingTimeInterval(0.1)) }
    private func waitUntil(_ ready: () -> Bool, file: StaticString = #filePath, line: UInt = #line) {
        let deadline = Date().addingTimeInterval(3)
        while !ready() && Date() < deadline {
            RunLoop.main.run(until: Date().addingTimeInterval(0.01))
        }
        XCTAssertTrue(ready(), "Native registration did not reach the expected state", file: file, line: line)
    }
    private func harness(route: String = "general", failure: @escaping (ControlRegistryFailure) -> Void = { XCTFail("Registration failed: \($0)") }) -> Harness {
        _ = NSApplication.shared
        let box = RouteBox(), model = Model()
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in box.open })
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 420, height: 320),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        let hosting = NSHostingView(rootView: fixture(model, registry, route, failure))
        window.contentView = hosting
        window.orderFrontRegardless()
        hosting.layoutSubtreeIfNeeded()
        waitUntil { registry.registrationInventory().count == 8 }
        return Harness(model: model, registry: registry, window: window, hosting: hosting, routeBox: box)
    }
    private func fixture(_ model: Model, _ registry: ControlRegistry, _ route: String,
                         _ failure: @escaping (ControlRegistryFailure) -> Void) -> Fixture {
        Fixture(model: model, registry: registry, route: route, appearance: appearanceChoices, sizes: sizeChoices,
                pickerID: pickerID, sizeID: sizeID, failure: failure)
    }
    private func args(_ id: String, verb: String, value: String? = nil) -> CBORValue {
        var fields: [CBORMapEntry] = [.init("window", .text("settings")), .init("control", .text(id)), .init("verb", .text(verb))]
        if let value { fields.append(.init("value", .text(value))) }
        return .map(fields)
    }
    private func field(_ value: CBORValue, _ key: String) -> CBORValue? {
        guard case .map(let fields) = value else { return nil }
        return fields.first { $0.key == key }?.value
    }
    private func controls(_ registry: ControlRegistry) throws -> [String: CBORValue] {
        let page = try registry.page(window: "settings", limit: 128)
        guard case .array(let all)? = field(page, "controls") else { XCTFail("Missing controls"); return [:] }
        return Dictionary(uniqueKeysWithValues: all.compactMap { control in
            guard case .text(let id)? = field(control, "id") else { return nil }
            return (id, control)
        })
    }
    private func value(_ registry: ControlRegistry, _ id: String) throws -> CBORValue {
        try XCTUnwrap(field(try XCTUnwrap(try controls(registry)[id], "missing \(id)"), "value"))
    }
    private func boolean(_ b: Bool) -> CBORValue { .map([.init("kind", .text("boolean")), .init("value", .boolean(b))]) }
    private func selection(_ wire: String) -> CBORValue { .map([.init("kind", .text("selection")), .init("value", .text(wire))]) }
    private let none: CBORValue = .map([.init("kind", .text("none"))])
    private func press(_ registry: ControlRegistry, _ id: String) throws {
        try registry.perform(registry.prepare(args(id, verb: "press"), sensitive: false), authorized: { true })
    }
    private func select(_ registry: ControlRegistry, _ id: String, _ wire: String) throws {
        try registry.perform(registry.prepare(args(id, verb: "select", value: wire), sensitive: false), authorized: { true })
    }
    private func assertState(_ registry: ControlRegistry, appearance: String?, size: String?,
                             file: StaticString = #filePath, line: UInt = #line) throws {
        XCTAssertEqual(try value(registry, pickerID), appearance.map(selection) ?? none, file: file, line: line)
        XCTAssertEqual(try value(registry, sizeID), size.map(selection) ?? none, file: file, line: line)
        for choice in appearanceChoices {
            XCTAssertEqual(try value(registry, pickerID + "." + choice.wire.lowercased()), boolean(choice.wire == appearance), file: file, line: line)
        }
        for choice in sizeChoices {
            XCTAssertEqual(try value(registry, sizeID + "." + choice.wire), boolean(choice.wire == size), file: file, line: line)
        }
    }

    func testAllSixChoicesAndBothPickersRegisterOnOriginalViewsAndReadLiveValues() throws {
        let h = harness(); defer { h.window.close() }
        let catalog = try controls(h.registry)
        XCTAssertEqual(catalog.count, 8)
        XCTAssertEqual(field(try XCTUnwrap(catalog[pickerID]), "role"), .text("picker"))
        XCTAssertEqual(field(try XCTUnwrap(catalog[sizeID]), "role"), .text("picker"))
        for choice in appearanceChoices { XCTAssertEqual(field(try XCTUnwrap(catalog[pickerID + "." + choice.wire.lowercased()]), "role"), .text("button")) }
        for choice in sizeChoices { XCTAssertEqual(field(try XCTUnwrap(catalog[sizeID + "." + choice.wire]), "role"), .text("button")) }
        let inventory = Dictionary(uniqueKeysWithValues: h.registry.registrationInventory().map { ($0.id, $0) })
        XCTAssertEqual(inventory[pickerID]?.verbs, ["select"])
        XCTAssertEqual(inventory[sizeID + ".large"]?.verbs, ["press"])
        try assertState(h.registry, appearance: "System", size: "standard")
        // Model changes made outside the registry are read live, with no re-registration.
        h.model.appearance = "Dark"; h.model.textSize = 1.6
        try assertState(h.registry, appearance: "Dark", size: "largest")
        XCTAssertEqual(h.model.appearanceWrites + h.model.textSizeWrites, 0)
    }

    func testChildPressAndParentSelectMutateSameModelThroughSameBinding() throws {
        let h = harness(); defer { h.window.close() }
        try press(h.registry, pickerID + ".light")
        XCTAssertEqual(h.model.appearance, "Light")
        try assertState(h.registry, appearance: "Light", size: "standard")
        try select(h.registry, pickerID, "Dark")
        XCTAssertEqual(h.model.appearance, "Dark")
        try press(h.registry, sizeID + ".largest")
        XCTAssertEqual(h.model.textSize, 1.6)
        try select(h.registry, sizeID, "large")
        XCTAssertEqual(h.model.textSize, 1.3)
        XCTAssertEqual(h.model.appearanceWrites, 2)
        XCTAssertEqual(h.model.textSizeWrites, 2)
        try assertState(h.registry, appearance: "Dark", size: "large")
        // Every one of the six choices is reachable from both entry points.
        for choice in appearanceChoices {
            try press(h.registry, pickerID + "." + choice.wire.lowercased()); XCTAssertEqual(h.model.appearance, choice.value)
            try select(h.registry, pickerID, choice.wire); XCTAssertEqual(h.model.appearance, choice.value)
        }
        for choice in sizeChoices {
            try press(h.registry, sizeID + "." + choice.wire); XCTAssertEqual(h.model.textSize, choice.value)
            try select(h.registry, sizeID, choice.wire); XCTAssertEqual(h.model.textSize, choice.value)
        }
    }

    func testInvalidSelectInputRefusedWithoutMutation() throws {
        let h = harness(); defer { h.window.close() }
        h.model.appearance = "Light"
        let writes = h.model.appearanceWrites
        // "standard" is a valid wire for the OTHER picker; "dark" differs in case.
        for bad in ["standard", "dark", "", "Unknown"] {
            XCTAssertThrowsError(try select(h.registry, pickerID, bad))
        }
        let missing = CBORValue.map([.init("window", .text("settings")), .init("control", .text(pickerID)), .init("verb", .text("select"))])
        XCTAssertThrowsError(try h.registry.perform(h.registry.prepare(missing, sensitive: false), authorized: { true }))
        let wrongType = CBORValue.map([.init("window", .text("settings")), .init("control", .text(pickerID)),
            .init("verb", .text("select")), .init("value", .boolean(true))])
        XCTAssertThrowsError(try h.registry.perform(h.registry.prepare(wrongType, sensitive: false), authorized: { true }))
        // Verb not declared by the registration is refused before any effect.
        XCTAssertThrowsError(try h.registry.perform(h.registry.prepare(args(pickerID, verb: "press"), sensitive: false), authorized: { true }))
        XCTAssertThrowsError(try h.registry.perform(h.registry.prepare(args(pickerID + ".dark", verb: "select", value: "Dark"), sensitive: false), authorized: { true }))
        XCTAssertEqual(h.model.appearance, "Light")
        XCTAssertEqual(h.model.appearanceWrites, writes)
        try assertState(h.registry, appearance: "Light", size: "standard")
    }

    func testRepeatedSelectedChoiceNeverTogglesOff() throws {
        let h = harness(); defer { h.window.close() }
        for _ in 0..<3 {
            try press(h.registry, pickerID + ".system")
            XCTAssertEqual(h.model.appearance, "System")
            try select(h.registry, pickerID, "System")
            try press(h.registry, sizeID + ".standard")
            try select(h.registry, sizeID, "standard")
            try assertState(h.registry, appearance: "System", size: "standard")
        }
    }

    func testOffGridModelProjectsNoneAllFalseAndRecoversByChildOrParent() throws {
        let h = harness(); defer { h.window.close() }
        h.model.appearance = "Sepia"; h.model.textSize = 1.5
        try assertState(h.registry, appearance: nil, size: nil)
        XCTAssertNoThrow(try h.registry.synchronizeNativeState(windows: ["settings"]))
        try press(h.registry, pickerID + ".dark")
        try select(h.registry, sizeID, "large")
        try assertState(h.registry, appearance: "Dark", size: "large")
        h.model.appearance = "Sepia"; h.model.textSize = 1.5
        try select(h.registry, pickerID, "Light")
        try press(h.registry, sizeID + ".largest")
        try assertState(h.registry, appearance: "Light", size: "largest")
    }

    func testSettlingEmitsParentAndChangedChildrenOnceAndDeduplicates() throws {
        let h = harness(); defer { h.window.close() }
        try h.registry.synchronizeNativeState(windows: ["settings"])
        var changed: [String] = []
        h.registry.onChange = { kind, payload in
            if kind == "softmax.app.control_changed", case .text(let id)? = self.field(payload, "control") { changed.append(id) }
        }
        try press(h.registry, pickerID + ".dark")
        pump()
        try h.registry.synchronizeNativeState(windows: ["settings"])
        pump()
        try h.registry.synchronizeNativeState(windows: ["settings"])
        // System turned off, Dark turned on, parent changed; Light and the size picker did not.
        XCTAssertEqual(changed.sorted(), [pickerID, pickerID + ".dark", pickerID + ".system"].sorted())
        changed.removeAll()
        try select(h.registry, pickerID, "Dark")   // no-op effect: already selected
        pump()
        try h.registry.synchronizeNativeState(windows: ["settings"])
        XCTAssertTrue(changed.isEmpty, "Unchanged projections must be deduplicated, got \(changed)")
        // An external model change settles through the same observation path.
        h.model.textSize = 1.3
        pump()
        try h.registry.synchronizeNativeState(windows: ["settings"])
        XCTAssertEqual(changed.sorted(), [sizeID, sizeID + ".large", sizeID + ".standard"].sorted())
    }

    func testRouteUnmountRemountRefusesStalePreparedRequest() throws {
        let h = harness(); defer { h.window.close() }
        let stale = try h.registry.prepare(args(pickerID + ".dark", verb: "press"), sensitive: false)
        let staleSelect = try h.registry.prepare(args(pickerID, verb: "select", value: "Dark"), sensitive: false)
        let failure: (ControlRegistryFailure) -> Void = { XCTFail("Registration failed: \($0)") }
        // Route change re-registers every control on that route.
        h.hosting.rootView = fixture(h.model, h.registry, "other", failure)
        h.hosting.layoutSubtreeIfNeeded()
        waitUntil { h.registry.registrationInventory().count == 8 && h.registry.registrationInventory().allSatisfy { $0.route == "other" } }
        XCTAssertEqual(h.registry.registrationInventory().count, 8)
        XCTAssertTrue(h.registry.registrationInventory().allSatisfy { $0.route == "other" })
        XCTAssertThrowsError(try h.registry.perform(stale, authorized: { true }))
        XCTAssertThrowsError(try h.registry.perform(staleSelect, authorized: { true }))
        XCTAssertEqual(h.model.appearance, "System")
        XCTAssertEqual(h.model.appearanceWrites, 0)
        // The actual open route is still "general": the new mounts are unavailable.
        XCTAssertThrowsError(try press(h.registry, pickerID + ".dark"))
        h.routeBox.open = "other"
        try press(h.registry, pickerID + ".dark")
        XCTAssertEqual(h.model.appearance, "Dark")
        // Remove the whole view: mounts disappear, requests prepared earlier cannot resurrect them.
        let live = try h.registry.prepare(args(pickerID, verb: "select", value: "Light"), sensitive: false)
        h.model.presented = false
        waitUntil { h.registry.registrationInventory().isEmpty }
        XCTAssertTrue(h.registry.registrationInventory().isEmpty)
        XCTAssertThrowsError(try h.registry.perform(live, authorized: { true }))
        XCTAssertEqual(h.model.appearance, "Dark")
        // Remount: a fresh incarnation exists, the earlier request remains refused.
        h.model.presented = true
        waitUntil { h.registry.registrationInventory().count == 8 }
        XCTAssertEqual(h.registry.registrationInventory().count, 8)
        XCTAssertThrowsError(try h.registry.perform(live, authorized: { true }))
        try select(h.registry, pickerID, "Light")
        XCTAssertEqual(h.model.appearance, "Light")
    }

    func testMalformedDeclarationsFailRegistrationInsteadOfExposingAControl() throws {
        _ = NSApplication.shared
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        var failures: [ControlRegistryFailure] = []
        var selected = "System"
        let binding = Binding(get: { selected }, set: { selected = $0 })
        let bad: [[(value: String, wire: String)]] = [[], [("System", "System"), ("Light", "System")], [("System", "not-a-wire")]]
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 200, height: 200),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        for (index, choices) in bad.enumerated() {
            window.contentView = NSHostingView(rootView: Text("x").cogletPickerSelection(
                id: "settings.general.bad\(index)", registry: registry, window: "settings", route: "general",
                label: "Bad", selection: binding, choices: choices, failure: { failures.append($0) }))
            window.orderFrontRegardless(); pump()
        }
        XCTAssertEqual(failures.count, 3)
        XCTAssertTrue(registry.registrationInventory().isEmpty)
        XCTAssertEqual(selected, "System")
    }
}
#endif
