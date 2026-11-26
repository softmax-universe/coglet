import Foundation

/// Native callbacks contain only the product's generated, already-redacted
/// payload type. This alias lets hosts avoid importing two CBOR enums unqualified.
public typealias AppSurfaceEventHandler = (String, CBORValue) -> Void

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
    fileprivate enum Payload: Sendable {
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

    /// Immutable command identity captured before SDK admission, safe to prepare
    /// on the off-main control owner. Contains no native closures or user values.
    public struct CapturedCommand: Sendable {
        fileprivate let adapter: UUID
        fileprivate let payload: Payload
        fileprivate let windowRevision: UInt64?
    }
    public struct CommandDirectory: Sendable {
        fileprivate let adapter: UUID
        var sourceIdentity: UUID { adapter }
        fileprivate let controls: ControlRegistry.CapturedDirectory

        public func prepare(_ operation: String, args: CBORValue) throws -> CapturedCommand {
            let payload: Payload
            var revision: UInt64?
            switch operation {
            case "softmax.app.invoke", "softmax.app.invoke_sensitive":
                payload = .invoke(try controls.prepare(args, sensitive: operation == "softmax.app.invoke_sensitive"))
            case "softmax.app.set_focus":
                payload = .focus(try controls.prepareFocus(args))
            case "softmax.app.set_setting":
                try CBORCodec.validate("app-setting-args", args)
                guard case .text(let key)? = args["key"], let value = args["value"] else { throw AppSurfaceFailure.invalidArguments }
                if key == "hotkey" { throw AppSurfaceFailure.unsupported }
                if key == "launch_at_login" { throw AppSurfaceFailure.permissionDenied }
                guard let control = AppSurfaceAdapter.settingControls[key] else { throw AppSurfaceFailure.unsupported }
                let verb: String, nativeValue: CBORValue
                switch (key, value) {
                case ("appearance", .text), ("text_size", .text): verb = "select"; nativeValue = value
                case ("floating_orb", .boolean), ("sound_cues", .boolean): verb = "toggle"; nativeValue = value
                case ("talk_mode", .text(let mode)): verb = "toggle"; nativeValue = .boolean(mode == "toggle")
                default: throw AppSurfaceFailure.invalidArguments
                }
                let invoke = CBORValue.map([.init("verb", .text(verb)), .init("window", .text("settings")),
                    .init("control", .text(control.id)), .init("value", nativeValue)])
                payload = .setting(try controls.prepare(invoke, sensitive: false), key: key, expected: value)
            case "softmax.app.navigate":
                try CBORCodec.validate("app-location", args)
                guard case .text(let window)? = args["window"], case .text(let route)? = args["route"] else { throw AppSurfaceFailure.invalidArguments }
                payload = .navigate(window: window, route: route); revision = controls.revision
            case "softmax.app.open_window", "softmax.app.close_window":
                try CBORCodec.validate("app-window-args", args)
                guard case .text(let window)? = args["window"] else { throw AppSurfaceFailure.invalidArguments }
                payload = operation == "softmax.app.open_window" ? .open(window: window) : .close(window: window)
                revision = controls.revision
            default: throw AppSurfaceFailure.unsupported
            }
            return CapturedCommand(adapter: adapter, payload: payload, windowRevision: revision)
        }
    }
    public func captureCommandDirectory() -> CommandDirectory {
        CommandDirectory(adapter: identity, controls: registry.captureDirectory())
    }
    /// Wrap, never re-resolve IDs on the native owner after a queue hop. The host
    /// mailbox supplies one-shot ticket custody; Prepared retains its own guard.
    public func prepareCaptured(_ command: CapturedCommand) throws -> Prepared {
        guard command.adapter == identity else { throw AppSurfaceFailure.conflict }
        return Prepared(adapter: command.adapter, payload: command.payload, windowRevision: command.windowRevision)
    }

    /// Fixed product mapping from explicit setting keys to actual mounted controls.
    private struct SettingControl: Sendable { let id: String; let route: String }
    nonisolated private static let settingControls: [String: SettingControl] = [
        "appearance": .init(id: "settings.general.appearance", route: "general"),
        "text_size": .init(id: "settings.general.text_size", route: "general"),
        "floating_orb": .init(id: "floatingOrbToggle", route: "general"),
        "talk_mode": .init(id: "settings.hotkey.toggle_mode", route: "hotkey"),
        "sound_cues": .init(id: "settings.audio.sound_cues", route: "audio")
    ]
    private static let maxEncodedRead = 250_000

    private let identity = UUID()
    var registryRevision: UInt64 { registry.revision }
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
        try wrap { try self.prepareCaptured(self.captureCommandDirectory().prepare(operation, args: args)) }
    }

    // MARK: Perform

    func isAsynchronous(_ prepared: Prepared) throws -> Bool {
        guard prepared.adapter == identity, !prepared.consumed else { throw AppSurfaceFailure.conflict }
        switch prepared.payload {
        case .invoke(let request), .setting(let request, _, _): return try registry.isAsynchronous(request)
        case .focus(let mount): return registry.isAsynchronousFocus(mount)
        default: return false
        }
    }

    func performAsync(_ prepared: Prepared, authorized: @escaping () -> Bool) async throws -> CBORValue {
        guard prepared.adapter == identity, !prepared.consumed else { throw AppSurfaceFailure.conflict }
        guard try isAsynchronous(prepared) else { throw AppSurfaceFailure.unsupported }
        prepared.consumed = true
        switch prepared.payload {
        case .invoke(let request): try await registry.performAsync(request, authorized: authorized)
        case .setting(let request, let key, let expected):
            try await registry.performAsync(request, authorized: authorized)
            guard authorized() else { throw AppSurfaceFailure.permissionDenied }
            guard try actualSetting(key) == expected else { throw AppSurfaceFailure.conflict }
        case .focus(let mount): try await registry.setFocusAsync(mount, authorized: authorized)
        default: throw AppSurfaceFailure.unsupported
        }
        return .map([])
    }

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
