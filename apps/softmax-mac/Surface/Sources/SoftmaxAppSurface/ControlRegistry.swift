import Foundation

/// Native registration failures contain no user values or provider diagnostics.
public enum ControlRegistryFailure: Swift.Error, Equatable {
    case invalidRegistration, duplicate, unavailable, conflict, permissionDenied, unsupported, exhausted
}

/// Main-actor native command directory. This is not grant authority: the host
/// supplies an effect-time authorization check after SDK durable admission.
/// Suspended commands receive a context that rechecks custody at EVERY effect.
@MainActor
public final class ControlRegistry {
    /// Product commands must place every deferred mutation inside perform. The
    /// callback is synchronous: no suspension between custody check and effect.
    /// This fences participating commands, not arbitrary code outside the context.
    public final class EffectContext {
        private let check: () throws -> Void
        fileprivate init(check: @escaping () throws -> Void) { self.check = check }
        public func perform<T>(_ effect: () throws -> T) throws -> T {
            try check()
            return try effect()
        }
    }
    public typealias AsyncInvocation = @MainActor (CBORValue, EffectContext) async throws -> Void
    public struct Mount: Equatable, Sendable {
        fileprivate let registry: UUID
        fileprivate let incarnation: UInt64
        public let id: String
    }
    public struct Request: Sendable {
        fileprivate let mount: Mount
        fileprivate let args: CBORValue
        fileprivate let sensitive: Bool
    }
    /// Immutable, value-free mount directory copied to the runtime control owner.
    /// Preparing from it performs no MainActor hop, native getter, ledger I/O or
    /// effect. The returned request still needs SDK durable admission and a live
    /// effect-time grant/deadline check before `perform` on the MainActor.
    public struct CapturedDirectory: Sendable {
        fileprivate struct Target: Sendable {
            let mount: Mount
            let window: String
            let sensitive: Bool
        }
        fileprivate let targets: [String: Target]
        public let revision: UInt64

        public func prepare(_ args: CBORValue, sensitive: Bool) throws -> Request {
            try CBORCodec.validate("app-invoke-args", args)
            let fields = try args.requireMap()
            guard let control = fields.first(where: { $0.key == "control" }), case .text(let id) = control.value,
                  let window = fields.first(where: { $0.key == "window" }), case .text(let name) = window.value,
                  let target = targets[id], target.window == name else { throw ControlRegistryFailure.unavailable }
            guard target.sensitive == sensitive else { throw ControlRegistryFailure.permissionDenied }
            return Request(mount: target.mount, args: args, sensitive: sensitive)
        }
        public func prepareFocus(_ args: CBORValue) throws -> Mount {
            try CBORCodec.validate("app-focus-args", args)
            let fields = try args.requireMap()
            guard let control = fields.first(where: { $0.key == "control" }), case .text(let id) = control.value,
                  let window = fields.first(where: { $0.key == "window" }), case .text(let name) = window.value,
                  let target = targets[id], target.window == name else { throw ControlRegistryFailure.unavailable }
            return target.mount
        }
    }

    /// Value-free native membership evidence for UI catalog audits. This is not
    /// a wire model, observation channel or grant. It deliberately omits labels
    /// and values, and never invokes value/focus/effect callbacks. Disabled and
    /// off-route mounts remain present so an audit cannot hide missing controls.
    public struct Registration: Codable, Equatable, Sendable {
        public let id: String
        public let window: String
        public let route: String
        public let role: String
        public let verbs: [String]
        public let sensitive: Bool
        public let enabled: Bool
        public let visible: Bool
    }

    public func registrationInventory() -> [Registration] {
        entries.values.sorted { $0.mount.id < $1.mount.id }.map {
            Registration(id: $0.mount.id, window: $0.window, route: $0.route,
                role: $0.role, verbs: $0.verbs.sorted(), sensitive: $0.sensitive,
                enabled: $0.enabled(), visible: $0.visible())
        }
    }

    public func captureDirectory() -> CapturedDirectory {
        CapturedDirectory(targets: Dictionary(uniqueKeysWithValues: entries.values.map {
            ($0.mount.id, CapturedDirectory.Target(mount: $0.mount, window: $0.window, sensitive: $0.sensitive))
        }), revision: revision)
    }

