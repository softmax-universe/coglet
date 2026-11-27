#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI

/// A pull-down menu whose visible control IS a native `NSPopUpButton` and whose
/// items are real `NSMenuItem`s. Registration lifetime follows the actual native
/// presentation instead of SwiftUI appearance callbacks (which are not delivered
/// for menu dismissal):
///   * the trigger is mounted while its window is visible and not miniaturized;
///   * item controls are mounted ONLY between `menuWillOpen` and `menuDidClose`
///     of this button's own menu (its NSMenuDelegate; no global notification);
///   * window close/miniaturize/orderOut, route/identity change and
///     `dismantleNSView` retire everything synchronously.
/// User clicks and adapter presses run the SAME item action closure. Every
/// item invocation is fenced by the registry `Mount` token (owner + route +
/// incarnation) plus this open-generation, so a request prepared for an old
/// menu presentation can never act on a later one.
/// Native selection is bound to the exact `NSMenuItem` object of ONE
/// presentation (items are rebuilt on every open): a sender is never resolved
/// through its tag/index. AppKit may deliver `menuDidClose` BEFORE the chosen
/// item's action, so close hands the highlighted item to a one-shot pending
/// selection (immutable action snapshot) that expires on the next main turn and
/// is cleared by reopen, replacement, retirement and dismantle.
/// The trigger runs as an async registry invocation: the native open happens
/// inside `EffectContext.perform` at the queued turn, never as a synchronous
/// success.
@available(macOS 14.0, *)
@MainActor
public struct CogletMenuButton: NSViewRepresentable {
    public struct Item {
        public let id: String
        public let title: String
        public let action: () -> Void
        public init(id: String, title: String, action: @escaping () -> Void) {
            self.id = id; self.title = title; self.action = action
        }
    }

    private let id: String, label: String, symbol: String, window: String, route: String
    private let registry: ControlRegistry
    private let items: [Item]
    private let failure: (ControlRegistryFailure) -> Void
    @Environment(\.isEnabled) private var enabled

    public init(id: String, label: String, symbol: String, registry: ControlRegistry, window: String,
                route: String, items: [Item], failure: @escaping (ControlRegistryFailure) -> Void) {
        self.id = id; self.label = label; self.symbol = symbol; self.registry = registry
        self.window = window; self.route = route; self.items = items; self.failure = failure
    }
    #if DEBUG
    /// Temporary native-order measurement, NOT an adapter observation. No input
    /// strings, ids, labels, values, timestamps or object addresses are accepted.
    /// Enabled only after the app validates its existing private UI-test probe.
    public enum DiagnosticEvent: String, Codable {
        case open, highlightOwned, highlightNil, highlightForeign
        case closeWithCandidate, closeWithoutCandidate, pendingCreated, pendingExpired
        case chooseEntered, chooseWrongSender, chooseOpen, chooseNoPending
        case chooseWrongPending, chooseWrongBox, choosePending
        case clearPending, retire, reconfigureSame, reconfigureReplacement
    }
    public struct DiagnosticSnapshot: Encodable {
        public let events: [DiagnosticEvent]
        public let dropped: Int
    }
    private static var diagnosticEnabled = false
    private static var diagnosticEvents: [DiagnosticEvent] = []
    private static var diagnosticDropped = 0
    public static func enableDiagnostics(_ enabled: Bool) {
        diagnosticEnabled = enabled
        diagnosticEvents = []; diagnosticDropped = 0
    }
    public static var diagnosticSnapshot: DiagnosticSnapshot {
        DiagnosticSnapshot(events: diagnosticEvents, dropped: diagnosticDropped)
    }
    private static func record(_ event: DiagnosticEvent) {
        guard diagnosticEnabled else { return }
        // Ignore startup/configuration churn before any menu presentation.
        guard !diagnosticEvents.isEmpty || event == .open else { return }
        // Prefix, never eviction: preserve the first causal sequence. Saturate
        // the dropped counter too; observers can detect incomplete evidence.
        guard diagnosticEvents.count < 128 else {
            if diagnosticDropped < Int.max { diagnosticDropped += 1 }
            return
        }
        diagnosticEvents.append(event)
    }
    #endif

