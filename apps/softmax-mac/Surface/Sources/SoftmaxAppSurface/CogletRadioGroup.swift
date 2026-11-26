#if canImport(AppKit) && canImport(SwiftUI)
import AppKit
import SwiftUI

/// A string picker whose visible controls ARE the registered native radio
/// buttons. IDs are supplied explicitly; labels never become semantic IDs.
/// The caller owns the title (including any labelsHidden presentation).
@available(macOS 14.0, *)
@MainActor
public struct CogletRadioGroup: NSViewRepresentable {
    public struct Choice: Equatable, Sendable {
        public let value: String
        public let label: String
        public let id: String
        public init(value: String, label: String, id: String) {
            self.value = value; self.label = label; self.id = id
        }
    }

    private let title: String, id: String, window: String, route: String
    private let registry: ControlRegistry
    private let choices: [Choice]
    private let fontSize: CGFloat
    @Binding private var selection: String
    private let failure: (ControlRegistryFailure) -> Void
    @Environment(\.isEnabled) private var enabled

    public init(_ title: String, id: String, registry: ControlRegistry, window: String, route: String,
                choices: [Choice], selection: Binding<String>, fontSize: CGFloat = 14,
                failure: @escaping (ControlRegistryFailure) -> Void) {
        self.title = title; self.id = id; self.registry = registry; self.window = window; self.route = route
        self.choices = choices; self._selection = selection; self.failure = failure
        self.fontSize = fontSize
    }
    public func makeNSView(context: Context) -> NativeView {
        let view = NativeView(frame: .zero)
        updateNSView(view, context: context)
        return view
    }
    public func updateNSView(_ view: NativeView, context: Context) {
        view.configure(title: title, id: id, registry: registry, window: window, route: route,
            choices: choices, selection: $selection, enabled: enabled, fontSize: fontSize, failure: failure)
    }
    public static func dismantleNSView(_ view: NativeView, coordinator: ()) { view.dismantle() }

    /// Public only because NSViewRepresentable exposes its native view type.
    /// Registration and lifetime management remain owned by the representable.
    @MainActor
    public final class NativeView: NSStackView {
        private struct Identity: Equatable {
            let title: String, id: String, window: String, route: String
            let choices: [Choice]
        }
        private var identity: Identity?
        private var registry: ControlRegistry?
        private var selection: Binding<String>?
        private var failure: (ControlRegistryFailure) -> Void = { _ in }
        private var enabled = true
        private var dismantled = false
        private var closed = false
        private var mountFailed = false
        private weak var mountedWindow: NSWindow?
        private var visibilityObservation: NSKeyValueObservation?
        private var parentMount: ControlRegistry.Mount?
        private var childMounts: [ControlRegistry.Mount] = []
        private var observedFocus: String?
        private(set) var buttons: [NSButton] = []

        public override init(frame frameRect: NSRect) {
            super.init(frame: frameRect)
            orientation = .vertical
            alignment = .leading
            spacing = 4
            setAccessibilityElement(true)
            setAccessibilityRole(.radioGroup)
        }
        public required init?(coder: NSCoder) { fatalError("Use init(frame:)") }

