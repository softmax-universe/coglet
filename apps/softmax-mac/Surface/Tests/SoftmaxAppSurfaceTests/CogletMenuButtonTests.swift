#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI
import XCTest
@testable import SoftmaxAppSurface

/// NSMenu tracking cannot be driven headlessly, so these tests call the
/// button's own NSMenuDelegate entry points and the NSMenuItem target/action
/// (`choose`) directly, in the order AppKit uses (close BEFORE the selected
/// item's action). Real tracking/Escape is covered only by native qualification.
@MainActor
final class CogletMenuButtonTests: XCTestCase {
    private final class Calls { var clear = 0, hide = 0 }
    private final class State {
        var allowed = true, interaction = true, route = "chat"
        var now = 0, deadline = 10
        var authorized: Bool { allowed && now < deadline }
        var opens = 0
    }
    private struct Fixture {
        let win: NSWindow, registry: ControlRegistry, view: CogletMenuButton.NativeView
        let calls: Calls, state: State
    }

    private func args(_ id: String) -> CBORValue {
        .map([.init("window", .text("chat")), .init("control", .text(id)), .init("verb", .text("press"))])
    }
    private func window() -> NSWindow {
        _ = NSApplication.shared
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 300, height: 100),
            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.orderFrontRegardless()
        return window
    }
    private func configure(_ view: CogletMenuButton.NativeView, registry: ControlRegistry, route: String = "chat",
                           calls: Calls, failure: ((ControlRegistryFailure) -> Void)? = nil) {
        view.configure(id: "chat.menu", label: "Conversation actions", symbol: "ellipsis.circle", registry: registry,
            window: "chat", route: route, items: [
                .init(id: "chat.clear_draft", title: "Clear draft") { calls.clear += 1 },
                .init(id: "chat.hide_orb", title: "Hide orb") { calls.hide += 1 }
            ], enabled: true, failure: failure ?? { XCTFail("Registration failed: \($0)") })
    }
    private func ids(_ registry: ControlRegistry) -> [String] { registry.registrationInventory().map(\.id) }
    private func fixture() -> Fixture {
        let state = State()
        let win = window()
        let registry = ControlRegistry(revision: 0, interactionAllowed: { [state] _ in state.interaction },
            currentRoute: { [state] _ in state.route })
        let view = CogletMenuButton.NativeView(frame: .zero, pullsDown: true)
        let calls = Calls()
        win.contentView = view
        configure(view, registry: registry, calls: calls)
        view.opener = { [state] _ in state.opens += 1 }
        return Fixture(win: win, registry: registry, view: view, calls: calls, state: state)
    }
    private func items(_ view: CogletMenuButton.NativeView) -> [NSMenuItem] {
        Array((view.menu?.items ?? []).dropFirst())
    }
    /// Opens, highlights `index`, closes (AppKit order: close before the action).
    private func openHighlightClose(_ f: Fixture, index: Int) throws -> NSMenuItem {
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let item = items(f.view)[index]
        f.view.menu(menu, willHighlight: item)
        f.view.menuDidClose(menu)
        return item
    }

    // MARK: presentation lifetime

    func testItemsExistOnlyBetweenNativeOpenAndClose() throws {
        let f = fixture(); defer { f.win.close() }
        XCTAssertEqual(ids(f.registry), ["chat.menu"])
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        XCTAssertEqual(ids(f.registry), ["chat.clear_draft", "chat.hide_orb", "chat.menu"])
        f.view.menuDidClose(menu)
        XCTAssertEqual(ids(f.registry), ["chat.menu"], "closed menu must retain no item registration")
        f.view.menuWillOpen(NSMenu())
        XCTAssertEqual(ids(f.registry), ["chat.menu"])
        f.view.dismantle()
        XCTAssertEqual(ids(f.registry), [])
    }

    func testItemPressUsesActionClosureAndOldGenerationIsFenced() throws {
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let old = try f.registry.prepare(args("chat.clear_draft"), sensitive: false)
        try f.registry.perform(old, authorized: { true })
        XCTAssertEqual(f.calls.clear, 1)
        f.view.menuDidClose(menu)
        XCTAssertThrowsError(try f.registry.perform(old, authorized: { true }))
        f.view.menuWillOpen(menu)
        XCTAssertThrowsError(try f.registry.perform(old, authorized: { true }))
        XCTAssertEqual(f.calls.clear, 1)
        try f.registry.perform(f.registry.prepare(args("chat.clear_draft"), sensitive: false), authorized: { true })
        XCTAssertEqual(f.calls.clear, 2)
        XCTAssertEqual(f.calls.hide, 0)
        f.view.menuDidClose(menu)
        f.view.dismantle()
    }

    func testWindowOrderOutAndRouteChangeRetireEverythingAndReopenRemountsOnce() throws {
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        XCTAssertEqual(ids(f.registry).count, 3)
        f.win.orderOut(nil)
        XCTAssertEqual(ids(f.registry), [], "orderOut during an open menu retires trigger and items")
        f.view.menuDidClose(menu)
        f.win.orderFrontRegardless()
        XCTAssertEqual(ids(f.registry), ["chat.menu"])
        configure(f.view, registry: f.registry, route: "voice", calls: f.calls)
        let entries = f.registry.registrationInventory()
        XCTAssertEqual(entries.map(\.id), ["chat.menu"])
        XCTAssertEqual(entries.map(\.route), ["voice"])
        f.view.dismantle()
        XCTAssertEqual(ids(f.registry), [])
    }

    func testNativeMenuHasRealItemsWithIdentifiersAndNoRegistrationWhenWindowless() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "chat" })
        let view = CogletMenuButton.NativeView(frame: .zero, pullsDown: true)
        configure(view, registry: registry, calls: Calls())
        XCTAssertFalse(view.isTriggerMounted)
        XCTAssertEqual(ids(registry), [])
        let menu = try XCTUnwrap(view.menu)
        XCTAssertEqual(menu.items.dropFirst().map(\.title), ["Clear draft", "Hide orb"])
        XCTAssertEqual(menu.items.dropFirst().map { $0.identifier?.rawValue }, ["chat.clear_draft", "chat.hide_orb"])
        view.menuWillOpen(menu)
        XCTAssertEqual(ids(registry), [], "no mount without a visible native window")
    }

    // MARK: trigger: native open only at the queued effect, under registry custody

    func testTriggerOpensOnlyAtQueuedEffectAndSuccessFollowsIt() async throws {
        let f = fixture(); defer { f.win.close() }
        var opensAtGate = -1
        f.view.presentationGate = { [state = f.state] in opensAtGate = state.opens }
        let request = try f.registry.prepare(args("chat.menu"), sensitive: false)
        XCTAssertEqual(f.state.opens, 0, "preparing must not open")
        try await f.registry.performAsync(request, authorized: { [state = f.state] in state.authorized })
        XCTAssertEqual(opensAtGate, 0, "queued, not yet opened, when the invocation suspends")
        XCTAssertEqual(f.state.opens, 1)
        for _ in 0..<20 { await Task.yield() }
        XCTAssertEqual(f.state.opens, 1, "a completed invocation leaves no later effect")
        // The synchronous path is not a way to open.
        XCTAssertThrowsError(try f.registry.perform(request, authorized: { true }))
        XCTAssertEqual(f.state.opens, 1)
    }

    private func assertQueuedRefusal(_ name: String, expected: ControlRegistryFailure? = nil,
                                     file: StaticString = #filePath, line: UInt = #line,
                                     mutate: @escaping (Fixture, Task<Void, Error>?) -> Void) async throws {
        let f = fixture(); defer { f.win.close() }
        let request = try f.registry.prepare(args("chat.menu"), sensitive: false)
        var task: Task<Void, Error>?
        f.view.presentationGate = { mutate(f, task) }
        let registry = f.registry, state = f.state
        task = Task { @MainActor in try await registry.performAsync(request, authorized: { state.authorized }) }
        do {
            try await task?.value
            XCTFail("\(name): must not succeed", file: file, line: line)
        } catch {
            if let expected { XCTAssertEqual(error as? ControlRegistryFailure, expected, name, file: file, line: line) }
        }
        XCTAssertEqual(f.state.opens, 0, "\(name): zero native presentations", file: file, line: line)
    }

    func testQueuedTriggerRefusedAtEffect() async throws {
        try await assertQueuedRefusal("revoked grant", expected: .permissionDenied) { f, _ in f.state.allowed = false }
        try await assertQueuedRefusal("deadline expiry", expected: .permissionDenied) { f, _ in f.state.now = f.state.deadline }
        try await assertQueuedRefusal("cancellation") { _, task in task?.cancel() }
        try await assertQueuedRefusal("interaction disallowed", expected: .unavailable) { f, _ in f.state.interaction = false }
        try await assertQueuedRefusal("route change", expected: .unavailable) { f, _ in f.state.route = "voice" }
        try await assertQueuedRefusal("orderOut") { f, _ in f.win.orderOut(nil) }
        try await assertQueuedRefusal("dismantle") { f, _ in f.view.dismantle() }
        try await assertQueuedRefusal("same-id successor", expected: .conflict) { f, _ in
            // Identity replacement retires the old mount and mounts a successor with the same id.
            f.state.route = "voice"
            self.configure(f.view, registry: f.registry, route: "voice", calls: f.calls)
            XCTAssertEqual(f.registry.registrationInventory().map(\.id), ["chat.menu"])
            XCTAssertEqual(f.registry.registrationInventory().map(\.route), ["voice"])
            XCTAssertTrue(f.view.isTriggerMounted)
        }
        try await assertQueuedRefusal("menu already open") { f, _ in
            if let menu = f.view.menu { f.view.menuWillOpen(menu) }
        }
    }

    // MARK: native selection: exact item + presentation, close-before-selection

    func testCloseBeforeSelectionExecutesOnceForTheSelectedItem() throws {
        let f = fixture(); defer { f.win.close() }
        let item = try openHighlightClose(f, index: 1)
        XCTAssertEqual(ids(f.registry), ["chat.menu"], "closed before the action arrives")
        f.view.choose(item)
        XCTAssertEqual(f.calls.hide, 1)
        f.view.choose(item)
        XCTAssertEqual(f.calls.hide, 1, "one-shot")
        XCTAssertEqual(f.calls.clear, 0)
    }

    func testSelectionWhileStillOpenExecutesOnce() throws {
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let item = items(f.view)[0]
        f.view.choose(item); f.view.choose(item); f.view.choose(items(f.view)[1])
        XCTAssertEqual(f.calls.clear, 1)
        XCTAssertEqual(f.calls.hide, 0, "one selection per presentation")
    }

    func testEscapeAndLateOrUnselectedItemsExecuteNothing() throws {
        let f = fixture(); defer { f.win.close() }
        // Closed without any highlighted item.
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let first = items(f.view)[0]
        f.view.menuDidClose(menu)
        f.view.choose(first)
        // Highlighted but dismissed: the action never arrives and the pending selection expires.
        let second = try openHighlightClose(f, index: 1)
        f.view.expirePendingSelection()
        f.view.choose(second)
        // Not the highlighted item.
        _ = try openHighlightClose(f, index: 0)
        f.view.choose(items(f.view)[1])
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)
    }

    func testForeignAndNeverMenuItemsAreRejected() throws {
        let f = fixture(); defer { f.win.close() }
        let other = fixture(); defer { other.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let foreign = NSMenuItem(title: "Clear draft", action: nil, keyEquivalent: "")
        foreign.tag = 0
        f.view.choose(foreign)
        f.view.choose(items(other.view)[0])
        let ghost = NSMenuItem(title: "Clear draft", action: nil, keyEquivalent: "")
        menu.addItem(ghost)  // in the menu, but not one of this presentation's items
        f.view.choose(ghost)
        f.view.menuDidClose(menu)
        f.view.choose(foreign); f.view.choose(items(other.view)[0])
        XCTAssertEqual(f.calls.clear + f.calls.hide + other.calls.clear + other.calls.hide, 0)
    }

    func testOldItemsAfterReopenOrPendingReopenAreRejectedAndNeverReachSuccessor() throws {
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let old = items(f.view)[0]
        f.view.menuDidClose(menu)
        f.view.menuWillOpen(menu)
        f.view.choose(old)
        XCTAssertEqual(f.calls.clear, 0, "an old item cannot act on the reopened presentation")
        f.view.menuDidClose(menu)
        // Pending from one presentation does not survive a reopen.
        let pendingItem = try openHighlightClose(f, index: 1)
        f.view.menuWillOpen(menu)
        f.view.choose(pendingItem)
        f.view.menuDidClose(menu)
        f.view.choose(pendingItem)
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)
    }

    func testReconfigureReplacementRejectsOldItemsAndSuccessorClosureIsNotInvoked() throws {
        let f = fixture(); defer { f.win.close() }
        let old = try openHighlightClose(f, index: 0)
        let successor = Calls()
        configure(f.view, registry: f.registry, route: "voice", calls: successor)
        f.view.choose(old)
        XCTAssertEqual(f.calls.clear, 0)
        XCTAssertEqual(successor.clear, 0, "no successor action reachable through the old item")
        // Same for an item saved while open.
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let openItem = items(f.view)[1]
        configure(f.view, registry: f.registry, route: "chat", calls: successor)
        f.view.choose(openItem)
        XCTAssertEqual(successor.hide + f.calls.hide, 0)
    }

    func testSameIdentityClosureRefreshStillWorks() throws {
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let item = items(f.view)[0]
        let refreshed = Calls()
        configure(f.view, registry: f.registry, calls: refreshed)
        XCTAssertTrue(f.view.isTriggerMounted)
        f.view.choose(item)
        XCTAssertEqual(refreshed.clear, 1, "ordinary binding update reaches the same item")
        XCTAssertEqual(f.calls.clear, 0)
    }

    func testOrderOutAndDismantleRejectPendingAndOpenSelections() throws {
        let f = fixture(); defer { f.win.close() }
        let pendingItem = try openHighlightClose(f, index: 0)
        f.win.orderOut(nil)
        f.view.choose(pendingItem)
        // Open, then orderOut mid-presentation.
        f.win.orderFrontRegardless()
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let openItem = items(f.view)[1]
        f.view.menu(menu, willHighlight: openItem)
        f.win.orderOut(nil)
        f.view.choose(openItem)
        f.view.menuDidClose(menu)
        f.view.choose(openItem)
        // Dismantle with a pending selection.
        f.win.orderFrontRegardless()
        let again = try openHighlightClose(f, index: 0)
        f.view.dismantle()
        f.view.choose(again)
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)
    }

    #if DEBUG
    // Temporary diagnostics must distinguish the two unproven native hypotheses
    // without changing either behavior. These are characterization, NOT native proof.
    func testDiagnosticDistinguishesNilHighlightFromPendingExpiry() async throws {
        CogletMenuButton.enableDiagnostics(true)
        defer { CogletMenuButton.enableDiagnostics(false) }
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        let item = items(f.view)[0]
        f.view.menu(menu, willHighlight: item)
        f.view.menu(menu, willHighlight: nil)
        f.view.menuDidClose(menu)
        f.view.choose(item)
        let nilEvents = CogletMenuButton.diagnosticSnapshot.events
        XCTAssertTrue(nilEvents.contains(.highlightOwned))
        XCTAssertTrue(nilEvents.contains(.highlightNil))
        XCTAssertTrue(nilEvents.contains(.closeWithoutCandidate))
        XCTAssertTrue(nilEvents.contains(.chooseNoPending))
        XCTAssertFalse(nilEvents.contains(.pendingCreated))
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)

        CogletMenuButton.enableDiagnostics(true)
        let pendingItem = try openHighlightClose(f, index: 0)
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.async { continuation.resume() }
        }
        f.view.choose(pendingItem)
        let expired = CogletMenuButton.diagnosticSnapshot.events
        let createdIndex = try XCTUnwrap(expired.firstIndex(of: .pendingCreated))
        let expiredIndex = try XCTUnwrap(expired.firstIndex(of: .pendingExpired))
        let chooseIndex = try XCTUnwrap(expired.firstIndex(of: .chooseEntered))
        XCTAssertLessThan(createdIndex, expiredIndex)
        XCTAssertLessThan(expiredIndex, chooseIndex)
        XCTAssertTrue(expired.contains(.chooseNoPending))
        XCTAssertFalse(expired.contains(.choosePending))
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)
        XCTAssertEqual(CogletMenuButton.diagnosticSnapshot.dropped, 0)
    }

    func testDiagnosticIsOptInClosedBoundedAndNeverSerializesNativeText() throws {
        CogletMenuButton.enableDiagnostics(false)
        defer { CogletMenuButton.enableDiagnostics(false) }
        let f = fixture(); defer { f.win.close() }
        let menu = try XCTUnwrap(f.view.menu)
        f.view.menuWillOpen(menu)
        XCTAssertTrue(CogletMenuButton.diagnosticSnapshot.events.isEmpty)
        f.view.menuDidClose(menu)
        CogletMenuButton.enableDiagnostics(true)
        f.view.menuWillOpen(menu)
        let foreign = NSMenuItem(title: "private-diagnostic-canary", action: nil, keyEquivalent: "")
        for _ in 0..<100 { f.view.choose(foreign) }
        let snapshot = CogletMenuButton.diagnosticSnapshot
        XCTAssertEqual(snapshot.events.count, 128)
        XCTAssertEqual(snapshot.dropped, 74) // open + clearPending + 100*(entry + refusal)
        XCTAssertEqual(snapshot.events.first, .open)
        XCTAssertTrue(snapshot.events.contains(.chooseWrongSender))
        let encoded = try JSONEncoder().encode(snapshot)
        let fields = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(Set(fields.keys), Set(["events", "dropped"]))
        XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains("private-diagnostic-canary"))
        XCTAssertLessThan(encoded.count, 4096)
        XCTAssertEqual(f.calls.clear + f.calls.hide, 0)
    }
    #endif

    func testGenerationOverflowFailsClosed() throws {
        var failures: [ControlRegistryFailure] = []
        let win = window(); defer { win.close() }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "chat" })
        let view = CogletMenuButton.NativeView(frame: .zero, pullsDown: true)
        let calls = Calls()
        win.contentView = view
        configure(view, registry: registry, calls: calls, failure: { failures.append($0) })
        let menu = try XCTUnwrap(view.menu)
        view.forceGenerationForTesting(UInt64.max)
        view.menuWillOpen(menu)
        XCTAssertEqual(failures.last, .exhausted)
        XCTAssertFalse(ids(registry).contains("chat.clear_draft"))
        for item in items(view) { view.choose(item) }
        view.menuDidClose(menu)
        for item in items(view) { view.choose(item) }
        XCTAssertEqual(calls.clear + calls.hide, 0)
    }
}
#endif
