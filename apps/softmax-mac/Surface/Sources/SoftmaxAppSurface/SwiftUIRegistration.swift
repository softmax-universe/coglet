#if canImport(SwiftUI)
import SwiftUI

/// Stable registration attached to the actual mounted SwiftUI control. Failure
/// leaves the control unregistered (and therefore adapter-inaccessible); the
/// independent native accessibility coverage test must catch it, not hide it.
@available(macOS 14.0, *)
@MainActor
private struct CogletRegistration: ViewModifier {
    let registry: ControlRegistry
    let id: String
    let window: String
    let route: String
    let role: String
    let label: String
    let sensitive: Bool
    let secure: Bool
    let verbs: Set<String>
    let value: () throws -> CBORValue
    let invoke: (CBORValue) throws -> Void
    let invokeAsync: ControlRegistry.AsyncInvocation?
    let focus: (() throws -> Bool)?
    let focused: (() -> Bool)?
    let failure: (ControlRegistryFailure) -> Void
    @Environment(\.isEnabled) private var enabled
    @State private var mount: ControlRegistry.Mount?

    func body(content: Content) -> some View {
        content.accessibilityIdentifier(id)
            .onAppear { register() }
            .onDisappear { unregister() }
            .onChange(of: enabled) { _, _ in unregister(); register() }
            .onChange(of: route) { _, _ in unregister(); register() }
            .onChange(of: label) { _, _ in unregister(); register() }
            .onChange(of: safeValue) { _, _ in
                guard let mount else { return }
                do { try registry.controlChanged(mount) }
                catch { failure(.invalidRegistration) }
            }
    }
    private var safeValue: CBORValue? {
        // In particular, SwiftUI change tracking must never inspect a secret.
        secure || sensitive ? .map([.init("kind", .text("redacted"))]) : try? value()
    }
    private func register() {
        guard mount == nil else { return }
        do {
            mount = try registry.mount(id: id, window: window, route: route, role: role, label: label,
                sensitive: sensitive, secure: secure, verbs: verbs, value: value,
                enabled: { enabled }, focus: focus, focused: focused, invokeAsync: invokeAsync, invoke: invoke)
        } catch let error as ControlRegistryFailure { failure(error) }
        catch { failure(.invalidRegistration) }
    }
    private func unregister() {
        guard let old = mount else { return }
        mount = nil
        do { try registry.unmount(old) }
        catch { failure(.invalidRegistration) }
    }
}

@available(macOS 14.0, *)
public extension View {
    @MainActor
    func coglet(id: String, registry: ControlRegistry, window: String, route: String,
                role: String, label: String, sensitive: Bool = false, secure: Bool = false,
                verbs: Set<String>, value: @escaping () throws -> CBORValue,
                focus: (() throws -> Bool)? = nil, focused: (() -> Bool)? = nil,
                invokeAsync: ControlRegistry.AsyncInvocation? = nil,
                failure: @escaping (ControlRegistryFailure) -> Void,
                invoke: @escaping (CBORValue) throws -> Void) -> some View {
        modifier(CogletRegistration(registry: registry, id: id, window: window, route: route,
            role: role, label: label, sensitive: sensitive, secure: secure, verbs: verbs,
            value: value, invoke: invoke, invokeAsync: invokeAsync, focus: focus, focused: focused, failure: failure))
    }
}

/// User press, keyboard shortcut and adapter invocation share exactly one closure.
@available(macOS 14.0, *)
@MainActor
public struct CogletButton: View {
    private let title: String
    private let id: String
    private let registry: ControlRegistry
    private let window: String
    private let route: String
    private let sensitive: Bool
    private let adapterSynchronous: Bool
    private let action: () -> Void
    private let failure: (ControlRegistryFailure) -> Void
    public init(_ title: String, id: String, registry: ControlRegistry, window: String, route: String,
                sensitive: Bool = false, adapterSynchronous: Bool = true, failure: @escaping (ControlRegistryFailure) -> Void,
                action: @escaping () -> Void) {
        self.title = title; self.id = id; self.registry = registry; self.window = window; self.route = route
        self.sensitive = sensitive; self.adapterSynchronous = adapterSynchronous
        self.failure = failure; self.action = action
    }
    public var body: some View {
        Button(title, action: action).coglet(id: id, registry: registry, window: window, route: route,
            role: "button", label: title, sensitive: sensitive, verbs: ["press"],
            value: { .map([.init("kind", .text("none"))]) }, failure: failure,
            invoke: { _ in
                // Pending native completion is not synchronous success. User
                // commands remain usable while their async adapter path is gated.
                guard adapterSynchronous else { throw ControlRegistryFailure.unsupported }
                action()
            })
    }
}

/// One completion-bearing command for both native user and adapter presses.
/// Every mutation after suspension must use the optional adapter effect context.
/// A user invocation has no adapter grant and passes nil, never a fabricated one.
@available(macOS 14.0, *)
@MainActor
public struct CogletAsyncButton: View {
    private let title: String, id: String, window: String, route: String
    private let registry: ControlRegistry
    private let sensitive: Bool
    private let failure: (ControlRegistryFailure) -> Void
    private let action: (ControlRegistry.EffectContext?) async throws -> Void
    private let userFailure: () -> Void
    public init(_ title: String, id: String, registry: ControlRegistry, window: String, route: String,
                sensitive: Bool = false, failure: @escaping (ControlRegistryFailure) -> Void,
                userFailure: @escaping () -> Void = {},
                action: @escaping (ControlRegistry.EffectContext?) async throws -> Void) {
        self.title = title; self.id = id; self.registry = registry; self.window = window; self.route = route
        self.sensitive = sensitive; self.failure = failure; self.userFailure = userFailure; self.action = action
    }
    public var body: some View {
        Button(title) { Task { do { try await action(nil) } catch { userFailure() } } }
            .coglet(id: id, registry: registry, window: window, route: route,
                role: "button", label: title, sensitive: sensitive, verbs: ["press"],
                value: { .map([.init("kind", .text("none"))]) },
                invokeAsync: { _, context in try await action(context) }, failure: failure,
                invoke: { _ in throw ControlRegistryFailure.unsupported })
    }
}

@available(macOS 14.0, *)
@MainActor
public struct CogletToggle: View {
    private let title: String
    private let id: String
    private let registry: ControlRegistry
    private let window: String
    private let route: String
    private let sensitive: Bool
    @Binding private var isOn: Bool
    private let failure: (ControlRegistryFailure) -> Void
    public init(_ title: String, id: String, registry: ControlRegistry, window: String, route: String,
                sensitive: Bool = false, isOn: Binding<Bool>,
                failure: @escaping (ControlRegistryFailure) -> Void) {
        self.title = title; self.id = id; self.registry = registry; self.window = window; self.route = route
        self.sensitive = sensitive; self._isOn = isOn; self.failure = failure
    }
    public var body: some View {
        Toggle(title, isOn: $isOn).coglet(id: id, registry: registry, window: window, route: route,
            role: "toggle", label: title, sensitive: sensitive, verbs: ["toggle"],
            value: { .map([.init("kind", .text("boolean")), .init("value", .boolean(isOn))]) },
            failure: failure, invoke: { args in
                guard case .map(let fields) = args,
                      let field = fields.first(where: { $0.key == "value" }), case .boolean(let newValue) = field.value else {
                    throw ControlRegistryFailure.unsupported
                }
                isOn = newValue
            })
    }
}
#endif
