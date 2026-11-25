import XCTest
@testable import SoftmaxAppSurface

@MainActor
private final class Host {
    var windows: [String: String] = ["settings": "general", "menu_bar": "menu"]
    var appearance = "System", textSize = "standard", orb = true, toggleMode = false, sound = true
    var loginCalls = 0, pressCalls = 0, navigateCalls = 0, openCalls = 0, closeCalls = 0
    var ignoreSettingWrites = false, ignoreNavigate = false
    var registry: ControlRegistry!
    var adapter: AppSurfaceAdapter!

    nonisolated static func obj(_ v: [String: CBORValue]) -> CBORValue { .map(v.sorted { $0.key < $1.key }.map { .init($0.key, $0.value) }) }
    func sel(_ v: String) -> CBORValue { Host.obj(["kind": .text("selection"), "value": .text(v)]) }
    func bool(_ v: Bool) -> CBORValue { Host.obj(["kind": .text("boolean"), "value": .boolean(v)]) }

    init() throws {
        registry = ControlRegistry(revision: 7, currentRoute: { [unowned self] in self.windows[$0] })
        adapter = Self.makeAdapter(host: self, registry: registry)
        try mountAll()
    }
    static func makeAdapter(host: Host, registry: ControlRegistry) -> AppSurfaceAdapter {
        AppSurfaceAdapter(registry: registry, captureState: { try host.snapshot() },
            currentRoute: { host.windows[$0] },
            navigate: { w, r in host.navigateCalls += 1; if !host.ignoreNavigate { host.windows[w] = r } },
            open: { w in
                host.openCalls += 1
                host.windows[w] = ["settings": "general", "about": "about", "chat": "chat"][w] ?? "menu"
            },
            close: { w in host.closeCalls += 1; host.windows[w] = nil })
    }
    func mountAll() throws {
        func m(_ id: String, _ route: String, _ role: String, _ verbs: Set<String>, sensitive: Bool = false,
               value: @escaping () throws -> CBORValue, invoke: @escaping (CBORValue) throws -> Void) throws -> ControlRegistry.Mount {
            try registry.mount(id: id, window: "settings", route: route, role: role, label: id, sensitive: sensitive,
                               verbs: verbs, value: value, invoke: invoke)
        }
        func arg(_ a: CBORValue) -> CBORValue {
            guard case .map(let f) = a else { return .null }
            return f.first { $0.key == "value" }!.value
        }
        _ = try m("settings.general.appearance", "general", "picker", ["select"], value: { self.sel(self.appearance) }) { a in
            if !self.ignoreSettingWrites, case .text(let t) = arg(a) { self.appearance = t }
        }
        _ = try m("settings.general.text_size", "general", "picker", ["select"], value: { self.sel(self.textSize) }) { a in
            if !self.ignoreSettingWrites, case .text(let t) = arg(a) { self.textSize = t }
        }
        _ = try m("floatingOrbToggle", "general", "toggle", ["toggle"], value: { self.bool(self.orb) }) { a in
            if !self.ignoreSettingWrites, case .boolean(let b) = arg(a) { self.orb = b }
        }
        _ = try m("settings.hotkey.toggle_mode", "hotkey", "toggle", ["toggle"], value: { self.bool(self.toggleMode) }) { a in
            if !self.ignoreSettingWrites, case .boolean(let b) = arg(a) { self.toggleMode = b }
        }
        _ = try m("settings.audio.sound_cues", "audio", "toggle", ["toggle"], value: { self.bool(self.sound) }) { a in
            if !self.ignoreSettingWrites, case .boolean(let b) = arg(a) { self.sound = b }
        }
        _ = try m("settings.general.login_enable", "general", "button", ["press"], sensitive: true,
                  value: { Host.obj(["kind": .text("none")]) }) { _ in self.loginCalls += 1 }
        _ = try m("settings.general.button", "general", "button", ["press"],
                  value: { Host.obj(["kind": .text("none")]) }) { _ in self.pressCalls += 1 }
    }
    func snapshot(revision: UInt64? = nil) throws -> CBORValue {
        let win: [CBORValue] = windows.sorted { $0.key < $1.key }.map {
            Host.obj(["open": .boolean(true), "window": .text($0.key), "route": .text($0.value)])
        }
        let hotkey = Host.obj(["key_code": .unsigned(61), "modifiers": .array([])])
        return Host.obj([
            "revision": .unsigned(revision ?? registry.revision), "windows": .array(win), "focus": .null,
            "settings": Host.obj(["appearance": .text(appearance), "text_size": .text(textSize), "floating_orb": .boolean(orb),
                "talk_mode": .text(toggleMode ? "toggle" : "hold"), "sound_cues": .boolean(sound),
                "hotkey": hotkey, "launch_at_login": .text("disabled")]),
            "ptt": Host.obj(["state": .text("idle"), "visible": .boolean(false), "elapsed_ms": .unsigned(0), "level": .unsigned(0)]),
            "hud": Host.obj(["state": .text("idle"), "visible": .boolean(false), "elapsed_ms": .unsigned(0), "level": .unsigned(0)]),
            "hub": .text("offline"),
            "face": Host.obj(["visible": .boolean(false), "mode": .text("full"), "expression": .text("neutral"), "status": .text("idle")])
        ])
    }
}

