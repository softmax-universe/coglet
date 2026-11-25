import XCTest
@testable import SoftmaxAppSurface

final class ControlRegistryTests: XCTestCase, @unchecked Sendable {
    private func obj(_ v: [String: CBORValue]) -> CBORValue { .map(v.sorted { $0.key < $1.key }.map { .init($0.key, $0.value) }) }
    private func invoke(_ id: String, verb: String = "press") -> CBORValue {
        obj(["window": .text("settings"), "control": .text(id), "verb": .text(verb)])
    }
    func testMountReplacementCannotBorrowSuccessorAndOldTeardownIsHarmless() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            var calls = 0
            let old = try r.mount(id: "settings.button", window: "settings", route: "general", role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in calls += 1 })
            let request = try r.prepare(invoke("settings.button"), sensitive: false)
            try r.unmount(old)
            let new = try r.mount(id: "settings.button", window: "settings", route: "general", role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in calls += 1 })
            try r.unmount(old)
            XCTAssertThrowsError(try r.perform(request, authorized: { true })) { XCTAssertEqual($0 as? ControlRegistryFailure, .conflict) }
            try r.perform(r.prepare(invoke("settings.button"), sensitive: false), authorized: { true })
            XCTAssertEqual(calls, 1)
            try r.unmount(new)
        }
    }
    func testSensitiveDirectionAndLateAuthorizationNeverEnterNative() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            var calls = 0
            _ = try r.mount(id: "settings.quit", window: "settings", route: "general", role: "button", label: "Quit", sensitive: true, verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in calls += 1 })
            XCTAssertThrowsError(try r.prepare(invoke("settings.quit"), sensitive: false))
            let request = try r.prepare(invoke("settings.quit"), sensitive: true)
            XCTAssertThrowsError(try r.perform(request, authorized: { false }))
            XCTAssertEqual(calls, 0)
            try r.perform(request, authorized: { true })
            XCTAssertEqual(calls, 1)
        }
    }
    func testSecureGetterNeverRunsAndEventsAlwaysRedact() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "hub" })
            var getterCalls = 0
            var event: CBORValue?
            r.onChange = { _, value in event = value }
            let mount = try r.mount(id: "settings.secret", window: "settings", route: "hub", role: "secure_field", label: "Pairing code", secure: true, verbs: ["type_text"], value: { getterCalls += 1; return self.obj(["kind": .text("text"), "value": .text("TOKEN_CANARY")]) }, invoke: { _ in })
            let page = try r.page(window: "settings", limit: 128)
            try r.controlChanged(mount, force: true)
            XCTAssertEqual(getterCalls, 0)
            let bytes = try CBORCodec.encodeProfile(page) + CBORCodec.encodeProfile(event!)
            XCTAssertFalse(String(decoding: bytes, as: UTF8.self).contains("TOKEN_CANARY"))
            XCTAssertTrue(String(decoding: bytes, as: UTF8.self).contains("redacted"))
        }
    }
    func testActualRouteDisabledVisibilityAndVerbChecks() async throws {
        try await MainActor.run {
            var route = "general", enabled = true, visible = true, calls = 0
            let r = ControlRegistry(revision: 0, currentRoute: { _ in route })
            _ = try r.mount(id: "settings.button", window: "settings", route: "general", role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, enabled: { enabled }, visible: { visible }, invoke: { _ in calls += 1 })
            let request = try r.prepare(invoke("settings.button"), sensitive: false)
            route = "audio"; XCTAssertThrowsError(try r.perform(request, authorized: { true }))
            route = "general"; enabled = false; XCTAssertThrowsError(try r.perform(request, authorized: { true }))
            enabled = true; visible = false; XCTAssertThrowsError(try r.perform(request, authorized: { true }))
            visible = true
            let wrong = self.obj(["window": .text("settings"), "control": .text("settings.button"), "verb": .text("toggle"), "value": .boolean(true)])
            XCTAssertThrowsError(try r.perform(r.prepare(wrong, sensitive: false), authorized: { true }))
            XCTAssertEqual(calls, 0)
            try r.perform(request, authorized: { true })
            XCTAssertEqual(calls, 1)
        }
    }
    func testFocusUsesRealWitnessAndPaginationMutationConflicts() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            var focused = false
            let mount = try r.mount(id: "settings.search", window: "settings", route: "general", role: "text_field", label: "Search", verbs: ["type_text"], value: { self.obj(["kind": .text("text"), "value": .text("")]) }, focus: { focused = true; return true }, focused: { focused }, invoke: { _ in })
            _ = try r.mount(id: "settings.button", window: "settings", route: "general", role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in })
            XCTAssertEqual(try r.focusedControl(), .null)
            let captured = try r.prepareFocus(self.obj(["window": .text("settings"), "control": .text("settings.search")]))
            XCTAssertEqual(captured, mount)
            try r.setFocus(captured, authorized: { true })
            XCTAssertTrue(focused)
            XCTAssertNotEqual(try r.focusedControl(), .null)
            let first = try r.page(window: "settings", limit: 1)
            let fields = try first.requireMap()
            guard case .text(let cursor) = fields.first(where: { $0.key == "next" })!.value else { return XCTFail("missing cursor") }
            let revision = r.revision
            _ = try r.page(window: "settings", after: cursor, revision: revision, limit: 1)
            try r.controlChanged(mount, force: true)
            XCTAssertThrowsError(try r.page(window: "settings", after: cursor, revision: revision, limit: 1))
        }
    }
    func testProtectedGetterAndRepeatedPaginationAreBounded() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            var getterCalls = 0
            _ = try r.mount(id: "settings.protected", window: "settings", route: "general", role: "toggle", label: "Protected", sensitive: true, verbs: ["toggle"], value: {
                getterCalls += 1; return self.obj(["kind": .text("text"), "value": .text("PROTECTED_CANARY")])
            }, invoke: { _ in })
            _ = try r.mount(id: "settings.second", window: "settings", route: "general", role: "button", label: "Second", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in })
            let first = try r.page(window: "settings", limit: 1)
            // One cursor per actual window/revision/offset, not per read request.
            for _ in 0..<4100 { XCTAssertEqual(try r.page(window: "settings", limit: 1), first) }
            XCTAssertEqual(getterCalls, 0)
            XCTAssertFalse(String(decoding: try CBORCodec.encodeProfile(first), as: UTF8.self).contains("PROTECTED_CANARY"))
        }
    }

    func testDuplicateDanglingParentInvalidRouteUnicodeAliasAndExhaustion() async throws {
        try await MainActor.run {
            let r = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            @MainActor func mount(_ id: String, parent: String? = nil, route: String = "general") throws {
                _ = try r.mount(id: id, window: "settings", route: route, parent: parent, role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in })
            }
            try mount("valid")
            XCTAssertThrowsError(try mount("valid"))
            XCTAssertThrowsError(try mount("child", parent: "absent"))
            XCTAssertThrowsError(try mount("bad", route: "not-a-route"))
            XCTAssertThrowsError(try mount("e\u{301}"))
            let exhausted = ControlRegistry(revision: UInt64.max, currentRoute: { _ in "general" })
            XCTAssertThrowsError(try exhausted.mount(id: "no", window: "settings", route: "general", role: "button", label: "Button", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in }))
        }
    }
}
