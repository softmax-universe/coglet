import Foundation

/// Adapter refusals carry no user values or provider diagnostics.
public enum AppSurfaceFailure: Swift.Error, Equatable {
    case invalidArguments, unavailable, conflict, permissionDenied, unsupported, exhausted, `internal`
}

/// Product-native dispatch for the `softmax.app.*` operations (ADR-0030).
///
/// Bounded native adapter only: it is not SDK registration, transport, grant
/// authority or a ship claim. It keeps no navigation/window/settings shadow;
/// every decision queries the supplied host closures or the registry.
/// Effects run synchronously on the MainActor through the same closures a user
/// command uses, with no suspension between the final authorization check and
/// the effect.
@MainActor
public final class AppSurfaceAdapter {
    fileprivate enum Payload {
        case invoke(ControlRegistry.Request)
        case setting(ControlRegistry.Request, key: String, expected: CBORValue)
        case focus(ControlRegistry.Mount)
        case navigate(window: String, route: String)
        case open(window: String)
        case close(window: String)
    }

    /// Opaque, single-use command captured at prepare time.
    @MainActor
    public final class Prepared {
        fileprivate let adapter: UUID
        fileprivate let payload: Payload
        fileprivate let windowRevision: UInt64?
        fileprivate var consumed = false
        fileprivate init(adapter: UUID, payload: Payload, windowRevision: UInt64?) {
            self.adapter = adapter; self.payload = payload; self.windowRevision = windowRevision
        }
    }

    /// Fixed product mapping from explicit setting keys to actual mounted controls.
    private struct SettingControl { let id: String; let route: String }
    private static let settingControls: [String: SettingControl] = [
        "appearance": .init(id: "settings.general.appearance", route: "general"),
        "text_size": .init(id: "settings.general.text_size", route: "general"),
        "floating_orb": .init(id: "floatingOrbToggle", route: "general"),
        "talk_mode": .init(id: "settings.hotkey.toggle_mode", route: "hotkey"),
        "sound_cues": .init(id: "settings.audio.sound_cues", route: "audio")
    ]
    private static let maxEncodedRead = 250_000

    private let identity = UUID()
    private let registry: ControlRegistry
    private let captureState: () throws -> CBORValue
    private let currentRoute: (String) -> String?
    private let navigateHost: (String, String) throws -> Void
    private let openHost: (String) throws -> Void
    private let closeHost: (String) throws -> Void

    public init(registry: ControlRegistry,
                captureState: @escaping () throws -> CBORValue,
                currentRoute: @escaping (String) -> String?,
                navigate: @escaping (String, String) throws -> Void,
                open: @escaping (String) throws -> Void,
                close: @escaping (String) throws -> Void) {
        self.registry = registry; self.captureState = captureState; self.currentRoute = currentRoute
        self.navigateHost = navigate; self.openHost = open; self.closeHost = close
    }

    // MARK: Reads

    /// `softmax.app.state` and `softmax.app.controls`; output is generated-rule validated.
    public func read(_ operation: String, args: CBORValue) throws -> CBORValue {
        switch operation {
        case "softmax.app.state":
            try validateInput("app-empty", args)
            return try wrap { try self.state() }
        case "softmax.app.controls":
            try validateInput("app-controls-args", args)
            let fields = try map(args)
            let window = try text(fields, "window")
            guard case .unsigned(let limit)? = field(fields, "limit"), limit <= 128 else { throw AppSurfaceFailure.invalidArguments }
            var after: String?
            if case .text(let cursor)? = field(fields, "after") { after = cursor }
            var revision: UInt64?
            if case .unsigned(let value)? = field(fields, "revision") { revision = value }
            return try wrap { try self.registry.page(window: window, after: after, revision: revision, limit: Int(limit)) }
        default:
            throw AppSurfaceFailure.unsupported
        }
    }