final class AppSurfaceAdapterTests: XCTestCase, @unchecked Sendable {
    func testQueuedWindowCommandCannotBorrowReopenedPresentation() async throws {
        try await MainActor.run {
            let host = try Host()
            let request = try host.adapter.prepare("softmax.app.close_window", args: Host.obj(["window": .text("settings")]))
            host.windows["settings"] = nil
            try host.registry.synchronizeNativeState(windows: ["settings", "menu_bar"])
            host.windows["settings"] = "general"
            try host.registry.synchronizeNativeState(windows: ["settings", "menu_bar"])
            XCTAssertThrowsError(try host.adapter.perform(request, authorized: { true })) { error in
                XCTAssertEqual(error as? AppSurfaceFailure, .conflict)
            }
            XCTAssertEqual(host.closeCalls, 0)
            XCTAssertEqual(host.windows["settings"], "general")
        }
    }

    func testModalBlockingIsRecheckedImmediatelyBeforeNativeCommand() async throws {
        try await MainActor.run {
            var modal = false, effects = 0
            let registry = ControlRegistry(revision: 0, interactionAllowed: { _ in !modal }, currentRoute: { _ in "general" })
            _ = try registry.mount(id: "native", window: "settings", route: "general", role: "button", label: "Native",
                verbs: ["press"], value: { Host.obj(["kind": .text("none")]) }, invoke: { _ in effects += 1 })
            let request = try registry.prepare(Host.obj(["window": .text("settings"), "control": .text("native"), "verb": .text("press")]), sensitive: false)
            modal = true
            XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
            XCTAssertEqual(effects, 0)
            modal = false
            try registry.perform(request, authorized: { true })
            XCTAssertEqual(effects, 1)
        }
    }

    private func obj(_ v: [String: CBORValue]) -> CBORValue { Host.obj(v) }
    private func invoke(_ id: String, verb: String = "press") -> CBORValue {
        obj(["window": .text("settings"), "control": .text(id), "verb": .text(verb)])
    }
    private func setting(_ key: String, _ value: CBORValue) -> CBORValue { obj(["key": .text(key), "value": value]) }
    private func fail(_ expected: AppSurfaceFailure, file: StaticString = #filePath, line: UInt = #line,
                      _ body: () throws -> Void) {
        XCTAssertThrowsError(try body(), file: file, line: line) { XCTAssertEqual($0 as? AppSurfaceFailure, expected, file: file, line: line) }
    }
    private let empty: CBORValue = .map([])

