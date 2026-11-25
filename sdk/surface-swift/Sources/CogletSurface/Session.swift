import Foundation
import CogletTypes

public struct SurfaceOperation: Sendable {
    public enum Direction: Sendable { case event, read, action }
    public let name: String
    public let entry: String
    public let direction: Direction
    public let inputRule: String
    public let outputRule: String?
    public let resources: [String]
    public init(name: String, entry: String, direction: Direction, inputRule: String, outputRule: String?, resources: [String]) {
        self.name = name; self.entry = entry; self.direction = direction
        self.inputRule = inputRule; self.outputRule = outputRule; self.resources = resources
    }
    /// Frozen built-ins. Products select only operations their backend implements.
    public static let builtins: [SurfaceOperation] = [
        .init(name: "speech.config", entry: "speech.control", direction: .read, inputRule: "surface::empty-map", outputRule: "speech::speech-config", resources: []),
        .init(name: "speech.speak_chunk", entry: "speech.control", direction: .action, inputRule: "speech::speak-chunk", outputRule: "surface::empty-map", resources: ["audio.speaker"]),
        .init(name: "speech.stop", entry: "speech.control", direction: .action, inputRule: "speech::stop", outputRule: "surface::empty-map", resources: ["audio.speaker"]),
        .init(name: "speech.backchannel", entry: "speech.control", direction: .action, inputRule: "speech::backchannel", outputRule: "surface::empty-map", resources: ["audio.speaker"]),
        .init(name: "speech.session_open", entry: "speech.session", direction: .action, inputRule: "speech::session-open", outputRule: "surface::empty-map", resources: ["audio.mic", "audio.speaker"]),
        .init(name: "speech.session_close", entry: "speech.session", direction: .action, inputRule: "speech::session-close", outputRule: "surface::empty-map", resources: ["audio.mic", "audio.speaker"]),
        .init(name: "speech.tool_result", entry: "speech.session", direction: .action, inputRule: "speech::tool-result", outputRule: "surface::empty-map", resources: ["audio.mic", "audio.speaker"]),
        .init(name: "speech.heard", entry: "speech.history", direction: .read, inputRule: "speech::heard-query", outputRule: "speech::heard-record", resources: ["audio.speaker"]),
        .init(name: "speech.work_state", entry: "speech.work", direction: .read, inputRule: "surface::empty-map", outputRule: "speech::work-state", resources: ["audio.mic", "audio.speaker"]),
        .init(name: "speech.work_result", entry: "speech.work", direction: .action, inputRule: "speech::work-result", outputRule: "surface::empty-map", resources: ["audio.mic", "audio.speaker"]),
        .init(name: "expression.set_expression", entry: "expression.control", direction: .action, inputRule: "expression::set-expression", outputRule: "surface::empty-map", resources: ["display.face"]),
        .init(name: "expression.state", entry: "expression.control", direction: .read, inputRule: "surface::empty-map", outputRule: "expression::expression-state", resources: ["display.face"])
    ] + ["start", "partial", "end", "position", "barge_in", "tool_call", "work_intent", "work_done"].map { name in
        let entry = name == "tool_call" ? "speech.session" : (name.hasPrefix("work_") ? "speech.work" : "speech.control")
        let resources = ["position"].contains(name) ? ["audio.speaker"] : ["audio.mic", "audio.speaker"]
        let rule = (["tool_call", "work_intent", "work_done"].contains(name) ? "" : "speech-") + name.replacingOccurrences(of: "_", with: "-")
        return .init(name: "speech." + name, entry: entry, direction: .event,
            inputRule: "speech::" + rule, outputRule: nil, resources: resources)
    }
}

public struct EffectiveSurfaceGrant: Sendable {
    public let coglet: String
    public let operations: Set<String>
    public let resources: Set<String>
    public let expires: UInt64
    public init(coglet: String, operations: Set<String>, resources: Set<String>, expires: UInt64) {
        self.coglet = coglet; self.operations = operations; self.resources = resources; self.expires = expires
    }
    var value: CBORValue { .record([("coglet", .text(coglet)), ("operations", .array(operations.sorted().map(CBORValue.text))),
        ("resources", .array(resources.sorted().map(CBORValue.text))), ("expires", .unsigned(expires))]) }
}

public struct NativeSurfaceError: Swift.Error, Sendable {
    public let code: String
    public init(_ code: String) { self.code = code }
    var value: CBORValue { .record([("code", .text(code)), ("message", .text(code)), ("retryable", .boolean(false))]) }
}

public enum NativeSurfaceResult: Sendable {
    case succeeded(CBORValue)
    case failed(NativeSurfaceError)
}