    public func makeNSView(context: Context) -> NativeView {
        let view = NativeView(frame: .zero, pullsDown: true)
        updateNSView(view, context: context)
        return view
    }
    public func updateNSView(_ view: NativeView, context: Context) {
        view.configure(id: id, label: label, symbol: symbol, registry: registry, window: window,
            route: route, items: items, enabled: enabled, failure: failure)
    }
    public static func dismantleNSView(_ view: NativeView, coordinator: ()) { view.dismantle() }

    /// Public only because NSViewRepresentable exposes its native view type.
    @MainActor
    public final class NativeView: NSPopUpButton, NSMenuDelegate {
        private struct Identity: Equatable {
            let id: String, label: String, symbol: String, window: String, route: String
            let itemIDs: [String], titles: [String]
        }
        private var identity: Identity?
        private var registry: ControlRegistry?
        /// One native item for exactly one presentation.
        final class ItemBox {
            let item: NSMenuItem
            let generation: UInt64
            var action: () -> Void
            init(item: NSMenuItem, generation: UInt64, action: @escaping () -> Void) {
                self.item = item; self.generation = generation; self.action = action
            }
        }
        private struct PendingSelection {
            let box: ItemBox
            let action: () -> Void
            let token: UInt64
        }
        private var latestActions: [() -> Void] = []
        private var boxes: [ItemBox] = []
        private var pending: PendingSelection?
        private var pendingToken: UInt64 = 0
        private weak var highlightedMenuItem: NSMenuItem?
        private var selectionConsumed = false
        private var generationExhausted = false
        /// Test seams: the native open and the suspension before it.
        var opener: @MainActor (NativeView) -> Void = { $0.performClick(nil) }
        var presentationGate: @MainActor () async -> Void = {
            await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
                DispatchQueue.main.async { continuation.resume() }
            }
        }
        private var failure: (ControlRegistryFailure) -> Void = { _ in }
        private var menuEnabled = true
        private var dismantled = false
        private var closed = false
        private var mountFailed = false
        private weak var mountedWindow: NSWindow?
        private var visibilityObservation: NSKeyValueObservation?
        private var triggerMount: ControlRegistry.Mount?
        private var itemMounts: [ControlRegistry.Mount] = []
        private var menuOpen = false
        /// Incremented for every native open; item closures capture their value.
        private var openGeneration: UInt64 = 0

        public override init(frame: NSRect, pullsDown flag: Bool) {
            // Always a pull-down: the title item carries the visible image.
            super.init(frame: frame, pullsDown: true)
            commonInit()
        }
        public required init?(coder: NSCoder) { fatalError("Use init(frame:pullsDown:)") }
        private func commonInit() {
            bezelStyle = .rounded
            imagePosition = .imageOnly
            menu?.delegate = self
        }

        func configure(id: String, label: String, symbol: String, registry: ControlRegistry, window: String,
                       route: String, items: [CogletMenuButton.Item], enabled: Bool,
                       failure: @escaping (ControlRegistryFailure) -> Void) {
            let next = Identity(id: id, label: label, symbol: symbol, window: window, route: route,
                itemIDs: items.map(\.id), titles: items.map(\.title))
            let replaced = identity != next || self.registry !== registry
            #if DEBUG
            if menuOpen || pending != nil {
                CogletMenuButton.record(replaced ? .reconfigureReplacement : .reconfigureSame)
            }
            #endif
            if replaced { retire() }
            self.identity = next; self.registry = registry; self.failure = failure
            self.menuEnabled = enabled; dismantled = false; mountFailed = false
            // Same identity: refresh the closures of the exact current boxes only
            // (never by a sender's tag). A replaced identity was retired above.
            latestActions = items.map(\.action)
            if !replaced, boxes.count == latestActions.count {
                for (box, action) in zip(boxes, latestActions) { box.action = action }
            }
            isEnabled = enabled
            if self.window?.isVisible == true { closed = false }
            if replaced || menu?.items.count != items.count + 1 { rebuildMenu(next) }
            synchronizePresentation()
        }