    private func state() throws -> CBORValue {
        let snapshot = try captureState()
        try CBORCodec.validate("app-state-result", snapshot)
        let fields = try map(snapshot)
        guard case .unsigned(let revision)? = field(fields, "revision"), revision == registry.revision else {
            throw AppSurfaceFailure.conflict
        }
        // Reported open routes must be the actual host routes, not a stale cache.
        guard case .array(let windows)? = field(fields, "windows") else { throw AppSurfaceFailure.internal }
        for item in windows {
            let w = try map(item)
            guard case .boolean(let open)? = field(w, "open") else { throw AppSurfaceFailure.internal }
            let name = try text(w, "window")
            if open {
                guard currentRoute(name) == (try text(w, "route")) else { throw AppSurfaceFailure.conflict }
            } else {
                guard currentRoute(name) == nil else { throw AppSurfaceFailure.conflict }
            }
        }
        guard try CBORCodec.encodeProfile(snapshot).count <= Self.maxEncodedRead else { throw AppSurfaceFailure.exhausted }
        return snapshot
    }

    // MARK: Prepare

    public func prepare(_ operation: String, args: CBORValue) throws -> Prepared {
        let payload: Payload
        switch operation {
        case "softmax.app.invoke":
            payload = .invoke(try wrap { try self.registry.prepare(args, sensitive: false) })
        case "softmax.app.invoke_sensitive":
            payload = .invoke(try wrap { try self.registry.prepare(args, sensitive: true) })
        case "softmax.app.set_focus":
            payload = .focus(try wrap { try self.registry.prepareFocus(args) })
        case "softmax.app.set_setting":
            payload = try prepareSetting(args)
        case "softmax.app.navigate":
            try validateInput("app-location", args)
            let fields = try map(args)
            payload = .navigate(window: try text(fields, "window"), route: try text(fields, "route"))
        case "softmax.app.open_window":
            try validateInput("app-window-args", args)
            payload = .open(window: try text(try map(args), "window"))
        case "softmax.app.close_window":
            try validateInput("app-window-args", args)
            payload = .close(window: try text(try map(args), "window"))
        default:
            throw AppSurfaceFailure.unsupported
        }
        let windowRevision: UInt64?
        switch payload {
        case .navigate, .open, .close: windowRevision = registry.revision
        default: windowRevision = nil
        }
        return Prepared(adapter: identity, payload: payload, windowRevision: windowRevision)
    }

    private func prepareSetting(_ args: CBORValue) throws -> Payload {
        try validateInput("app-setting-args", args)
        let fields = try map(args)
        let key = try text(fields, "key")
        guard let value = field(fields, "value") else { throw AppSurfaceFailure.invalidArguments }
        // Hotkey has no native setter. Login is an approval-bearing operation that
        // ordinary authority never performs; it must use the sensitive path.
        if key == "hotkey" { throw AppSurfaceFailure.unsupported }
        if key == "launch_at_login" { throw AppSurfaceFailure.permissionDenied }
        guard let control = Self.settingControls[key] else { throw AppSurfaceFailure.unsupported }
        let invoke: CBORValue
        switch (key, value) {
        case ("appearance", .text), ("text_size", .text):
            invoke = object(["verb": .text("select"), "window": .text("settings"),
                             "control": .text(control.id), "value": value])
        case ("floating_orb", .boolean), ("sound_cues", .boolean):
            invoke = object(["verb": .text("toggle"), "window": .text("settings"),
                             "control": .text(control.id), "value": value])
        case ("talk_mode", .text(let mode)):
            // hold == toggle-mode off; toggle == toggle-mode on.
            invoke = object(["verb": .text("toggle"), "window": .text("settings"),
                             "control": .text(control.id), "value": .boolean(mode == "toggle")])
        default:
            throw AppSurfaceFailure.invalidArguments
        }
        let request = try wrap { try self.registry.prepare(invoke, sensitive: false) }
        return .setting(request, key: key, expected: value)
    }

    // MARK: Perform