/// Opaque callback custody captured when a native/provider generation STARTS.
/// Product code must never obtain a new token inside an old callback.
public struct NativeObservationToken: Equatable, Sendable {
    fileprivate let owner: String
    fileprivate let epoch: UInt64
    fileprivate let session: UUID
}

/// Operation-scoped product event custody, independent of speech capture.
public struct ExtensionObservationToken: Equatable, Sendable {
    fileprivate let owner: String
    fileprivate let operation: String
    fileprivate let epoch: UInt64
    fileprivate let session: UUID
}

/// Native callbacks are explicit. preflight MUST be side-effect-free; begin runs
/// only after durable admission. Returning nil leaves the action pending until
/// complete() with the captured epoch. Stop MUST synchronously fence native I/O.
/// The app's hardware off-switch calls its native stop independently, then tells
/// the serialized session to latch/notify. No wire message can reset the latch.
public protocol SurfaceNativeBackend: AnyObject {
    func preflight(action: CBORValue, now: UInt64) throws
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult?
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue
    func stop(owner: String?)
}

/// Control-queue state machine shared by socket and embedded transports. The
/// supervisor authenticates/configures the binding before construction. Never
/// call concurrently or on a real-time audio thread. All public dispatch/tick
/// errors are fatal: stop, close the transport, and establish a fresh session.
public final class SurfaceSession {
    private let ledger: SurfaceLedger
    private let backend: any SurfaceNativeBackend
    private let boundCoglets: Set<String>
    private let operations: [String: SurfaceOperation]
    private let contractProfile: SurfaceContractProfile
    private let trustedCatalog: TrustedExtensionCatalog?
    private var wireProfile: SurfaceContractProfile = .surface01
    private var sessionOperations: [String: SurfaceOperation] {
        if contractProfile == .surface02 && wireProfile == .surface01 {
            return operations.filter { name, _ in SurfaceOperation.builtins.contains { $0.name == name } }
        }
        return operations
    }
    private let clock: () -> UInt64
    private let correlation: () -> String
    private var lastClock: UInt64
    private var lastPeerClock: UInt64?
    private var lastReceived: UInt64
    private var lastHealth: UInt64
    private var connected = true
    private var retired = false
    private var negotiated = false
    private var helloSent = false
    private var helloCorrelation: String?
    private var revision: UInt64 = 1
    private var grants: [EffectiveSurfaceGrant] = []
    private var output: [(bytes: [UInt8], observationOwner: String?, extensionOperation: String?)] = []
    private var observationTokens: [String: NativeObservationToken] = [:]
    private var extensionTokens: [String: ExtensionObservationToken] = [:]
    private var normalizers: [String: SpeechObservations] = [:]
    public var grantEpoch: UInt64 { ledger.epoch }
    public var isKilled: Bool { ledger.killed }

    public init(identity: String, boundCoglets: Set<String>, operations: [SurfaceOperation],
                store: any SurfaceLedgerStore, backend: any SurfaceNativeBackend,
                contractProfile: SurfaceContractProfile = .surface01, trustedCatalog: TrustedExtensionCatalog? = nil,
                initialGrants: [EffectiveSurfaceGrant] = [],
                clock: @escaping () -> UInt64, correlation: @escaping () -> String = { UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased() }) throws {
        try CBORCodec.validate("common::surface-id", .text(identity))
        for owner in boundCoglets { try CBORCodec.validate("common::coglet-id", .text(owner)) }
        guard Set(operations.map(\.name)).count == operations.count else { throw SurfaceFailure.protocolViolation("duplicate operation") }
        guard trustedCatalog == nil || trustedCatalog?.contractProfile == contractProfile else {
            throw SurfaceFailure.protocolViolation("catalog/profile mismatch")
        }
        if let catalog = trustedCatalog, operations.contains(where: { operation in catalog.operations.contains { $0.name == operation.name } }), boundCoglets.count != 1 {
            throw SurfaceFailure.protocolViolation("extension operations require exactly one bound owner")
        }
        // No product/test/runtime authority can be invented by advertisement.
        for operation in operations {
            guard let canonical = (SurfaceOperation.builtins + (trustedCatalog?.operations ?? [])).first(where: { $0.name == operation.name }),
                  operation.entry == canonical.entry, operation.direction == canonical.direction,
                  operation.inputRule == canonical.inputRule, operation.outputRule == canonical.outputRule,
                  operation.resources == canonical.resources else { throw SurfaceFailure.protocolViolation("unknown or altered binding") }
        }
        self.operations = Dictionary(uniqueKeysWithValues: operations.map { ($0.name, $0) })
        self.contractProfile = contractProfile; self.trustedCatalog = trustedCatalog
        self.boundCoglets = boundCoglets; self.backend = backend; self.clock = clock; self.correlation = correlation
        self.ledger = try SurfaceLedger(store: store, identity: identity, contractProfile: contractProfile, catalogPin: trustedCatalog?.pin)
        let now = clock(); self.lastClock = now; self.lastReceived = now; self.lastHealth = now
        backend.stop(owner: nil)
        guard ledger.epoch < UInt64.max else { throw SurfaceFailure.exhausted }
        ledger.epoch += 1
        // Recovery never resumes an uncertain physical effect.
        ledger.running = true
        for (key, record) in ledger.records where record.accept["accepted"] == .boolean(true) && record.outcome == nil {
            ledger.records[key]?.outcome = Self.outcome(record: record, status: "failed", result: NativeSurfaceError("unavailable").value)
        }
        if !ledger.killed {
            try validateGrants(initialGrants, now: now)
            grants = initialGrants
        }
        try ledger.save()
    }
    deinit { backend.stop(owner: nil) }