    private struct Entry {
        let mount: Mount
        let window: String
        let route: String
        let parent: String?
        let role: String
        let label: String
        let sensitive: Bool
        let secure: Bool
        let verbs: Set<String>
        let value: () throws -> CBORValue
        let enabled: () -> Bool
        let visible: () -> Bool
        let invoke: (CBORValue) throws -> Void
        let invokeAsync: AsyncInvocation?
        let focus: (() throws -> Bool)?
        let focused: (() -> Bool)?
    }
    private struct Cursor { let revision: UInt64; let window: String; let offset: Int }
    private let identity = UUID()
    private var incarnation: UInt64 = 0
    private var entries: [String: Entry] = [:]
    private var lastValues: [String: CBORValue] = [:]
    /// Previous safe observations, never authority or a second navigation model.
    private var observedWindows: [String: String] = [:]
    private var observedFocus: (window: String, control: String)?
    private var cursors: [String: Cursor] = [:]
    /// Seed from the host's durable source incarnation; exhaustion fences changes.
    public private(set) var revision: UInt64
    public var onChange: ((String, CBORValue) -> Void)?
    /// Returns the ACTUAL open route, not adapter-owned shadow navigation.
    private let currentRoute: (String) -> String?
    private let interactionAllowed: (String) -> Bool

    public init(revision: UInt64, interactionAllowed: @escaping (String) -> Bool = { _ in true },
                currentRoute: @escaping (String) -> String?) {
        self.revision = revision
        self.currentRoute = currentRoute
        self.interactionAllowed = interactionAllowed
    }

    @discardableResult
    public func mount(id: String, window: String, route: String, parent: String? = nil,
                      role: String, label: String, sensitive: Bool = false, secure: Bool = false,
                      verbs: Set<String>, value: @escaping () throws -> CBORValue,
                      enabled: @escaping () -> Bool = { true }, visible: @escaping () -> Bool = { true },
                      focus: (() throws -> Bool)? = nil, focused: (() -> Bool)? = nil,
                      invokeAsync: AsyncInvocation? = nil,
                      invoke: @escaping (CBORValue) throws -> Void) throws -> Mount {
        // ASCII semantic ids eliminate Swift normalized-key aliases, values and labels
        // still preserve exact arbitrary UTF-8 under their generated byte limits.
        guard validID(id), parent.map(validID) ?? true,
              verbs.isSubset(of: ["press", "toggle", "select", "set_value", "type_text"]),
              !verbs.isEmpty, entries.count < 4096 else { throw ControlRegistryFailure.invalidRegistration }
        guard entries[id] == nil else { throw ControlRegistryFailure.duplicate }
        try CBORCodec.validate("app-location", object(["window": .text(window), "route": .text(route)]))
        var depth = 1
        var ancestor = parent
        while let next = ancestor {
            guard let entry = entries[next], entry.window == window, entry.route == route else {
                throw ControlRegistryFailure.invalidRegistration
            }
            depth += 1
            guard depth <= 32 else { throw ControlRegistryFailure.invalidRegistration }
            ancestor = entry.parent
        }
        // Validate metadata without ever invoking a secure-value getter. The safe
        // label is a product constant; callers must not interpolate secret material.
        let safeValue = secure || sensitive ? object(["kind": .text("redacted")]) : try value()
        try CBORCodec.validate("app-control", object([
            "id": .text(id), "parent": parent.map(CBORValue.text) ?? .null, "window": .text(window),
            "role": .text(role), "label": .text(label), "value": safeValue,
            "enabled": .boolean(enabled()), "visible": .boolean(visible()), "sensitive": .boolean(sensitive)
        ]))
        guard incarnation < UInt64.max else { throw ControlRegistryFailure.exhausted }
        try changed()
        incarnation += 1
        let mount = Mount(registry: identity, incarnation: incarnation, id: id)
        entries[id] = Entry(mount: mount, window: window, route: route, parent: parent, role: role,
                            label: label, sensitive: sensitive, secure: secure, verbs: verbs,
                            value: value, enabled: enabled, visible: visible, invoke: invoke,
                            invokeAsync: invokeAsync,
                            focus: focus, focused: focused)
        lastValues[id] = safeValue
        return mount
    }