        private func rebuildMenu(_ next: Identity) {
            setAccessibilityIdentifier(next.id)
            setAccessibilityLabel(next.label)
            guard let menu else { return }
            menu.removeAllItems()
            boxes = []; pending = nil; highlightedMenuItem = nil
            menu.delegate = self
            // Pull-down title item: carries the visible image only.
            let head = NSMenuItem(title: "", action: nil, keyEquivalent: "")
            head.image = NSImage(systemSymbolName: next.symbol, accessibilityDescription: next.label)
            menu.addItem(head)
            for (index, title) in next.titles.enumerated() {
                let item = NSMenuItem(title: title, action: #selector(choose(_:)), keyEquivalent: "")
                item.target = self
                item.identifier = NSUserInterfaceItemIdentifier(next.itemIDs[index])
                // NSMenuItem AX identifier support is runtime-checked, not assumed.
                let setter = NSSelectorFromString("setAccessibilityIdentifier:")
                if item.responds(to: setter) { _ = item.perform(setter, with: next.itemIDs[index]) }
                menu.addItem(item)
                boxes.append(ItemBox(item: item, generation: openGeneration,
                    action: index < latestActions.count ? latestActions[index] : {}))
            }
            invalidateIntrinsicContentSize()
        }

        /// Real user selection (NSMenuItem target/action). Accepted only for the
        /// exact item object of the live presentation, or the one-shot pending
        /// selection of the presentation that just closed.
        @objc func choose(_ sender: NSMenuItem) {
            #if DEBUG
            CogletMenuButton.record(.chooseEntered)
            if generationExhausted || dismantled || menu == nil || sender.menu !== menu
                || menu?.items.contains(where: { $0 === sender }) != true {
                CogletMenuButton.record(.chooseWrongSender)
            }
            #endif
            guard !generationExhausted, !dismantled, let own = menu,
                  sender.menu === own, own.items.contains(where: { $0 === sender }) else { return }
            if menuOpen, !selectionConsumed, let box = boxes.first(where: { $0.item === sender }),
               box.generation == openGeneration, isOperable(in: mountedWindow) {
                #if DEBUG
                CogletMenuButton.record(.chooseOpen)
                #endif
                selectionConsumed = true
                box.action()
                return
            }
            #if DEBUG
            if pending == nil { CogletMenuButton.record(.chooseNoPending) }
            else if pending?.box.item !== sender || menuOpen { CogletMenuButton.record(.chooseWrongPending) }
            else if !boxes.contains(where: { $0 === pending?.box }) { CogletMenuButton.record(.chooseWrongBox) }
            #endif
            guard let selected = pending, selected.box.item === sender, !menuOpen,
                  boxes.contains(where: { $0 === selected.box }) else { return }
            #if DEBUG
            CogletMenuButton.record(.choosePending)
            #endif
            pending = nil
            selected.action()
        }
        private func advanceGeneration() {
            guard openGeneration < UInt64.max else { generationExhausted = true; return }
            openGeneration += 1
        }
        private func clearPending() {
            #if DEBUG
            CogletMenuButton.record(.clearPending)
            #endif
            pending = nil; highlightedMenuItem = nil; pendingToken &+= 1
        }
        func expirePendingSelection() { clearPending() }

        // MARK: Native presentation lifetime

        public override func viewWillMove(toWindow newWindow: NSWindow?) {
            if window !== newWindow {
                retire()
                visibilityObservation?.invalidate(); visibilityObservation = nil
                NotificationCenter.default.removeObserver(self)
                closed = false
            }
            super.viewWillMove(toWindow: newWindow)
        }
        public override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            if let window {
                visibilityObservation = window.observe(\.isVisible, options: [.new]) { [weak self] _, change in
                    let visible = change.newValue == true
                    MainActor.assumeIsolated {
                        guard let self else { return }
                        self.closed = !visible
                        self.synchronizePresentation()
                    }
                }
                let center = NotificationCenter.default
                center.addObserver(self, selector: #selector(closePresentation(_:)),
                    name: NSWindow.willCloseNotification, object: window)
                center.addObserver(self, selector: #selector(closePresentation(_:)),
                    name: NSWindow.willMiniaturizeNotification, object: window)
                center.addObserver(self, selector: #selector(presentationChanged(_:)),
                    name: NSWindow.didDeminiaturizeNotification, object: window)
            }
            synchronizePresentation()
        }
        func dismantle() {
            dismantled = true
            retire()
            visibilityObservation?.invalidate(); visibilityObservation = nil
            NotificationCenter.default.removeObserver(self)
        }
        @objc private func closePresentation(_ notification: Notification) {
            closed = true
            retire()
        }
        @objc private func presentationChanged(_ notification: Notification) {
            closed = false
            synchronizePresentation()
        }