    public func start() throws {
        try guarded {
            guard connected, !helloSent else { throw SurfaceFailure.protocolViolation("duplicate start") }
            let now = try sample()
            let id = correlation(); helloCorrelation = id; helloSent = true
            try emit(.message("hello", .record([("surface", .text(ledger.identity)),
                ("offer", .record([("versions", .array(contractProfile == .surface02 ? [.text("0.2"), .text("0.1")] : [.text("0.1")]))])),
                ("clock", .unsigned(now)), ("correlation_id", .text(id))])))
        }
    }
    public func drain() -> [[UInt8]] { let bytes = output.map(\.bytes); output = []; return bytes }

    /// Durably reserves one source incarnation for host subscription setup and
    /// returns the OLD value of the ledger's existing event-sequence allocator.
    /// The counter is advanced and saved BEFORE return, so the returned value can
    /// never equal any event `seq` (past or future) of this identity; consumed
    /// values are never rolled back (gaps are allowed, including after a failed
    /// or stopped reservation). Needs no negotiation and mints no grant. Same
    /// serialized-owner contract as every other method: never call concurrently.
    /// Any failure (closed/killed/retired session, save error, clock rollback,
    /// exhaustion, or a disconnect/kill/retire observed after the save, e.g. from
    /// a store callback) is terminal for this instance and returns no value.
    public func reserveSourceIncarnation() throws -> UInt64 {
        guard !retired else { throw SurfaceFailure.closed }
        var reserved: UInt64 = 0
        try guarded {
            guard connected, !ledger.killed else { throw SurfaceFailure.closed }
            guard ledger.sequence < UInt64.max else { throw SurfaceFailure.exhausted }
            let value = ledger.sequence; ledger.sequence += 1
            try ledger.save()
            _ = try sample()
            guard connected, !ledger.killed, !retired else { throw SurfaceFailure.closed }
            reserved = value
        }
        return reserved
    }

    /// Called before capture/provider generation begins. Replacing the token
    /// fences a native session even within the SAME grant epoch.
    public func openNativeObservationSession(owner: String, epoch: UInt64) throws -> NativeObservationToken {
        guard connected, negotiated, !ledger.killed, epoch == ledger.epoch,
              boundCoglets.contains(owner), let grant = grants.first(where: { $0.coglet == owner }),
              grant.expires > clock() else { throw NativeSurfaceError("permission_denied") }
        backend.stop(owner: owner)
        output.removeAll { $0.observationOwner == owner && $0.extensionOperation == nil }
        let token = NativeObservationToken(owner: owner, epoch: epoch, session: UUID())
        observationTokens[owner] = token; normalizers[owner] = SpeechObservations()
        return token
    }
    public func closeNativeObservationSession(_ token: NativeObservationToken) {
        guard observationTokens[token.owner] == token else { return }
        backend.stop(owner: token.owner)
        observationTokens[token.owner] = nil; normalizers[token.owner] = nil
        output.removeAll { $0.observationOwner == token.owner && $0.extensionOperation == nil }
    }

    public func openExtensionObservationSession(owner: String, operation: String, epoch: UInt64) throws -> ExtensionObservationToken {
        let now = try sample()
        guard connected, negotiated, let binding = sessionOperations[operation], binding.direction == .event,
              trustedCatalog?.operations.contains(where: { $0.name == operation }) == true else { throw NativeSurfaceError("permission_denied") }
        try authorize(owner: owner, operation: binding, epoch: epoch, deadline: now == UInt64.max ? now : now + 1, now: now)
        output.removeAll { $0.observationOwner == owner && $0.extensionOperation == operation }
        let token = ExtensionObservationToken(owner: owner, operation: operation, epoch: epoch, session: UUID())
        extensionTokens[owner + "/" + operation] = token
        return token
    }