    /// An old view's teardown cannot unregister a successor using the same id.
    public func unmount(_ mount: Mount) throws {
        guard let entry = entries[mount.id], entry.mount == mount else { return }
        var removed = Set([mount.id])
        var previous = -1
        while previous != removed.count {
            previous = removed.count
            for item in entries.values where item.parent.map(removed.contains) == true { removed.insert(item.mount.id) }
        }
        try changed()
        for id in removed { entries.removeValue(forKey: id); lastValues.removeValue(forKey: id) }
    }

    public func prepare(_ args: CBORValue, sensitive: Bool) throws -> Request {
        try CBORCodec.validate("app-invoke-args", args)
        let fields = try args.requireMap()
        let id = try text(fields, "control"), window = try text(fields, "window")
        guard let entry = entries[id], entry.window == window else { throw ControlRegistryFailure.unavailable }
        guard entry.sensitive == sensitive else { throw ControlRegistryFailure.permissionDenied }
        return Request(mount: entry.mount, args: args, sensitive: sensitive)
    }

    /// No await between final authority/instance checks and the real user command.
    /// Host checks must include current owner/epoch/resource/deadline/generation.
    public func perform(_ request: Request, authorized: () -> Bool) throws {
        guard entries[request.mount.id]?.invokeAsync == nil else { throw ControlRegistryFailure.unsupported }
        guard let entry = entries[request.mount.id], entry.mount == request.mount,
              entry.mount.registry == identity else { throw ControlRegistryFailure.conflict }
        guard entry.sensitive == request.sensitive else { throw ControlRegistryFailure.permissionDenied }
        guard currentRoute(entry.window) == entry.route, interactionAllowed(entry.window), entry.visible(), entry.enabled() else {
            throw ControlRegistryFailure.unavailable
        }
        let verb = try text(try request.args.requireMap(), "verb")
        guard entry.verbs.contains(verb) else { throw ControlRegistryFailure.unsupported }
        guard revision < UInt64.max else { throw ControlRegistryFailure.exhausted }
        guard authorized() else { throw ControlRegistryFailure.permissionDenied }
        try entry.invoke(request.args)
        // A shared native command may synchronously dismiss/unmount its own
        // control. Its applied effect is not a failure, and its successor must
        // never supply a value/change event for the old mounted request.
        if entries[entry.mount.id]?.mount == entry.mount {
            try controlChanged(entry.mount, force: entry.secure || entry.sensitive)
        }
    }

    /// Capture the exact mounted focus target before queuing a UI request.
    func isAsynchronous(_ request: Request) throws -> Bool {
        guard let entry = entries[request.mount.id], entry.mount == request.mount else { throw ControlRegistryFailure.conflict }
        return entry.invokeAsync != nil
    }

    func performAsync(_ request: Request, authorized: @escaping () -> Bool) async throws {
        guard let entry = entries[request.mount.id], entry.mount == request.mount,
              let invoke = entry.invokeAsync else { throw ControlRegistryFailure.conflict }
        let verb = try text(try request.args.requireMap(), "verb")
        var finished = false
        let context = EffectContext { [self] in
            guard !finished, let current = entries[request.mount.id], current.mount == request.mount,
                  current.mount.registry == identity else { throw ControlRegistryFailure.conflict }
            guard current.sensitive == request.sensitive else { throw ControlRegistryFailure.permissionDenied }
            guard currentRoute(current.window) == current.route, interactionAllowed(current.window),
                  current.visible(), current.enabled() else { throw ControlRegistryFailure.unavailable }
            guard current.verbs.contains(verb) else { throw ControlRegistryFailure.unsupported }
            guard revision < UInt64.max else { throw ControlRegistryFailure.exhausted }
            guard authorized() else { throw ControlRegistryFailure.permissionDenied }
        }
        defer { finished = true }
        // Starting a command may perform native work before its first await.
        try context.perform { }
        try await invoke(request.args, context)
        // Completion is not permission to read a replacement's private value.
        if entries[entry.mount.id]?.mount == entry.mount {
            try context.perform { try controlChanged(entry.mount, force: entry.secure || entry.sensitive) }
        }
    }