        func configure(title: String, id: String, registry: ControlRegistry, window: String, route: String,
                       choices: [Choice], selection: Binding<String>, enabled: Bool, fontSize: CGFloat = 14,
                       failure: @escaping (ControlRegistryFailure) -> Void) {
            let next = Identity(title: title, id: id, window: window, route: route, choices: choices)
            let replaced = identity != next || self.registry !== registry
            if replaced { retire() }
            self.identity = next; self.registry = registry; self.selection = selection
            self.enabled = enabled; self.failure = failure; dismantled = false; mountFailed = false
            // A fresh SwiftUI update can explicitly reopen the same native view.
            if self.window?.isVisible == true { closed = false }
            if replaced || buttons.isEmpty {
                for button in buttons { removeArrangedSubview(button); button.removeFromSuperview() }
                buttons = []
                setAccessibilityIdentifier(id)
                setAccessibilityLabel(title)
                do {
                    guard !choices.isEmpty, choices.count <= 127,
                          Set(choices.map(\.id)).count == choices.count,
                          Set(choices.map(\.value)).count == choices.count,
                          choices.allSatisfy({ $0.id.hasPrefix(id + ".") }) else {
                        throw ControlRegistryFailure.invalidRegistration
                    }
                    for choice in choices {
                        try CBORCodec.validate("app-selection", .text(choice.value))
                    }
                    for choice in choices {
                        let button = RadioButton(title: choice.label, target: self, action: #selector(choose(_:)))
                        button.setButtonType(.radio)
                        button.setAccessibilityIdentifier(choice.id)
                        button.setAccessibilityLabel(choice.label)
                        button.owner = self
                        buttons.append(button)
                        addArrangedSubview(button)
                    }
                } catch { report(error) }
                setAccessibilityChildren(buttons)
            }
            // Presentation changes retain native controls, focus and mounts.
            let pointSize = fontSize.isFinite && fontSize > 0 ? fontSize : 14
            for button in buttons { button.font = .systemFont(ofSize: pointSize) }
            invalidateIntrinsicContentSize()
            refreshButtons()
            synchronizePresentation()
            emitValues()
        }

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
                // AppKit window mutations and their KVO callbacks run on the
                // native main thread. Retire synchronously on orderOut, not in
                // a later Task that could race an immediate reopen.
                visibilityObservation = window.observe(\.isVisible, options: [.new]) { [weak self] _, change in
                    let visible = change.newValue == true
                    MainActor.assumeIsolated {
                        guard let self else { return }
                        self.closed = !visible
                        self.synchronizePresentation()
                    }
                }
                let center = NotificationCenter.default
                center.addObserver(self, selector: #selector(closePresentation(_:)), name: NSWindow.willCloseNotification, object: window)
                center.addObserver(self, selector: #selector(closePresentation(_:)), name: NSWindow.willMiniaturizeNotification, object: window)
                for name in [NSWindow.didDeminiaturizeNotification, NSWindow.didChangeOcclusionStateNotification,
                             NSWindow.didUpdateNotification] {
                    center.addObserver(self, selector: #selector(presentationChanged(_:)), name: name, object: window)
                }
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
            if notification.name == NSWindow.didDeminiaturizeNotification { closed = false }
            synchronizePresentation()
        }
        private func synchronizePresentation() {
            guard !dismantled, !closed, let native = window, native.isVisible, !native.isMiniaturized else {
                retire(); return
            }
            if mountedWindow !== native { retire() }
            guard !mountFailed, parentMount == nil, let identity, let registry,
                  buttons.count == identity.choices.count, !buttons.isEmpty else { return }
            do {
                guard selection != nil else { throw ControlRegistryFailure.unavailable }
                mountedWindow = native
                // Mount the parent FIRST. ControlRegistry validates ancestry by
                // ID; keep its exact Mount token for atomic rollback/retirement.
                parentMount = try registry.mount(id: identity.id, window: identity.window, route: identity.route,
                    role: "picker", label: identity.title, verbs: ["select"],
                    value: { [weak self] in
                        guard let self else { throw ControlRegistryFailure.unavailable }
                        return self.nativeSelectionValue()
                    }, enabled: { [weak self, weak native] in self?.isEnabled(in: native) == true },
                    visible: { [weak self, weak native] in self?.isVisible(in: native) == true },
                    invoke: { [weak self, weak native] args in
                        guard let self, self.isEnabled(in: native), self.isVisible(in: native),
                              case .map(let fields) = args,
                              let field = fields.first(where: { $0.key == "value" }), case .text(let value) = field.value,
                              let index = identity.choices.firstIndex(where: { $0.value == value }) else {
                            throw ControlRegistryFailure.unavailable
                        }
                        try self.press(self.buttons[index], in: native)
                    })
                for (index, choice) in identity.choices.enumerated() {
                    let button = buttons[index]
                    let mount = try registry.mount(id: choice.id, window: identity.window, route: identity.route,
                        parent: parentMount!.id, role: "button", label: choice.label, verbs: ["press"],
                        value: { [weak button] in
                            guard let button else { throw ControlRegistryFailure.unavailable }
                            return .map([.init("kind", .text("boolean")), .init("value", .boolean(button.state == .on))])
                        }, enabled: { [weak self, weak button, weak native] in
                            self?.isEnabled(in: native) == true && button?.isEnabled == true
                        }, visible: { [weak self, weak button, weak native] in
                            self?.isVisible(in: native) == true && button?.isHiddenOrHasHiddenAncestor == false
                        }, focus: { [weak self, weak button, weak native] in
                            guard let self, let button, let native, self.canPress(button, in: native) else { return false }
                            return native.makeFirstResponder(button)
                        }, focused: { [weak button, weak native] in
                            guard let button, let native else { return false }
                            return button.window === native && native.firstResponder === button
                        }, invoke: { [weak self, weak button, weak native] _ in
                            guard let self, let button else { throw ControlRegistryFailure.unavailable }
                            try self.press(button, in: native)
                        })
                    childMounts.append(mount)
                }
            } catch {
                // Never leave an actionable partial group or remove someone
                // else's duplicate. Only our exact parent token owns rollback.
                retire()
                mountFailed = true
                report(error)
            }
        }
        private func retire() {
            let token = parentMount
            parentMount = nil; childMounts = []; mountedWindow = nil
            if let token, let registry {
                do { try registry.unmount(token) } catch { report(error) }
            }
            // Remove our own entries before checking the global focus owner.
            publishFocus(nil)
        }
        // Only validated choice values enter the catalog. A persisted binding
        // outside those choices is an actual no-selection state, not a new
        // wire selection and not grounds to disable the human repair path.
        private func nativeSelectionValue() -> CBORValue {
            // Project the actual committed native state for BOTH parent and
            // children. A model change awaiting SwiftUI update still reports
            // the old visible selection, never a mixed parent/child snapshot.
            guard let identity, let index = buttons.firstIndex(where: { $0.state == .on }) else {
                return .map([.init("kind", .text("none"))])
            }
            return .map([.init("kind", .text("selection")), .init("value", .text(identity.choices[index].value))])
        }
        private func refreshButtons() {
            for (index, button) in buttons.enumerated() {
                button.isEnabled = enabled
                button.state = identity?.choices[index].value == selection?.wrappedValue ? .on : .off
            }
        }
        private func isEnabled(in native: NSWindow?) -> Bool {
            guard let native else { return false }
            return !dismantled && !closed && parentMount != nil && mountedWindow === native && window === native && enabled
        }
        private func isVisible(in native: NSWindow?) -> Bool {
            guard let native else { return false }
            return mountedWindow === native && window === native && native.isVisible
                && !native.isMiniaturized && !isHiddenOrHasHiddenAncestor
        }
        fileprivate func canPress(_ button: NSButton, in native: NSWindow?) -> Bool {
            !dismantled && !closed && window === native && native?.isVisible == true
                && native?.isMiniaturized == false && !isHiddenOrHasHiddenAncestor && enabled
                && button.window === native && button.isEnabled && !button.isHiddenOrHasHiddenAncestor
                && buttons.contains(where: { $0 === button })
        }
        private func press(_ button: NSButton, in native: NSWindow?) throws {
            guard canPress(button, in: native) else { throw ControlRegistryFailure.unavailable }
            button.performClick(nil)
        }
        @objc private func choose(_ sender: NSButton) {
            guard canPress(sender, in: window), let identity,
                  let index = buttons.firstIndex(where: { $0 === sender }) else { refreshButtons(); return }
            selection?.wrappedValue = identity.choices[index].value
            refreshButtons()
            emitValues()
            observeFocus()
        }
        private func emitValues() {
            guard let registry, let parentMount else { return }
            do {
                guard selection != nil else { throw ControlRegistryFailure.unavailable }
                try registry.controlChanged(parentMount)
                for mount in childMounts { try registry.controlChanged(mount) }
            } catch { retire(); report(error) }
        }
        fileprivate func observeFocus() {
            guard let identity, parentMount != nil else { return }
            let index = buttons.firstIndex { mountedWindow?.firstResponder === $0 }
            publishFocus(index.map { identity.choices[$0].id })
        }
        private func publishFocus(_ next: String?) {
            guard next != observedFocus, let registry, let identity else { return }
            do {
                // A group's resignation is not a global focus clear. Another
                // registered native control may already own firstResponder.
                if next == nil, case .map = try registry.focusedControl() {
                    observedFocus = nil
                    return
                }
                // Do not call synchronizeNativeState with a partial window
                // inventory: that would falsely close the host's other windows.
                try registry.nativeStateChanged()
                let payload = CBORValue.map([.init("revision", .unsigned(registry.revision)),
                    .init("window", .text(identity.window)), .init("control", next.map(CBORValue.text) ?? .null)])
                try CBORCodec.validate("app-focus-changed", payload)
                observedFocus = next
                registry.onChange?("softmax.app.focus_changed", payload)
            } catch { report(error) }
        }
        fileprivate func moveSelection(from button: NSButton, offset: Int) {
            guard let index = buttons.firstIndex(where: { $0 === button }), !buttons.isEmpty else { return }
            let next = buttons[(index + offset + buttons.count) % buttons.count]
            guard canPress(next, in: mountedWindow) else { return }
            _ = window?.makeFirstResponder(next)
            next.performClick(nil)
        }
        private func report(_ error: Error) { failure(error as? ControlRegistryFailure ?? .invalidRegistration) }
    }
}

@available(macOS 14.0, *)
@MainActor
private final class RadioButton: NSButton {
    weak var owner: CogletRadioGroup.NativeView?
    // App-owned native radio semantics must not depend on NSButton's cell
    // inference. AX reads the SAME live state that the target/action updates.
    override func accessibilityRole() -> NSAccessibility.Role? { .radioButton }
    override func isAccessibilityElement() -> Bool { true }
    // The NSButton owns this complete semantic control. Its AppKit cell is
    // implementation detail, not a second independently actionable radio.
    override func accessibilityChildren() -> [Any]? { [] }
    override func accessibilityValue() -> Any? { NSNumber(value: state == .on ? 1 : 0) }
    override var acceptsFirstResponder: Bool { isEnabled }
    override func performClick(_ sender: Any?) {
        guard owner?.canPress(self, in: window) == true else { return }
        super.performClick(sender)
    }
    override func accessibilityPerformPress() -> Bool {
        guard owner?.canPress(self, in: window) == true else { return false }
        performClick(nil)
        return true
    }
    override func keyDown(with event: NSEvent) {
        switch event.keyCode {
        case 123, 126: owner?.moveSelection(from: self, offset: -1)
        case 124, 125: owner?.moveSelection(from: self, offset: 1)
        case 49: performClick(nil)
        default: super.keyDown(with: event)
        }
    }
    override func becomeFirstResponder() -> Bool {
        let result = super.becomeFirstResponder()
        // AppKit commits firstResponder AFTER this callback returns.
        DispatchQueue.main.async { [weak self] in self?.owner?.observeFocus() }
        return result
    }
    override func resignFirstResponder() -> Bool {
        let result = super.resignFirstResponder()
        DispatchQueue.main.async { [weak self] in self?.owner?.observeFocus() }
        return result
    }
}
#endif