    public func closeExtensionObservationSession(_ token: ExtensionObservationToken) {
        let key = token.owner + "/" + token.operation
        guard extensionTokens[key] == token else { return }
        extensionTokens[key] = nil
        output.removeAll { $0.observationOwner == token.owner && $0.extensionOperation == token.operation }
    }

    /// Hosts must fence already-drained/partial transport frames on custody
    /// change. Never acquire a new token inside an old native callback.
    public func extensionObservationIsCurrent(_ token: ExtensionObservationToken) -> Bool {
        connected && negotiated && !ledger.killed && token.epoch == ledger.epoch &&
            extensionTokens[token.owner + "/" + token.operation] == token &&
            sessionOperations[token.operation] != nil &&
            grants.contains { $0.coglet == token.owner && $0.expires > clock() && $0.operations.contains(token.operation) }
    }

    @discardableResult
    public func publishExtensionObservation(_ token: ExtensionObservationToken, payload: CBORValue) throws -> Bool {
        guard extensionObservationIsCurrent(token) else { return false }
        try guarded {
            let now = try sample()
            let binding = sessionOperations[token.operation]!
            try authorize(owner: token.owner, operation: binding, epoch: token.epoch, deadline: now == UInt64.max ? now : now + 1, now: now)
            guard try CBORCodec.encodeProfile(payload).count <= 262_144 else { throw SurfaceFailure.exhausted }
            try check(binding, output: false, value: payload)
            if let read = try trustedCatalog?.selectedRead(operation: token.operation, value: payload) {
                guard let selected = sessionOperations[read], selected.direction == .read else { throw NativeSurfaceError("permission_denied") }
                try authorize(owner: token.owner, operation: selected, epoch: token.epoch, deadline: now == UInt64.max ? now : now + 1, now: now)
            }
            try emitEvent(kind: token.operation, payload: payload, correlationID: correlation(), now: now,
                observationOwner: token.owner, extensionOperation: token.operation, beforeEmit: {
                    let publicationTime = try self.sample()
                    guard self.extensionObservationIsCurrent(token) else { throw NativeSurfaceError("permission_denied") }
                    try self.authorize(owner: token.owner, operation: binding, epoch: token.epoch,
                        deadline: publicationTime == UInt64.max ? publicationTime : publicationTime + 1, now: publicationTime)
                    if let read = try self.trustedCatalog?.selectedRead(operation: token.operation, value: payload) {
                        guard let selected = self.sessionOperations[read] else { throw NativeSurfaceError("permission_denied") }
                        try self.authorize(owner: token.owner, operation: selected, epoch: token.epoch,
                            deadline: publicationTime == UInt64.max ? publicationTime : publicationTime + 1, now: publicationTime)
                    }
                })
        }
        return true
    }

    /// false = fenced session or stale/late transcript, with no event/sequence.
    /// Uses the SAME durable sequence allocator and bounded queue as outcomes.
    @discardableResult
    public func publishObservation(_ token: NativeObservationToken, kind: String, payload: CBORValue) throws -> Bool {
        guard connected, negotiated, !ledger.killed, token.epoch == ledger.epoch,
              observationTokens[token.owner] == token else { return false }
        var published = false
        try guarded {
            let now = try sample()
            guard let operation = operations[kind], operation.direction == .event,
                  SurfaceOperation.builtins.contains(where: { $0.name == kind }) else { throw NativeSurfaceError("permission_denied") }
            try authorize(owner: token.owner, operation: operation, epoch: token.epoch, deadline: now == UInt64.max ? now : now + 1, now: now)
            try CBORCodec.validate(operation.inputRule, payload)
            let normalizer = normalizers[token.owner]!
            if kind == "speech.start" {
                try normalizer.start(utterance: payload["utterance"]!.textValue!)
            } else if kind == "speech.partial" || kind == "speech.end" {
                guard try normalizer.transcript(utterance: payload["utterance"]!.textValue!,
                    revision: payload["revision"]!.uintValue!, text: payload["text"]!.textValue!, final: kind == "speech.end") else { return }
            }
            try emitEvent(kind: kind, payload: payload, correlationID: correlation(), now: now, observationOwner: token.owner)
            published = true
        }
        return published
    }