    /// Capture the exact mounted focus target before queuing a UI request.
    public func prepareFocus(_ args: CBORValue) throws -> Mount {
        try CBORCodec.validate("app-focus-args", args)
        let fields = try args.requireMap()
        let id = try text(fields, "control"), window = try text(fields, "window")
        guard let entry = entries[id], entry.window == window else { throw ControlRegistryFailure.unavailable }
        return entry.mount
    }

    public func setFocus(_ mount: Mount, authorized: () -> Bool) throws {
        guard let entry = entries[mount.id], entry.mount == mount,
              currentRoute(entry.window) == entry.route, interactionAllowed(entry.window), entry.visible(), entry.enabled() else {
            throw ControlRegistryFailure.unavailable
        }
        guard let focus = entry.focus, let focused = entry.focused else { throw ControlRegistryFailure.unsupported }
        guard revision < UInt64.max else { throw ControlRegistryFailure.exhausted }
        guard authorized() else { throw ControlRegistryFailure.permissionDenied }
        guard try focus(), focused() else { throw ControlRegistryFailure.unavailable }
        try changed()
        onChange?("softmax.app.focus_changed", object(["revision": .unsigned(revision),
            "window": .text(entry.window), "control": .text(mount.id)]))
    }

    public func focusedControl() throws -> CBORValue {
        let focused = entries.values.filter { currentRoute($0.window) == $0.route && $0.visible() && $0.focused?() == true }
        guard focused.count <= 1 else { throw ControlRegistryFailure.conflict }
        guard let entry = focused.first else { return .null }
        return object(["window": .text(entry.window), "control": .text(entry.mount.id)])
    }

    /// Called by the shared user binding setter as well as adapter invocation.
    public func controlChanged(_ mount: Mount, force: Bool = false) throws {
        guard let entry = entries[mount.id], entry.mount == mount else { throw ControlRegistryFailure.conflict }
        let value = try projectedValue(entry)
        guard force || lastValues[mount.id] != value else { return }
        try changed()
        lastValues[mount.id] = value
        onChange?("softmax.app.control_changed", object(["revision": .unsigned(revision),
            "window": .text(entry.window), "control": .text(mount.id), "value": value]))
    }

    /// Capture safe copies on MainActor. The host validates/encodes/queues off actor.
    public func page(window: String, after: String? = nil, revision expected: UInt64? = nil,
                     limit: Int) throws -> CBORValue {
        guard (1...128).contains(limit) else { throw ControlRegistryFailure.invalidRegistration }
        try CBORCodec.validate("app-window", .text(window))
        let start: Int
        if let after {
            guard let cursor = cursors[after], cursor.revision == revision, expected == revision,
                  cursor.window == window else { throw ControlRegistryFailure.conflict }
            start = cursor.offset
        } else {
            guard expected == nil else { throw ControlRegistryFailure.conflict }
            start = 0
        }
        let items = entries.values.filter { $0.window == window }.sorted { $0.mount.id < $1.mount.id }
        guard start <= items.count else { throw ControlRegistryFailure.conflict }
        let end = min(start + limit, items.count)
        var controls: [CBORValue] = []
        for entry in items[start..<end] {
            controls.append(object(["id": .text(entry.mount.id), "parent": entry.parent.map(CBORValue.text) ?? .null,
                "window": .text(window), "role": .text(entry.role), "label": .text(entry.label),
                "value": try projectedValue(entry), "enabled": .boolean(entry.enabled() && interactionAllowed(window)),
                "visible": .boolean(entry.visible() && currentRoute(window) == entry.route), "sensitive": .boolean(entry.sensitive)]))
        }
        var next: CBORValue = .null
        if end < items.count {
            guard cursors.count < 4096 else { throw ControlRegistryFailure.exhausted }
            let token = cursors.first(where: {
                $0.value.revision == revision && $0.value.window == window && $0.value.offset == end
            })?.key ?? UUID().uuidString
            cursors[token] = Cursor(revision: revision, window: window, offset: end)
            next = .text(token)
        }
        let result = object(["revision": .unsigned(revision), "window": .text(window), "controls": .array(controls), "next": next])
        try CBORCodec.validate("app-controls-result", result)
        guard try CBORCodec.encodeProfile(result).count <= 250_000 else { throw ControlRegistryFailure.exhausted }
        return result
    }