        private func synchronizePresentation() {
            guard !dismantled, !closed, let native = window, native.isVisible, !native.isMiniaturized else {
                retire(); return
            }
            if mountedWindow !== native { retire() }
            guard !generationExhausted else {
                if !mountFailed { mountFailed = true; report(ControlRegistryFailure.exhausted) }
                return
            }
            guard !mountFailed, triggerMount == nil, let identity, let registry else { return }
            do {
                mountedWindow = native
                triggerMount = try registry.mount(id: identity.id, window: identity.window, route: identity.route,
                    role: "button", label: identity.label, verbs: ["press"],
                    value: { .map([.init("kind", .text("none"))]) },
                    enabled: { [weak self, weak native] in self?.isOperable(in: native) == true },
                    visible: { [weak self, weak native] in self?.isShown(in: native) == true },
                    invokeAsync: { [weak self, weak native] _, context in
                        guard let self, self.isOperable(in: native), !self.menuOpen,
                              let mount = self.triggerMount else { throw ControlRegistryFailure.unavailable }
                        // The native open is queued, and the invocation stays live
                        // until it executes: authority/deadline/route are rechecked
                        // by the registry context at the actual effect.
                        await self.presentationGate()
                        try Task.checkCancellation()
                        try context.perform {
                            guard self.triggerMount == mount, self.isOperable(in: native), !self.menuOpen,
                                  !self.generationExhausted else { throw ControlRegistryFailure.unavailable }
                            self.opener(self)
                        }
                    }, invoke: { _ in throw ControlRegistryFailure.unsupported })
            } catch {
                retire()
                mountFailed = true
                report(error)
            }
        }

        /// Exact retirement: the trigger token's unmount removes children too,
        /// but each token is also unmounted individually so a duplicate owner's
        /// entries are never touched.
        private func retire() {
            #if DEBUG
            CogletMenuButton.record(.retire)
            #endif
            let tokens = itemMounts + (triggerMount.map { [$0] } ?? [])
            itemMounts = []; triggerMount = nil; mountedWindow = nil
            let wasOpen = menuOpen
            menuOpen = false
            clearPending()
            advanceGeneration()
            if wasOpen { menu?.cancelTracking() }
            guard let registry else { return }
            for token in tokens {
                do { try registry.unmount(token) } catch { report(error) }
            }
        }
        private func retireItems() {
            let tokens = itemMounts
            itemMounts = []
            advanceGeneration()
            guard let registry else { return }
            for token in tokens {
                do { try registry.unmount(token) } catch { report(error) }
            }
        }
        private func isOperable(in native: NSWindow?) -> Bool {
            guard let native else { return false }
            return !dismantled && !closed && triggerMount != nil && mountedWindow === native
                && window === native && menuEnabled && isEnabled && native.isVisible && !native.isMiniaturized
                && !isHiddenOrHasHiddenAncestor
        }
        private func isShown(in native: NSWindow?) -> Bool {
            guard let native else { return false }
            return mountedWindow === native && window === native && native.isVisible
                && !native.isMiniaturized && !isHiddenOrHasHiddenAncestor
        }