    public func receive(_ bytes: [UInt8]) throws {
        try guarded {
            guard connected, helloSent else { throw SurfaceFailure.closed }
            let message = try SurfaceWire.decode(bytes, from: .runtime, contractProfile: wireProfile)
            let type = message["type"]!.textValue!, body = message["body"]!
            let now = try sample()
            if !negotiated {
                guard type == "hello_reply", body["correlation_id"] == helloCorrelation.map(CBORValue.text),
                      let version = body["selection"]?["version"]?.textValue,
                      version == "0.1" || (version == "0.2" && contractProfile == .surface02) else {
                    throw SurfaceFailure.protocolViolation("invalid negotiation")
                }
                wireProfile = version == "0.2" ? .surface02 : .surface01
                negotiated = true; lastReceived = now
                try publishCapabilities(now: now)
                try publishGrants()
                return
            }
            switch type {
            case "act": try act(message: message, bytes: bytes, now: now)
            case "read": try read(body: body, now: now)
            case "health":
                let peerClock = body["clock"]!.uintValue!
                if let previous = lastPeerClock, peerClock < previous { throw SurfaceFailure.protocolViolation("peer clock rollback") }
                lastPeerClock = peerClock
            case "error": break
            default: throw SurfaceFailure.protocolViolation("invalid lifecycle")
            }
            lastReceived = now
        }
    }
    /// Install authenticated local policy intersected with OS/privacy permissions.
    /// Invalid replacements leave the prior snapshot intact. Epoch always advances.
    public func installGrants(_ replacement: [EffectiveSurfaceGrant]) throws {
        let admissionTime = clock()
        // Local configuration failure is atomic, not a peer protocol failure.
        try validateGrants(replacement, now: admissionTime)
        try guarded {
            guard connected, !ledger.killed else { throw SurfaceFailure.closed }
            let now = try sample()
            guard ledger.epoch < UInt64.max else { throw SurfaceFailure.exhausted }
            backend.stop(owner: nil)
            output.removeAll { $0.observationOwner != nil }
            observationTokens = [:]; extensionTokens = [:]; normalizers = [:]
            try cancelPending(status: "cancelled", code: "cancelled", now: now, deliver: false)
            ledger.epoch += 1; grants = replacement
            try ledger.save()
            if negotiated { try publishGrants() }
        }
    }
    public func tick() throws {
        try guarded {
            guard connected else { throw SurfaceFailure.closed }
            let now = try sample()
            if now - lastReceived >= 3_000_000 { throw SurfaceFailure.protocolViolation("health timeout") }
            if grants.contains(where: { $0.expires <= now }) {
                // Revocation is a full snapshot, before any ownership reuse.
                let survivors = grants.filter { $0.expires > now }
                try installGrants(survivors)
            }
            for (key, record) in ledger.records where record.accept["accepted"] == .boolean(true) && record.outcome == nil {
                let request = try SurfaceWire.decode(record.envelope, from: .runtime, contractProfile: contractProfile)["body"]!["action"]!
                if request["deadline"]!.uintValue! <= now {
                    backend.stop(owner: request["coglet"]!.textValue!)
                    try terminal(key: key, status: "expired", value: NativeSurfaceError("deadline_exceeded").value, now: now)
                }
            }
            if negotiated, now - lastHealth >= 1_000_000 {
                try emitHealth(now: now); lastHealth = now
            }
        }
    }
    /// Callback carries the epoch captured at begin; delayed callbacks cannot
    /// complete an action under a replacement owner. Exactly one terminal result.
    public func complete(owner: String, id: String, epoch: UInt64, result: NativeSurfaceResult) throws {
        try guarded {
            guard connected, !ledger.killed, epoch == ledger.epoch else { return }
            let key = owner + "/" + id
            guard let record = ledger.records[key], record.accept["accepted"] == .boolean(true), record.outcome == nil else { return }
            let request = try SurfaceWire.decode(record.envelope, from: .runtime, contractProfile: contractProfile)["body"]!["action"]!
            let now = try sample()
            guard let grant = grants.first(where: { $0.coglet == owner }), grant.expires > now else {
                try installGrants(grants.filter { $0.expires > now })
                return
            }
            if request["deadline"]!.uintValue! <= now {
                backend.stop(owner: owner)
                try terminal(key: key, status: "expired", value: NativeSurfaceError("deadline_exceeded").value, now: now)
                return
            }
            switch result {
            case .succeeded(let value):
                let operation = operations[request["capability"]!.textValue!]!
                try check(operation, output: true, value: value)
                try terminal(key: key, status: "succeeded", value: value, now: now)
            case .failed(let error): try terminal(key: key, status: "failed", value: error.value, now: now)
            }
        }
    }
    public func kill(reason: String) throws {
        guard !retired else { throw SurfaceFailure.closed }
        // Native hardware stop must ALREADY have happened, independently of the
        // control queue. Repeat it here as a defense in depth.
        backend.stop(owner: nil)
        try guarded {
            ledger.killed = true; grants = []
            // Local kill remains durable even after link loss or a bad clock.
            if ledger.epoch < UInt64.max { ledger.epoch += 1 }
            let now = lastClock
            output.removeAll { $0.observationOwner != nil }
            observationTokens = [:]; extensionTokens = [:]; normalizers = [:]
            try cancelPending(status: "cancelled", code: "cancelled", now: now, deliver: false)
            try ledger.save()
            if connected && negotiated {
                try emit(.message("kill", .record([("reason", .text(reason)), ("correlation_id", .text(correlation()))])))
                try emitHealth(now: now)
            }
        }
    }
    /// Terminal orderly shutdown. Unlike disconnect/deinit, success permits a
    /// healthy restart without operator reset; it NEVER clears a kill latch.
    /// Stop external callbacks before calling. This instance is retired even if
    /// the final save throws: reopen to recover, never reuse it.
    public func close() throws {
        guard !retired else { throw SurfaceFailure.closed }
        disconnect()
        retired = true
        do {
            try cancelPending(status: "cancelled", code: "cancelled", now: lastClock, deliver: false)
            ledger.running = false
            try ledger.save()
        } catch {
            ledger.running = true; ledger.killed = true
            throw error
        }
    }