    /// Returns the closed empty map only after the effect and postconditions held.
    /// `authorized` is evaluated at effect time, after every native availability check.
    public func perform(_ prepared: Prepared, authorized: () -> Bool) throws -> CBORValue {
        guard prepared.adapter == identity else { throw AppSurfaceFailure.conflict }
        guard !prepared.consumed else { throw AppSurfaceFailure.conflict }
        // Conservative presentation fence: ANY intervening native mutation
        // invalidates a queued window command, including close/reopen ABA. The
        // host must announce each committed presentation transition.
        if let captured = prepared.windowRevision, registry.revision != captured {
            throw AppSurfaceFailure.conflict
        }
        // Consumption begins only once authority holds, so a refused attempt may retry
        // with the same capture, while an authorized one can never replay.
        func gate() -> Bool {
            guard authorized() else { return false }
            prepared.consumed = true
            return true
        }
        switch prepared.payload {
        case .invoke(let request):
            try wrap { try self.registry.perform(request, authorized: gate) }
        case .setting(let request, let key, let expected):
            try wrap { try self.registry.perform(request, authorized: gate) }
            guard try actualSetting(key) == expected else { throw AppSurfaceFailure.conflict }
        case .focus(let mount):
            try wrap { try self.registry.setFocus(mount, authorized: gate) }
        case .navigate(let window, let route):
            guard currentRoute(window) != nil else { throw AppSurfaceFailure.unavailable }
            try gateEffect(gate)
            try wrap { try self.navigateHost(window, route) }
            guard currentRoute(window) == route else { throw AppSurfaceFailure.unavailable }
            try wrap { try self.registry.nativeStateChanged() }
        case .open(let window):
            try gateEffect(gate)
            if currentRoute(window) == nil {
                try wrap { try self.openHost(window) }
                guard currentRoute(window) != nil else { throw AppSurfaceFailure.unavailable }
                try wrap { try self.registry.nativeStateChanged() }
            }
        case .close(let window):
            try gateEffect(gate)
            if currentRoute(window) != nil {
                try wrap { try self.closeHost(window) }
                guard currentRoute(window) == nil else { throw AppSurfaceFailure.unavailable }
                try wrap { try self.registry.nativeStateChanged() }
            }
        }
        return .map([])
    }

    private func gateEffect(_ gate: () -> Bool) throws {
        guard registry.revision < UInt64.max else { throw AppSurfaceFailure.exhausted }
        guard gate() else { throw AppSurfaceFailure.permissionDenied }
    }

    private func actualSetting(_ key: String) throws -> CBORValue? {
        let snapshot = try wrap { try self.captureState() }
        try wrap { try CBORCodec.validate("app-state-result", snapshot) }
        guard let settings = field(try map(snapshot), "settings") else { return nil }
        return field(try map(settings), key)
    }

    // MARK: Helpers

    private func validateInput(_ rule: String, _ value: CBORValue) throws {
        do { try CBORCodec.validate(rule, value) } catch { throw AppSurfaceFailure.invalidArguments }
    }
    private func wrap<T>(_ body: () throws -> T) throws -> T {
        do { return try body() }
        catch let error as AppSurfaceFailure { throw error }
        catch let error as ControlRegistryFailure {
            switch error {
            case .invalidRegistration: throw AppSurfaceFailure.invalidArguments
            case .duplicate, .conflict: throw AppSurfaceFailure.conflict
            case .unavailable: throw AppSurfaceFailure.unavailable
            case .permissionDenied: throw AppSurfaceFailure.permissionDenied
            case .unsupported: throw AppSurfaceFailure.unsupported
            case .exhausted: throw AppSurfaceFailure.exhausted
            }
        } catch is CBORFailure { throw AppSurfaceFailure.invalidArguments }
        catch { throw AppSurfaceFailure.internal }
    }
    private func map(_ value: CBORValue) throws -> [CBORMapEntry] {
        guard case .map(let fields) = value else { throw AppSurfaceFailure.invalidArguments }
        return fields
    }
    private func field(_ fields: [CBORMapEntry], _ key: String) -> CBORValue? {
        fields.first(where: { $0.key.utf8.elementsEqual(key.utf8) })?.value
    }
    private func text(_ fields: [CBORMapEntry], _ key: String) throws -> String {
        guard case .text(let value)? = field(fields, key) else { throw AppSurfaceFailure.invalidArguments }
        return value
    }
    private func object(_ fields: [String: CBORValue]) -> CBORValue {
        .map(fields.sorted { $0.key < $1.key }.map { CBORMapEntry($0.key, $0.value) })
    }
}
