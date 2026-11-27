import XCTest
@testable import SoftmaxAppSurface

final class FocusObservationTests: XCTestCase, @unchecked Sendable {
    @MainActor
    private final class Fixture {
        var shown = false
        var searchFocused = true
        var codeFocused = false
        var searchEnabled = true
        var searchFocusCalls = 0
        var searchInvocations = 0
        var events: [(String, CBORValue)] = []
        lazy var registry = ControlRegistry(revision: 0,
            interactionAllowed: { [unowned self] in !shown || $0 == "hub_pairing" },
            focusObservationEligible: { [unowned self] in !shown || $0 != "settings" },
            currentRoute: { [unowned self] in route($0) })
        func route(_ window: String) -> String? {
            switch window {
            case "settings": return "hub"
            case "hub_pairing": return shown ? "code" : nil
            default: return nil
            }
        }
        func mount(on registry: ControlRegistry) throws {
            _ = try registry.mount(id: "settings.search", window: "settings", route: "hub",
                role: "text_field", label: "Search", verbs: ["type_text"],
                value: { .map([.init("kind", .text("text")), .init("value", .text(""))]) },
                enabled: { [unowned self] in searchEnabled },
                focus: { [unowned self] in searchFocusCalls += 1; searchFocused = true; return true },
                focused: { [unowned self] in searchFocused },
                invoke: { [unowned self] _ in searchInvocations += 1 })
            _ = try registry.mount(id: "settings.hub.pairing.code", window: "hub_pairing", route: "code",
                role: "secure_field", label: "Pairing code", secure: true, verbs: ["type_text"],
                value: { XCTFail("Secure getter must not execute"); return .null },
                focused: { [unowned self] in codeFocused }, invoke: { _ in })
        }
        func observe() throws { try registry.synchronizeNativeState(windows: ["settings", "hub_pairing"]) }
        var focusEvents: [CBORValue] { events.filter { $0.0 == "softmax.app.focus_changed" }.map(\.1) }
    }

    private func field(_ value: CBORValue, _ key: String) throws -> CBORValue {
        try XCTUnwrap(try value.requireMap().first { $0.key == key }?.value)
    }
    private func focus(_ window: String, _ control: String) -> CBORValue {
        .map([.init("control", .text(control)), .init("window", .text(window))])
    }
    private func assertConflict(_ operation: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertThrowsError(try operation(), file: file, line: line) {
            XCTAssertEqual($0 as? ControlRegistryFailure, .conflict, file: file, line: line)
        }
    }

    // Characterizes a43 without the new initializer argument: interaction policy
    // alone never arbitrates the two independent local focus witnesses.
    func testDefaultEligibilityRetainsOriginalDualWitnessConflict() async throws {
        try await MainActor.run {
            let fixture = Fixture()
            let registry = ControlRegistry(revision: 0,
                interactionAllowed: { _ in false }, currentRoute: { fixture.route($0) })
            try fixture.mount(on: registry)
            fixture.shown = true; fixture.codeFocused = true
            assertConflict { _ = try registry.focusedControl() }
            assertConflict { try registry.synchronizeNativeState(windows: ["settings", "hub_pairing"]) }
        }
    }

    func testShownSheetExcludesRetainedParentAndDismissalRestoresItOnce() async throws {
        try await MainActor.run {
            let fixture = Fixture()
            try fixture.mount(on: fixture.registry)
            fixture.registry.onChange = { [unowned fixture] in fixture.events.append(($0, $1)) }
            try fixture.observe()
            fixture.events.removeAll()

            fixture.shown = true; fixture.codeFocused = true
            XCTAssertTrue(fixture.searchFocused) // Never clear the retained parent witness.
            XCTAssertEqual(try fixture.registry.focusedControl(), focus("hub_pairing", "settings.hub.pairing.code"))
            try fixture.observe()
            XCTAssertEqual(fixture.focusEvents.count, 1)
            XCTAssertEqual(try field(fixture.focusEvents[0], "control"), .text("settings.hub.pairing.code"))
            XCTAssertEqual(try field(fixture.focusEvents[0], "window"), .text("hub_pairing"))
            fixture.events.removeAll()
            try fixture.observe()
            XCTAssertTrue(fixture.events.isEmpty)

            // Pairing blocks parent effects independently of passive eligibility.
            let parent = try fixture.registry.prepareFocus(focus("settings", "settings.search"))
            XCTAssertThrowsError(try fixture.registry.setFocus(parent, authorized: { true })) {
                XCTAssertEqual($0 as? ControlRegistryFailure, .unavailable)
            }
            XCTAssertEqual(fixture.searchFocusCalls, 0)

            // The sheet's local truth may remain set after dismissal: its route
            // closes, while the still-true parent becomes eligible again.
            fixture.shown = false
            XCTAssertTrue(fixture.codeFocused)
            XCTAssertEqual(try fixture.registry.focusedControl(), focus("settings", "settings.search"))
            try fixture.observe()
            XCTAssertEqual(fixture.focusEvents.count, 1)
            XCTAssertEqual(try field(fixture.focusEvents[0], "control"), .text("settings.search"))
            fixture.events.removeAll()
            try fixture.observe()
            XCTAssertTrue(fixture.events.isEmpty)
        }
    }