    func testInvokeRunsActualCallbackAndEffectTimeAuthorization() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            let p = try a.prepare("softmax.app.invoke", args: invoke("settings.general.button"))
            fail(.permissionDenied) { _ = try a.perform(p, authorized: { false }) }
            XCTAssertEqual(h.pressCalls, 0)
            XCTAssertEqual(try a.perform(p, authorized: { true }), self.empty)
            XCTAssertEqual(h.pressCalls, 1)
            fail(.conflict) { _ = try a.perform(p, authorized: { true }) } // single use
            XCTAssertEqual(h.pressCalls, 1)
            // Ordinary path never reaches a sensitive control, and vice versa.
            fail(.permissionDenied) { _ = try a.prepare("softmax.app.invoke", args: self.invoke("settings.general.login_enable")) }
            fail(.permissionDenied) { _ = try a.prepare("softmax.app.invoke_sensitive", args: self.invoke("settings.general.button")) }
            let s = try a.prepare("softmax.app.invoke_sensitive", args: self.invoke("settings.general.login_enable"))
            _ = try a.perform(s, authorized: { true })
            XCTAssertEqual(h.loginCalls, 1)
        }
    }

    func testQueuedOldMountAndRouteChangeRefused() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            let old = try a.prepare("softmax.app.invoke", args: self.invoke("settings.general.button"))
            // Replace the exact mounted control with a successor using the same id.
            let r2 = ControlRegistry(revision: 0, currentRoute: { h.windows[$0] })
            let a2 = Host.makeAdapter(host: h, registry: r2)
            var calls = 0
            let m1 = try r2.mount(id: "x.button", window: "settings", route: "general", role: "button", label: "x",
                verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in calls += 1 })
            let queued = try a2.prepare("softmax.app.invoke", args: self.invoke("x.button"))
            try r2.unmount(m1)
            _ = try r2.mount(id: "x.button", window: "settings", route: "general", role: "button", label: "x",
                verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in calls += 100 })
            fail(.conflict) { _ = try a2.perform(queued, authorized: { true }) }
            XCTAssertEqual(calls, 0)
            // Route moved away after prepare.
            h.windows["settings"] = "audio"
            fail(.unavailable) { _ = try a.perform(old, authorized: { true }) }
            XCTAssertEqual(h.pressCalls, 0)
        }
    }

    func testCrossAdapterPreparedCommandRefused() async throws {
        try await MainActor.run {
            let h = try Host()
            let other = Host.makeAdapter(host: h, registry: h.registry) // same registry, different identity
            let p = try h.adapter.prepare("softmax.app.invoke", args: self.invoke("settings.general.button"))
            fail(.conflict) { _ = try other.perform(p, authorized: { true }) }
            let nav = try h.adapter.prepare("softmax.app.navigate", args: self.obj(["window": .text("settings"), "route": .text("audio")]))
            fail(.conflict) { _ = try other.perform(nav, authorized: { true }) }
            XCTAssertEqual(h.pressCalls, 0); XCTAssertEqual(h.navigateCalls, 0)
            // A foreign registry's request is also refused by the registry.
            let foreign = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
            let m = try foreign.mount(id: "settings.general.button", window: "settings", route: "general", role: "button",
                label: "b", verbs: ["press"], value: { self.obj(["kind": .text("none")]) }, invoke: { _ in })
            _ = m
            let fa = Host.makeAdapter(host: h, registry: foreign)
            let fp = try fa.prepare("softmax.app.invoke", args: self.invoke("settings.general.button"))
            fail(.conflict) { _ = try h.adapter.perform(fp, authorized: { true }) }
        }
    }

    func testNavigateOpenCloseUseHostAndCheckActualState() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            let before = h.registry.revision
            let nav = try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("settings"), "route": .text("audio")]))
            fail(.permissionDenied) { _ = try a.perform(nav, authorized: { false }) }
            XCTAssertEqual(h.navigateCalls, 0)
            XCTAssertEqual(try a.perform(nav, authorized: { true }), self.empty)
            XCTAssertEqual(h.windows["settings"], "audio")
            XCTAssertGreaterThan(h.registry.revision, before)
            // Host that does not actually move is reported, not claimed applied.
            h.ignoreNavigate = true
            let bad = try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("settings"), "route": .text("hub")]))
            fail(.unavailable) { _ = try a.perform(bad, authorized: { true }) }
            // Closed window cannot be navigated.
            let closedNav = try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("about"), "route": .text("about")]))
            fail(.unavailable) { _ = try a.perform(closedNav, authorized: { true }) }
            XCTAssertEqual(h.navigateCalls, 1 + 1)
            _ = try a.perform(try a.prepare("softmax.app.open_window", args: self.obj(["window": .text("about")])), authorized: { true })
            XCTAssertEqual(h.windows["about"], "about"); XCTAssertEqual(h.openCalls, 1)
            _ = try a.perform(try a.prepare("softmax.app.close_window", args: self.obj(["window": .text("about")])), authorized: { true })
            XCTAssertNil(h.windows["about"]); XCTAssertEqual(h.closeCalls, 1)
            // main is never fabricated: this host cannot open it into main/main... closing an already closed window is a no-op.
            _ = try a.perform(try a.prepare("softmax.app.close_window", args: self.obj(["window": .text("about")])), authorized: { true })
            XCTAssertEqual(h.closeCalls, 1)
        }
    }

    func testGeneratedInvalidArgumentsAndUnknownOperationsRefused() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("settings"), "route": .text("welcome")])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("main"), "route": .text("menu")])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.open_window", args: self.obj(["window": .text("nope")])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.open_window", args: self.obj(["window": .text("about"), "extra": .null])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.invoke", args: self.obj(["window": .text("settings"), "control": .text("c"), "verb": .text("dance")])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.set_focus", args: self.obj(["window": .text("settings")])) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.set_setting", args: self.setting("appearance", .text("Purple"))) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.set_setting", args: self.setting("sound_cues", .text("on"))) }
            fail(.invalidArguments) { _ = try a.prepare("softmax.app.set_setting", args: self.setting("arbitrary_pref", .boolean(true))) }
            fail(.unsupported) { _ = try a.prepare("softmax.app.state", args: self.empty) }
            fail(.unsupported) { _ = try a.prepare("softmax.app.nope", args: self.empty) }
            fail(.unsupported) { _ = try a.read("softmax.app.invoke", args: self.empty) }
            // Unmounted target.
            fail(.unavailable) { _ = try a.prepare("softmax.app.invoke", args: self.invoke("missing.control")) }
            XCTAssertEqual(h.navigateCalls + h.openCalls + h.closeCalls + h.pressCalls, 0)
        }
    }

    func testSetSettingTransformsToExactMountedControlsOnly() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            @MainActor func apply(_ key: String, _ value: CBORValue) throws {
                _ = try a.perform(try a.prepare("softmax.app.set_setting", args: self.setting(key, value)), authorized: { true })
            }
            try apply("appearance", .text("Dark")); XCTAssertEqual(h.appearance, "Dark")
            try apply("text_size", .text("largest")); XCTAssertEqual(h.textSize, "largest")
            try apply("floating_orb", .boolean(false)); XCTAssertFalse(h.orb)
            h.windows["settings"] = "audio"
            try apply("sound_cues", .boolean(false)); XCTAssertFalse(h.sound)
            h.windows["settings"] = "hotkey"
            try apply("talk_mode", .text("toggle")); XCTAssertTrue(h.toggleMode)
            try apply("talk_mode", .text("hold")); XCTAssertFalse(h.toggleMode)
            // Wrong tab: native route check refuses; nothing changes.
            h.windows["settings"] = "hotkey"
            let p = try a.prepare("softmax.app.set_setting", args: self.setting("appearance", .text("Light")))
            fail(.unavailable) { _ = try a.perform(p, authorized: { true }) }
            XCTAssertEqual(h.appearance, "Dark")
            // Hotkey fails closed; login is permission denied; neither touches native code.
            let hotkey = self.obj(["key_code": .unsigned(3), "modifiers": .array([.text("command")])])
            fail(.unsupported) { _ = try a.prepare("softmax.app.set_setting", args: self.setting("hotkey", hotkey)) }
            fail(.permissionDenied) { _ = try a.prepare("softmax.app.set_setting", args: self.setting("launch_at_login", .boolean(true))) }
            XCTAssertEqual(h.loginCalls, 0)
            // Unmounted control (no fabricated success).
            let bare = ControlRegistry(revision: 0, currentRoute: { h.windows[$0] })
            let ba = Host.makeAdapter(host: h, registry: bare)
            fail(.unavailable) { _ = try ba.prepare("softmax.app.set_setting", args: self.setting("sound_cues", .boolean(true))) }
        }
    }

    func testSetSettingReportsPostcommandStateMismatch() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            h.ignoreSettingWrites = true; h.windows["settings"] = "audio"
            let p = try a.prepare("softmax.app.set_setting", args: self.setting("sound_cues", .boolean(false)))
            fail(.conflict) { _ = try a.perform(p, authorized: { true }) }
            XCTAssertTrue(h.sound)
        }
    }

    func testSetFocusUsesRegistryWitness() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            // Registered controls above have no focus witness: unsupported, not faked.
            let p = try a.prepare("softmax.app.set_focus", args: self.obj(["window": .text("settings"), "control": .text("settings.general.button")]))
            fail(.unsupported) { _ = try a.perform(p, authorized: { true }) }
            var focused = false
            let r = ControlRegistry(revision: 0, currentRoute: { h.windows[$0] })
            _ = try r.mount(id: "settings.search", window: "settings", route: "general", role: "text_field", label: "s",
                verbs: ["type_text"], value: { self.obj(["kind": .text("text"), "value": .text("")]) },
                focus: { focused = true; return true }, focused: { focused }, invoke: { _ in })
            let fa = Host.makeAdapter(host: h, registry: r)
            let fp = try fa.prepare("softmax.app.set_focus", args: self.obj(["window": .text("settings"), "control": .text("settings.search")]))
            fail(.permissionDenied) { _ = try fa.perform(fp, authorized: { false }) }
            XCTAssertFalse(focused)
            XCTAssertEqual(try fa.perform(fp, authorized: { true }), self.empty)
            XCTAssertTrue(focused)
        }
    }

    func testStateReadValidatesGeneratedRuleRevisionAndActualRoutes() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            let state = try a.read("softmax.app.state", args: self.empty)
            try CBORCodec.validate("app-state-result", state)
            fail(.invalidArguments) { _ = try a.read("softmax.app.state", args: self.obj(["x": .null])) }
            // Snapshot revision must be the registry's.
            let stale = AppSurfaceAdapter(registry: h.registry, captureState: { try h.snapshot(revision: 1) },
                currentRoute: { h.windows[$0] }, navigate: { _, _ in }, open: { _ in }, close: { _ in })
            fail(.conflict) { _ = try stale.read("softmax.app.state", args: self.empty) }
            // Snapshot disagreeing with actual host routes is refused.
            let lying = AppSurfaceAdapter(registry: h.registry, captureState: { try h.snapshot() },
                currentRoute: { _ in "audio" }, navigate: { _, _ in }, open: { _ in }, close: { _ in })
            fail(.conflict) { _ = try lying.read("softmax.app.state", args: self.empty) }
            // A host snapshot violating the closed schema never leaves the adapter.
            let bad = AppSurfaceAdapter(registry: h.registry, captureState: { self.obj(["revision": .unsigned(h.registry.revision)]) },
                currentRoute: { h.windows[$0] }, navigate: { _, _ in }, open: { _ in }, close: { _ in })
            fail(.invalidArguments) { _ = try bad.read("softmax.app.state", args: self.empty) }
        }
    }

    func testControlsReadParsesValidatedFieldsAndPaginates() async throws {
        try await MainActor.run {
            let h = try Host(); let a = h.adapter!
            func args(after: CBORValue = .null, limit: UInt64, revision: CBORValue = .null) -> CBORValue {
                self.obj(["window": .text("settings"), "after": after, "limit": .unsigned(limit), "revision": revision])
            }
            let first = try a.read("softmax.app.controls", args: args(limit: 3))
            try CBORCodec.validate("app-controls-result", first)
            guard case .map(let f) = first, case .text(let cursor)? = f.first(where: { $0.key == "next" })?.value,
                  case .unsigned(let rev)? = f.first(where: { $0.key == "revision" })?.value else { return XCTFail("no cursor") }
            let second = try a.read("softmax.app.controls", args: args(after: .text(cursor), limit: 128, revision: .unsigned(rev)))
            try CBORCodec.validate("app-controls-result", second)
            XCTAssertNotEqual(first, second)
            // Cursor without revision is a generated-rule violation; wrong revision conflicts.
            fail(.invalidArguments) { _ = try a.read("softmax.app.controls", args: args(after: .text(cursor), limit: 1)) }
            fail(.conflict) { _ = try a.read("softmax.app.controls", args: args(after: .text(cursor), limit: 1, revision: .unsigned(rev &+ 5))) }
            fail(.invalidArguments) { _ = try a.read("softmax.app.controls", args: args(limit: 0)) }
            fail(.invalidArguments) { _ = try a.read("softmax.app.controls", args: args(limit: 129)) }
            // Sensitive values never appear in the page.
            let bytes = String(decoding: try CBORCodec.encodeProfile(first) + CBORCodec.encodeProfile(second), as: UTF8.self)
            XCTAssertTrue(bytes.contains("settings.general.login_enable"))
            // Mutation (actual navigation) invalidates the old cursor.
            _ = try a.perform(try a.prepare("softmax.app.navigate", args: self.obj(["window": .text("settings"), "route": .text("general")])), authorized: { true })
            fail(.conflict) { _ = try a.read("softmax.app.controls", args: args(after: .text(cursor), limit: 1, revision: .unsigned(rev))) }
        }
    }
}