        // MARK: NSMenuDelegate (this button's own menu only)

        public func menuWillOpen(_ menu: NSMenu) {
            guard menu === self.menu else { return }
            #if DEBUG
            CogletMenuButton.record(.open)
            #endif
            clearPending()
            selectionConsumed = false
            retireItems()
            guard !generationExhausted else {
                self.menu?.cancelTracking(); report(ControlRegistryFailure.exhausted); return
            }
            // Fresh native items for this presentation; older items can never match.
            if let identity { rebuildMenu(identity) }
            guard let identity, let registry, let parent = triggerMount, let native = mountedWindow,
                  isOperable(in: native) else { return }
            menuOpen = true
            let generation = openGeneration
            do {
                for (index, itemID) in identity.itemIDs.enumerated() {
                    let box = boxes[index]
                    let mount = try registry.mount(id: itemID, window: identity.window, route: identity.route,
                        parent: parent.id, role: "button", label: identity.titles[index], verbs: ["press"],
                        value: { .map([.init("kind", .text("none"))]) },
                        enabled: { [weak self, weak native] in
                            self?.itemLive(generation, parent: parent, in: native) == true
                        }, visible: { [weak self, weak native] in
                            self?.itemLive(generation, parent: parent, in: native) == true
                        }, invoke: { [weak self, weak native] _ in
                            guard let self, self.itemLive(generation, parent: parent, in: native),
                                  !self.selectionConsumed else {
                                throw ControlRegistryFailure.unavailable
                            }
                            self.selectionConsumed = true
                            box.action()
                            // Dismiss like a real selection; menuDidClose retires.
                            self.menu?.cancelTracking()
                        })
                    itemMounts.append(mount)
                }
            } catch {
                retireItems()
                menuOpen = false
                report(error)
            }
        }
        public func menu(_ menu: NSMenu, willHighlight item: NSMenuItem?) {
            guard menu === self.menu else { return }
            #if DEBUG
            CogletMenuButton.record(item == nil ? .highlightNil
                : boxes.contains(where: { $0.item === item }) ? .highlightOwned : .highlightForeign)
            #endif
            highlightedMenuItem = item
        }
        public func menuDidClose(_ menu: NSMenu) {
            guard menu === self.menu else { return }
            let wasOpen = menuOpen
            menuOpen = false
            var candidate: ItemBox?
            if wasOpen, !selectionConsumed, !generationExhausted, let item = highlightedMenuItem,
               let box = boxes.first(where: { $0.item === item }), box.generation == openGeneration {
                candidate = box
            }
            #if DEBUG
            CogletMenuButton.record(candidate == nil ? .closeWithoutCandidate : .closeWithCandidate)
            #endif
            highlightedMenuItem = nil
            retireItems()
            guard let box = candidate, !generationExhausted else { return }
            pendingToken &+= 1
            let token = pendingToken
            pending = PendingSelection(box: box, action: box.action, token: token)
            #if DEBUG
            CogletMenuButton.record(.pendingCreated)
            #endif
            // AppKit sends the item action in the same turn as the close; anything
            // later (Escape, stale item) is expired.
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated {
                    guard let self, self.pending?.token == token else { return }
                    #if DEBUG
                    CogletMenuButton.record(.pendingExpired)
                    #endif
                    self.pending = nil
                }
            }
        }
        private func itemLive(_ generation: UInt64, parent: ControlRegistry.Mount, in native: NSWindow?) -> Bool {
            !generationExhausted && menuOpen && openGeneration == generation && triggerMount == parent
                && isOperable(in: native)
        }

        // Test seams (no NSMenu tracking can be driven headlessly).
        var mountedItemIDs: [String] { itemMounts.map(\.id) }
        var isTriggerMounted: Bool { triggerMount != nil }
        func forceGenerationForTesting(_ value: UInt64) { openGeneration = value }

        private func report(_ error: Error) { failure(error as? ControlRegistryFailure ?? .invalidRegistration) }
    }
}
#endif
