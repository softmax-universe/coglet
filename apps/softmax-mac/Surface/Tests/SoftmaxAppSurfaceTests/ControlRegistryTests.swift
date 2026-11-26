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

    // MARK: Opt-in asynchronous focus settling (deterministic: injected clock/sleeper)

    @MainActor
    final class FocusProbe {
        var focusCalls = 0, sleeps = 0, replacementFocusCalls = 0, replacementWitnessReads = 0
        var focused = false
        var settleAfterSleeps: Int?
        var now: ContinuousClock.Instant
        var start: ContinuousClock.Instant
        var route = "general", enabled = true, visible = true, allowed = true, interaction = true
        var onSleep: (() -> Void)?
        var onFocus: (() -> Void)?
        var onWitness: (() -> Void)?
        var events: [(String, CBORValue)] = []
        var registry: ControlRegistry!
        var mount: ControlRegistry.Mount!
        init(settleAfter: Int?, revision: UInt64 = 0, settles: Bool = true) throws {
            let t0 = ContinuousClock.now
            start = t0; now = t0
            settleAfterSleeps = settleAfter
            registry = ControlRegistry(revision: revision, interactionAllowed: { [unowned self] _ in self.interaction },
                                       currentRoute: { [unowned self] _ in self.route })
            registry.onChange = { [unowned self] kind, payload in self.events.append((kind, payload)) }
            registry.focusSettling = .init(timeout: .seconds(2), interval: .milliseconds(10),
                now: { [unowned self] in self.now },
                sleep: { [unowned self] duration in
                    self.sleeps += 1
                    self.now = self.now.advanced(by: duration)
                    try Task.checkCancellation()
                    self.onSleep?()
                    if let n = self.settleAfterSleeps, self.sleeps >= n { self.focused = true }
                })
            mount = try registry.mount(id: "settings.search", window: "settings", route: "general", role: "text_field",
                label: "Search", verbs: ["type_text"], value: { .map([.init("kind", .text("none"))]) },
                enabled: { [unowned self] in self.enabled }, visible: { [unowned self] in self.visible },
                focus: { [unowned self] in self.focusCalls += 1; self.onFocus?(); return true },
                focused: { [unowned self] in self.onWitness?(); return self.focused },
                focusSettles: settles, invoke: { _ in })
        }
        func run() async throws { try await registry.setFocusAsync(mount, authorized: { [unowned self] in self.allowed }) }
        var focusEvents: [CBORValue] { events.filter { $0.0 == "softmax.app.focus_changed" }.map { $0.1 } }
    }

    func testAsyncFocusSettlesAfterDelayWithExactlyOneEffectAndOneEvent() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: 3) }
        let before = await MainActor.run { p.registry.revision }
        try await p.run()
        try await MainActor.run {
            XCTAssertEqual(p.focusCalls, 1)
            XCTAssertEqual(p.sleeps, 3)
            XCTAssertEqual(p.focusEvents.count, 1)
            XCTAssertEqual(p.registry.revision, before + 1)
            if case .map(let f) = p.focusEvents[0], case .unsigned(let rev)? = f.first(where: { $0.key == "revision" })?.value {
                XCTAssertEqual(rev, p.registry.revision)
            } else { XCTFail("focus event lacks revision") }
            // The passive observer must not publish a duplicate for the same focus.
            try p.registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(p.focusEvents.count, 1)
            XCTAssertNotEqual(try p.registry.focusedControl(), .null)
        }
    }

    func testAsyncFocusPassiveObservationDuringSleepPublishesOnlyOnce() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
        let before = try await MainActor.run {
            try p.registry.synchronizeNativeState(windows: ["settings"])
            p.events = []
            p.onSleep = {
                p.focused = true
                do { try p.registry.synchronizeNativeState(windows: ["settings"]) }
                catch { XCTFail("passive observation failed: \(error)") }
            }
            return p.registry.revision
        }
        try await p.run()
        try await MainActor.run {
            XCTAssertEqual(p.focusCalls, 1)
            XCTAssertEqual(p.sleeps, 1)
            XCTAssertEqual(p.focusEvents.count, 1)
            XCTAssertEqual(p.registry.revision, before + 1)
            let event = try XCTUnwrap(p.focusEvents.first)
            let fields = try event.requireMap()
            XCTAssertEqual(fields.first(where: { $0.key == "revision" })?.value, .unsigned(p.registry.revision))
            try p.registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(p.focusEvents.count, 1)
            XCTAssertEqual(p.registry.revision, before + 1)
        }
    }

    func testAsyncFocusAlreadyObservedStillRequiresFreshAuthority() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
        let before = try await MainActor.run {
            try p.registry.synchronizeNativeState(windows: ["settings"])
            p.events = []
            p.onSleep = {
                p.focused = true
                do { try p.registry.synchronizeNativeState(windows: ["settings"]) }
                catch { XCTFail("passive observation failed: \(error)") }
                p.allowed = false
            }
            return p.registry.revision
        }
        do { try await p.run(); XCTFail("an observed transition is not command authority") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .permissionDenied) }
        await MainActor.run {
            XCTAssertEqual(p.focusCalls, 1)
            // The truthful passive event remains; refusal emits no second event.
            XCTAssertEqual(p.focusEvents.count, 1)
            XCTAssertEqual(p.registry.revision, before + 1)
        }
    }

    func testAsyncFocusRejectsWitnessAtOrAfterExclusiveDeadline() async throws {
        for elapsed in [Duration.seconds(2), .seconds(3)] {
            let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
            await MainActor.run {
                p.onSleep = { p.now = p.start.advanced(by: elapsed); p.focused = true }
            }
            do { try await p.run(); XCTFail("late true witness must not succeed") }
            catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
            await MainActor.run {
                XCTAssertTrue(p.allowed) // Independent of ticket/authority expiry.
                XCTAssertTrue(p.focused)
                XCTAssertEqual(p.focusCalls, 1)
                XCTAssertEqual(p.sleeps, 1)
                XCTAssertEqual(p.focusEvents.count, 0)
                XCTAssertEqual(p.registry.revision, 1)
            }
        }
    }

    func testAsyncFocusAcceptsWitnessStrictlyBeforeDeadline() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
        await MainActor.run {
            p.onSleep = { p.now = p.start.advanced(by: .milliseconds(1999)); p.focused = true }
        }
        try await p.run()
        await MainActor.run {
            XCTAssertEqual(p.focusCalls, 1)
            XCTAssertEqual(p.focusEvents.count, 1)
        }
    }

    func testAsyncFocusDeadlineIncludesEffectAndFinalWitnessTime() async throws {
        for duringEffect in [true, false] {
            let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
            await MainActor.run {
                p.focused = true
                if duringEffect {
                    p.onFocus = { p.now = p.start.advanced(by: .seconds(2)) }
                } else {
                    var reads = 0
                    p.onWitness = {
                        reads += 1
                        if reads == 2 { p.now = p.start.advanced(by: .seconds(2)) }
                    }
                }
            }
            do { try await p.run(); XCTFail("deadline must fence final publication") }
            catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
            await MainActor.run {
                XCTAssertEqual(p.focusCalls, 1)
                XCTAssertEqual(p.sleeps, 0)
                XCTAssertEqual(p.focusEvents.count, 0)
            }
        }
    }

    func testAsyncFocusRejectsInvalidPolicyBeforeEffect() async throws {
        let policies: [(Duration, Duration)] = [(.zero, .milliseconds(10)),
            (.seconds(-1), .milliseconds(10)), (.seconds(3), .milliseconds(10)),
            (.seconds(2), .zero), (.seconds(2), .milliseconds(-1)), (.seconds(1), .seconds(2))]
        for (timeout, interval) in policies {
            let p = try await MainActor.run { try FocusProbe(settleAfter: 1) }
            await MainActor.run {
                p.registry.focusSettling.timeout = timeout
                p.registry.focusSettling.interval = interval
            }
            do { try await p.run(); XCTFail("invalid policy must not enter native effect") }
            catch { XCTAssertEqual(error as? ControlRegistryFailure, .invalidRegistration) }
            await MainActor.run {
                XCTAssertEqual(p.focusCalls, 0)
                XCTAssertEqual(p.sleeps, 0)
                XCTAssertEqual(p.focusEvents.count, 0)
            }
        }
    }

    func testAsyncFocusAlreadyFocusedSucceedsWithoutSleeping() async throws {
        let p = try await MainActor.run { () -> FocusProbe in let p = try FocusProbe(settleAfter: nil); p.focused = true; return p }
        try await p.run()
        await MainActor.run { XCTAssertEqual(p.sleeps, 0); XCTAssertEqual(p.focusCalls, 1); XCTAssertEqual(p.focusEvents.count, 1) }
    }

    func testAsyncFocusThatNeverSettlesTimesOutBoundedWithoutSuccessOrEvent() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: nil) }
        do { try await p.run(); XCTFail("must not fabricate focus") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
        await MainActor.run {
            XCTAssertEqual(p.focusCalls, 1)
            XCTAssertEqual(p.focusEvents.count, 0)
            XCTAssertLessThanOrEqual(p.sleeps, 201)
            XCTAssertGreaterThanOrEqual(p.start.duration(to: p.now), .seconds(2))
            XCTAssertLessThanOrEqual(p.start.duration(to: p.now), .seconds(2) + .milliseconds(10))
            XCTAssertEqual(try? p.registry.focusedControl(), .null)
        }
    }

    func testAsyncFocusRechecksCustodyAfterEverySuspensionAndNeverEffectsTwice() async throws {
        enum Reason: CaseIterable { case route, disabled, hidden, window, authority, replacement, unmount, cancel, exhausted }
        for reason in Reason.allCases {
            let p = try await MainActor.run { try FocusProbe(settleAfter: 50, revision: reason == .exhausted ? UInt64.max - 2 : 0) }
            await MainActor.run {
                p.onSleep = {
                    guard p.sleeps == 1 else { return }
                    switch reason {
                    case .route: p.route = "audio"
                    case .disabled: p.enabled = false
                    case .hidden: p.visible = false
                    case .window: p.interaction = false
                    case .authority: p.allowed = false
                    case .replacement, .unmount:
                        try? p.registry.unmount(p.mount)
                        if reason == .replacement {
                            _ = try? p.registry.mount(id: "settings.search", window: "settings", route: "general", role: "text_field",
                                label: "Search", verbs: ["type_text"], value: { .map([.init("kind", .text("none"))]) },
                                focus: { p.replacementFocusCalls += 1; return true },
                                focused: { p.replacementWitnessReads += 1; return true }, focusSettles: true, invoke: { _ in })
                        }
                    case .cancel: withUnsafeCurrentTask { $0?.cancel() }
                    case .exhausted: try? p.registry.nativeStateChanged()
                    }
                }
            }
            let expected: ControlRegistryFailure
            switch reason {
            case .authority: expected = .permissionDenied
            case .replacement, .unmount: expected = .conflict
            case .exhausted: expected = .exhausted
            default: expected = .unavailable
            }
            let task = Task { @MainActor in try await p.run() }
            do { try await task.value; XCTFail("\(reason) must not succeed") }
            catch { XCTAssertEqual(error as? ControlRegistryFailure, expected, "\(reason)") }
            await MainActor.run {
                XCTAssertEqual(p.focusCalls, 1, "\(reason)")
                XCTAssertEqual(p.sleeps, 1, "\(reason): no resumption after refusal")
                XCTAssertEqual(p.focusEvents.count, 0, "\(reason)")
                XCTAssertEqual(p.replacementFocusCalls, 0, "\(reason)")
                XCTAssertEqual(p.replacementWitnessReads, 0, "\(reason): replacement never read")
            }
        }
    }

    func testAsyncFocusRefusedBeforeEffectNeverCallsNativeFocus() async throws {
        let denied = try await MainActor.run { try FocusProbe(settleAfter: 1) }
        await MainActor.run { denied.allowed = false }
        do { try await denied.run(); XCTFail() } catch { XCTAssertEqual(error as? ControlRegistryFailure, .permissionDenied) }
        let exhausted = try await MainActor.run { try FocusProbe(settleAfter: 1, revision: UInt64.max - 1) }
        do { try await exhausted.run(); XCTFail() } catch { XCTAssertEqual(error as? ControlRegistryFailure, .exhausted) }
        await MainActor.run { XCTAssertEqual(denied.focusCalls, 0); XCTAssertEqual(exhausted.focusCalls, 0) }
    }

    func testAsyncFocusToleratesUnrelatedRevisionChurnAndPublishesCurrentRevision() async throws {
        let p = try await MainActor.run { try FocusProbe(settleAfter: 3) }
        await MainActor.run { p.onSleep = { try? p.registry.nativeStateChanged() } }
        try await p.run()
        try await MainActor.run {
            XCTAssertEqual(p.focusEvents.count, 1)
            guard case .map(let f) = p.focusEvents[0], case .unsigned(let rev)? = f.first(where: { $0.key == "revision" })?.value else {
                return XCTFail("missing revision")
            }
            XCTAssertEqual(rev, p.registry.revision)
            XCTAssertEqual(p.focusCalls, 1)
        }
    }

    func testSyncFocusRefusesOptedInAsyncMountBeforeEffectAndOtherMountsUnchanged() async throws {
        try await MainActor.run {
            let p = try FocusProbe(settleAfter: nil)
            XCTAssertTrue(p.registry.isAsynchronousFocus(p.mount))
            XCTAssertThrowsError(try p.registry.setFocus(p.mount, authorized: { true })) {
                XCTAssertEqual($0 as? ControlRegistryFailure, .unsupported)
            }
            XCTAssertEqual(p.focusCalls, 0)
            XCTAssertEqual(p.focusEvents.count, 0)
            // Default registration stays synchronous and its API is unchanged.
            let plain = try FocusProbe(settleAfter: nil, settles: false)
            XCTAssertFalse(plain.registry.isAsynchronousFocus(plain.mount))
            plain.focused = true
            try plain.registry.setFocus(plain.mount, authorized: { true })
            XCTAssertEqual(plain.focusCalls, 1)
            XCTAssertEqual(plain.focusEvents.count, 1)
            // Opt-in without a complete witness is an invalid registration.
            XCTAssertThrowsError(try plain.registry.mount(id: "bad.focus", window: "settings", route: "general", role: "text_field",
                label: "x", verbs: ["type_text"], value: { .map([.init("kind", .text("none"))]) },
                focus: { true }, focused: nil, focusSettles: true, invoke: { _ in })) {
                XCTAssertEqual($0 as? ControlRegistryFailure, .invalidRegistration)
            }
            XCTAssertEqual(ControlRegistry.FocusSettling.live.timeout, .seconds(2))
        }
    }
}