    /// Observe committed native mutations, including user-driven responder changes.
    /// `windows` is a finite host inventory; actual routes are queried live. Only
    /// safe, generated-rule-validated payloads reach onChange. Initial open windows
    /// are reported as opens, never inferred as grants or SDK activation.
    public func synchronizeNativeState(windows: [String]) throws {
        guard windows.count <= 64, Set(windows).count == windows.count else {
            throw ControlRegistryFailure.invalidRegistration
        }
        var current: [String: String] = [:]
        for window in windows {
            try CBORCodec.validate("app-window", .text(window))
            if let route = currentRoute(window) {
                try CBORCodec.validate("app-location", object(["window": .text(window), "route": .text(route)]))
                current[window] = route
            }
        }
        for window in Set(observedWindows.keys).union(current.keys).sorted() {
            let old = observedWindows[window], new = current[window]
            guard old != new else { continue }
            try changed()
            observedWindows[window] = new
            let kind: String, payload: CBORValue
            if old == nil || new == nil {
                kind = "softmax.app.window_changed"
                payload = object(["revision": .unsigned(revision), "window": .text(window),
                    "open": .boolean(new != nil), "route": new.map(CBORValue.text) ?? .null])
                try CBORCodec.validate("app-window-changed", payload)
            } else {
                kind = "softmax.app.view_changed"
                payload = object(["revision": .unsigned(revision), "window": .text(window), "route": .text(new!)])
                try CBORCodec.validate("app-view-changed", payload)
            }
            onChange?(kind, payload)
        }
        // This also catches binding changes made by native keyboard/user commands.
        // Copy mounts first: a callback may synchronously unmount another control.
        for mount in entries.values.map(\.mount).sorted(by: { $0.id < $1.id }) {
            guard entries[mount.id]?.mount == mount else { continue }
            try controlChanged(mount)
        }
        let focus = try focusedControl()
        let next: (window: String, control: String)?
        if case .map(let fields) = focus {
            next = (try text(fields, "window"), try text(fields, "control"))
        } else { next = nil }
        if observedFocus?.window != next?.window || observedFocus?.control != next?.control {
            let previous = observedFocus
            try changed()
            observedFocus = next
            if let window = next?.window ?? previous?.window {
                let payload = object(["revision": .unsigned(revision), "window": .text(window),
                    "control": next.map { .text($0.control) } ?? .null])
                try CBORCodec.validate("app-focus-changed", payload)
                onChange?("softmax.app.focus_changed", payload)
            }
        }
    }

    private func projectedValue(_ entry: Entry) throws -> CBORValue {
        let value = entry.secure || entry.sensitive ? object(["kind": .text("redacted")]) : try entry.value()
        try CBORCodec.validate("app-value", value)
        return value
    }
    /// Actual host navigation/window mutations invalidate paged snapshots.
    /// Every action still queries the host; this stores no navigation shadow.
    public func nativeStateChanged() throws { try changed() }

    private func changed() throws {
        guard revision < UInt64.max else { throw ControlRegistryFailure.exhausted }
        revision += 1
        cursors.removeAll(keepingCapacity: true)
    }
    private func validID(_ value: String) -> Bool {
        (1...128).contains(value.utf8.count) && value.utf8.allSatisfy {
            (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || [45, 46, 95].contains($0)
        }
    }
    private func text(_ fields: [CBORMapEntry], _ key: String) throws -> String {
        guard let field = fields.first(where: { $0.key.utf8.elementsEqual(key.utf8) }), case .text(let value) = field.value else {
            throw ControlRegistryFailure.invalidRegistration
        }
        return value
    }
    private func object(_ fields: [String: CBORValue]) -> CBORValue {
        .map(fields.sorted { $0.key < $1.key }.map { CBORMapEntry($0.key, $0.value) })
    }
}