    public func disconnect() {
        guard !retired else { return }
        backend.stop(owner: nil)
        guard connected else { return }
        connected = false; grants = []; output = []
        observationTokens = [:]; extensionTokens = [:]; normalizers = [:]
        // Even on storage failure, never resume effects. Reopen must reconcile.
        do {
            try cancelPending(status: "cancelled", code: "cancelled", now: lastClock, deliver: false)
            if ledger.epoch < UInt64.max { ledger.epoch += 1 }
            try ledger.save()
        } catch { /* no further receipt or work is permitted on this instance */ }
    }

    /// Explicit trusted local-operator API. It cannot resume this connection or
    /// install grants; create a NEW connection/session and negotiate after reset.
    /// Never expose this method to a coglet, provider or hub message dispatcher.
    public func localOperatorResetForNextConnection() throws {
        guard !retired else { throw SurfaceFailure.closed }
        guard !connected else { throw SurfaceFailure.protocolViolation("reset requires closed connection") }
        backend.stop(owner: nil)
        retired = true
        ledger.killed = false
        ledger.running = false
        do { try ledger.save() }
        catch { ledger.killed = true; ledger.running = true; throw error }
    }

    private func guarded(_ operation: () throws -> Void) throws {
        do { try operation() }
        catch { disconnect(); throw error }
    }
    private func sample() throws -> UInt64 {
        let now = clock()
        guard now >= lastClock else { throw SurfaceFailure.protocolViolation("surface clock rollback") }
        lastClock = now; return now
    }
    private func emit(_ value: CBORValue, observationOwner: String? = nil, extensionOperation: String? = nil) throws {
        guard output.count < SurfaceWire.maximumQueuedMessages else { throw SurfaceFailure.exhausted }
        output.append((try SurfaceWire.encode(value, from: .surface, contractProfile: wireProfile), observationOwner, extensionOperation))
    }
    private func publishCapabilities(now: UInt64) throws {
        let entries = Set(sessionOperations.values.map(\.entry)).sorted().map { entry in
            let selected = sessionOperations.values.filter { $0.entry == entry }.sorted { $0.name < $1.name }
            return CBORValue.record([("name", .text(entry)),
                ("events", .array(selected.filter { $0.direction == .event }.map { .text($0.name) })),
                ("reads", .array(selected.filter { $0.direction == .read }.map { .text($0.name) })),
                ("actions", .array(selected.filter { $0.direction == .action }.map { .text($0.name) })),
                ("exclusive", .array(Set(selected.flatMap(\.resources)).sorted().map(CBORValue.text))),
                ("schemas", .map(selected.map { operation in
                    if trustedCatalog?.operations.contains(where: { $0.name == operation.name }) == true {
                        return CBORMapEntry(operation.name, .text(trustedCatalog!.schemaURI(operation: operation.name)))
                    }
                    let rule = operation.direction == .read ? operation.outputRule! : operation.inputRule
                    let parts = rule.components(separatedBy: "::")
                    return CBORMapEntry(operation.name, .text("https://coglet.dev/spec/messages/" + parts[0] + ".schema.json#/$defs/" + parts[1]))
                }))])
        }
        try emit(.message("capabilities", .record([("revision", .unsigned(revision)), ("clock", .unsigned(now)),
            ("entries", .array(entries)), ("correlation_id", .text(correlation()))])))
    }
    private func publishGrants() throws {
        let allowed = sessionOperations
        let availableResources = Set(allowed.values.flatMap(\.resources))
        let selected = grants.map { grant in
            EffectiveSurfaceGrant(coglet: grant.coglet, operations: grant.operations.intersection(Set(allowed.keys)),
                resources: grant.resources.intersection(availableResources), expires: grant.expires)
        }
        try emit(.message("grants", .record([("epoch", .unsigned(ledger.epoch)),
            ("entries", .array(selected.map(\.value))), ("correlation_id", .text(correlation()))])))
    }
    private func validateGrants(_ replacement: [EffectiveSurfaceGrant], now: UInt64) throws {
        var owners = Set<String>(), resources = Set<String>()
        for grant in replacement {
            try CBORCodec.validate(contractProfile.rule("surface::grant"), grant.value)
            guard boundCoglets.contains(grant.coglet), grant.expires > now,
                  owners.insert(grant.coglet).inserted,
                  grant.operations.allSatisfy({ operations[$0] != nil }),
                  grant.resources.allSatisfy({ resources.insert($0).inserted }),
                  grant.resources.isSubset(of: Set(operations.values.flatMap(\.resources))) else {
                throw SurfaceFailure.protocolViolation("invalid grant snapshot")
            }
        }
    }
    private func authorize(owner: String, operation: SurfaceOperation, epoch: UInt64, deadline: UInt64, now: UInt64) throws {
        guard sessionOperations[operation.name] != nil, boundCoglets.contains(owner), !ledger.killed, epoch == ledger.epoch,
              let grant = grants.first(where: { $0.coglet == owner }), grant.expires > now,
              grant.operations.contains(operation.name), Set(operation.resources).isSubset(of: grant.resources) else {
            throw NativeSurfaceError("permission_denied")
        }
        guard deadline > now else { throw NativeSurfaceError("deadline_exceeded") }
        guard deadline - now <= 60_000_000 else { throw NativeSurfaceError("invalid_argument") }
    }
    private func read(body: CBORValue, now: UInt64) throws {
        let id = body["correlation_id"]!, name = body["capability"]!.textValue!, owner = body["coglet"]!.textValue!
        do {
            guard let operation = operations[name], operation.direction == .read else { throw NativeSurfaceError("permission_denied") }
            try authorize(owner: owner, operation: operation, epoch: body["grant_epoch"]!.uintValue!, deadline: body["deadline"]!.uintValue!, now: now)
            do { try check(operation, output: false, value: body["args"]!) }
            catch { throw NativeSurfaceError("invalid_argument") }
            let value = try backend.read(operation: name, args: body["args"]!, owner: owner, now: now)
            try check(operation, output: true, value: value)
            try authorize(owner: owner, operation: operation, epoch: body["grant_epoch"]!.uintValue!, deadline: body["deadline"]!.uintValue!, now: sample())
            if name == "speech.heard", value["item"] != body["args"]?["item"] {
                throw SurfaceFailure.protocolViolation("heard result item does not match retained query")
            }
            try emit(.message("read_reply", .record([("correlation_id", id), ("ok", .boolean(true)),
                ("value", value), ("snapshot", .unsigned(revision))])))
        } catch let error as NativeSurfaceError {
            try emit(.message("read_reply", .record([("correlation_id", id), ("ok", .boolean(false)), ("error", error.value)])))
        }
    }
    private func act(message: CBORValue, bytes: [UInt8], now: UInt64) throws {
        let body = message["body"]!, action = body["action"]!, owner = action["coglet"]!.textValue!
        let key = owner + "/" + action["id"]!.textValue!
        // Authenticated binding precedes historical lookup, current grants do not.
        guard boundCoglets.contains(owner), sessionOperations[action["capability"]!.textValue!] != nil else {
            try emit(.message("accept", accept(action, error: NativeSurfaceError("permission_denied"))))
            return
        }
        if let record = ledger.records[key] {
            guard record.envelope == bytes else {
                try emit(.message("accept", accept(action, error: NativeSurfaceError("conflict"))))
                return
            }
            try emit(.message("accept", record.accept))
            if let outcome = record.outcome { try emitOutcome(outcome, now: now) }
            return
        }
        // If the ledger cannot retain a rejection, disconnect rather than issue
        // an unrecorded receipt whose identical retry could later execute.
        guard ledger.records.count < 1024 else { throw SurfaceFailure.exhausted }
        let name = action["capability"]!.textValue!
        var rejection: NativeSurfaceError?
        do {
            guard let operation = operations[name], operation.direction == .action else { throw NativeSurfaceError("permission_denied") }
            try authorize(owner: owner, operation: operation, epoch: body["grant_epoch"]!.uintValue!, deadline: action["deadline"]!.uintValue!, now: now)
            do {
                try check(operation, output: false, value: action["args"]!)
                if name == "speech.speak_chunk", let text = action["args"]?["text"]?.textValue,
                   text.utf8.count > 4096 { throw NativeSurfaceError("invalid_argument") }
            } catch { throw NativeSurfaceError("invalid_argument") }
            guard pending < SurfaceWire.maximumQueuedMessages else { throw NativeSurfaceError("resource_exhausted") }
            try backend.preflight(action: action, now: now)
        } catch let error as NativeSurfaceError { rejection = error }
        let receipt = accept(action, error: rejection)
        ledger.records[key] = .init(envelope: bytes, accept: receipt)
        try ledger.save() // admission/rejection before ack or physical effect
        try emit(.message("accept", receipt))
        guard rejection == nil else { return }
        let result: NativeSurfaceResult?
        do {
            let effectTime = try sample()
            try authorize(owner: owner, operation: operations[name]!, epoch: body["grant_epoch"]!.uintValue!, deadline: action["deadline"]!.uintValue!, now: effectTime)
            result = try backend.begin(action: action, epoch: ledger.epoch, now: effectTime)
        }
        catch {
            backend.stop(owner: owner)
            try terminal(key: key, status: "failed", value: NativeSurfaceError("unavailable").value, now: now)
            return
        }
        if let result {
            try complete(owner: owner, id: action["id"]!.textValue!, epoch: ledger.epoch, result: result)
        }
    }
    private var pending: Int { ledger.records.values.filter { $0.accept["accepted"] == .boolean(true) && $0.outcome == nil }.count }
    private func check(_ operation: SurfaceOperation, output: Bool, value: CBORValue) throws {
        if trustedCatalog?.operations.contains(where: { $0.name == operation.name }) == true {
            try trustedCatalog!.check(operation: operation.name, output: output, value: value)
        } else {
            try CBORCodec.validate(output ? operation.outputRule! : operation.inputRule, value)
        }
    }
    private func accept(_ action: CBORValue, error: NativeSurfaceError?) -> CBORValue {
        var entries: [(String, CBORValue)] = [("id", action["id"]!), ("correlation_id", action["correlation_id"]!), ("accepted", .boolean(error == nil))]
        if let error { entries.append(("error", error.value)) }
        return .record(entries)
    }
    private static func outcome(record: SurfaceActionRecord, status: String, result: CBORValue) -> CBORValue {
        .record([("id", record.accept["id"]!), ("correlation_id", record.accept["correlation_id"]!),
            ("status", .text(status)), (status == "succeeded" ? "result" : "error", result)])
    }
    private func terminal(key: String, status: String, value: CBORValue, now: UInt64, deliver: Bool = true) throws {
        guard let record = ledger.records[key], record.outcome == nil else { return }
        let outcome = Self.outcome(record: record, status: status, result: value)
        try CBORCodec.validate("common::action-outcome", outcome)
        ledger.records[key]?.outcome = outcome
        try ledger.save()
        if deliver { try emitOutcome(outcome, now: now) }
    }
    private func emitOutcome(_ outcome: CBORValue, now: UInt64) throws {
        try emitEvent(kind: "action.outcome", payload: outcome, correlationID: outcome["correlation_id"]!.textValue!, now: now)
    }
    private func emitEvent(kind: String, payload: CBORValue, correlationID: String, now: UInt64, observationOwner: String? = nil, extensionOperation: String? = nil, beforeEmit: (() throws -> Void)? = nil) throws {
        guard ledger.sequence < UInt64.max else { throw SurfaceFailure.exhausted }
        let seq = ledger.sequence; ledger.sequence += 1
        try ledger.save()
        try beforeEmit?()
        try emit(.message("event", .record([("source", .text(ledger.identity)), ("kind", .text(kind)),
            ("seq", .unsigned(seq)), ("logical_time", .unsigned(now)), ("payload", payload),
            ("correlation_id", .text(correlationID))])), observationOwner: observationOwner, extensionOperation: extensionOperation)
    }
    private func cancelPending(status: String, code: String, now: UInt64, deliver: Bool = true) throws {
        for (key, record) in ledger.records where record.accept["accepted"] == .boolean(true) && record.outcome == nil {
            try terminal(key: key, status: status, value: NativeSurfaceError(code).value, now: now, deliver: deliver)
        }
    }
    private func emitHealth(now: UInt64) throws {
        try emit(.message("health", .record([("state", .text(ledger.killed ? "stopped" : "ready")),
            ("clock", .unsigned(now)), ("pending", .unsigned(UInt64(pending))), ("correlation_id", .text(correlation()))])))
    }
}