    func testDisabledFocusedControlRemainsObservable() async throws {
        try await MainActor.run {
            let fixture = Fixture()
            fixture.searchEnabled = false
            try fixture.mount(on: fixture.registry)
            XCTAssertEqual(try fixture.registry.focusedControl(), focus("settings", "settings.search"))
            try fixture.observe()
        }
    }

    func testSameEligibleOwnerConflictStillFailsWithoutPublishingFocus() async throws {
        try await MainActor.run {
            let fixture = Fixture()
            try fixture.mount(on: fixture.registry)
            try fixture.observe()
            fixture.registry.onChange = { [unowned fixture] in fixture.events.append(($0, $1)) }
            _ = try fixture.registry.mount(id: "settings.other", window: "settings", route: "hub",
                role: "text_field", label: "Other", verbs: ["type_text"],
                value: { .map([.init("kind", .text("none"))]) }, focused: { true }, invoke: { _ in })
            assertConflict { _ = try fixture.registry.focusedControl() }
            assertConflict { try fixture.observe() }
            XCTAssertTrue(fixture.events.isEmpty)
        }
    }

    func testUnrelatedOwnerFocusSwitchIsNotKeyWindowArbitration() async throws {
        try await MainActor.run {
            var searchFocused = true, aboutFocused = false
            var events: [(String, CBORValue)] = []
            let registry = ControlRegistry(revision: 0, focusObservationEligible: { _ in true }) {
                $0 == "settings" ? "hub" : "about"
            }
            _ = try registry.mount(id: "settings.search", window: "settings", route: "hub",
                role: "text_field", label: "Search", verbs: ["type_text"],
                value: { .map([.init("kind", .text("none"))]) }, focused: { searchFocused }, invoke: { _ in })
            _ = try registry.mount(id: "about.button", window: "about", route: "about",
                role: "button", label: "About", verbs: ["press"],
                value: { .map([.init("kind", .text("none"))]) }, focused: { aboutFocused }, invoke: { _ in })
            try registry.synchronizeNativeState(windows: ["settings", "about"])
            registry.onChange = { events.append(($0, $1)) }
            searchFocused = false; aboutFocused = true
            XCTAssertEqual(try registry.focusedControl(), focus("about", "about.button"))
            try registry.synchronizeNativeState(windows: ["settings", "about"])
            XCTAssertEqual(events.map(\.0), ["softmax.app.focus_changed"])
            XCTAssertEqual(try field(events[0].1, "control"), .text("about.button"))
            events.removeAll()
            searchFocused = true
            assertConflict { _ = try registry.focusedControl() }
            assertConflict { try registry.synchronizeNativeState(windows: ["settings", "about"]) }
            XCTAssertTrue(events.isEmpty, "Unrelated owner ambiguity must not be silently arbitrated")
        }
    }

    func testEligibilityDoesNotChangePagesOrActionAuthority() async throws {
        try await MainActor.run {
            var eligible = true, focused = true, calls = 0
            let registry = ControlRegistry(revision: 0, focusObservationEligible: { _ in eligible }) { _ in "hub" }
            let mount = try registry.mount(id: "settings.search", window: "settings", route: "hub",
                role: "text_field", label: "Search", verbs: ["type_text"],
                value: { .map([.init("kind", .text("none"))]) },
                focus: { calls += 1; focused = true; return true }, focused: { focused }, invoke: { _ in calls += 1 })
            let before = try registry.page(window: "settings", limit: 128)
            eligible = false
            XCTAssertEqual(try registry.focusedControl(), .null)
            XCTAssertEqual(try registry.page(window: "settings", limit: 128), before)
            XCTAssertThrowsError(try registry.setFocus(mount, authorized: { false })) {
                XCTAssertEqual($0 as? ControlRegistryFailure, .permissionDenied)
            }
            try registry.setFocus(mount, authorized: { true })
            let request = try registry.prepare(.map([.init("window", .text("settings")),
                .init("control", .text("settings.search")), .init("verb", .text("type_text")),
                .init("value", .text("test"))]), sensitive: false)
            XCTAssertThrowsError(try registry.perform(request, authorized: { false }))
            try registry.perform(request, authorized: { true })
            XCTAssertEqual(calls, 2)
        }
    }
}
