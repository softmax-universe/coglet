import Foundation
import CogletSurface
import CogletTypes

/// Safe immutable main-owner copy. This is a sampled revision, not a second
/// navigation model, native subscription, grant, or durable source identity.
public struct AppSurfaceCapture: Sendable {
    public let directory: AppSurfaceAdapter.CommandDirectory
    public let revision: UInt64
    fileprivate let state: CBORValue
    fileprivate let controls: [String: [CBORValue]]
    fileprivate let cursorIdentity: String

    fileprivate init(directory: AppSurfaceAdapter.CommandDirectory, revision: UInt64,
                     state: CBORValue, controls: [String: [CBORValue]]) {
        self.directory = directory; self.revision = revision
        self.state = state; self.controls = controls
        self.cursorIdentity = directory.sourceIdentity.uuidString + ":" + String(revision)
    }
    /// Validation/encoding/pagination runs on the SDK control owner, never by
    /// hopping synchronously back to a SwiftUI/native getter.
    func read(_ operation: String, args: CBORValue) throws -> CBORValue {
        if operation == "softmax.app.state" {
            try CBORCodec.validate("app-empty", args)
            try CBORCodec.validate("app-state-result", state)
            return state
        }
        guard operation == "softmax.app.controls" else { throw AppSurfaceFailure.unsupported }
        try CBORCodec.validate("app-controls-args", args)
        guard case .text(let window)? = args["window"], case .unsigned(let limit)? = args["limit"],
              let rows = controls[window] else { throw AppSurfaceFailure.unavailable }
        let start: Int
        if case .text(let after)? = args["after"] {
            let prefix = cursorIdentity + ":" + window + ":"
            guard args["revision"] == .unsigned(revision), after.hasPrefix(prefix),
                  let offset = Int(after.dropFirst(prefix.count)), offset > 0, offset < rows.count,
                  String(offset) == String(after.dropFirst(prefix.count)) else { throw AppSurfaceFailure.conflict }
            start = offset
        } else { start = 0 }
        let end = min(start + Int(limit), rows.count)
        let next: CBORValue = end < rows.count ? .text(cursorIdentity + ":" + window + ":" + String(end)) : .null
        let value = CBORValue.map([.init("revision", .unsigned(revision)), .init("window", .text(window)),
            .init("controls", .array(Array(rows[start..<end]))), .init("next", next)])
        try CBORCodec.validate("app-controls-result", value)
        guard try CBORCodec.encodeProfile(value).count <= 250_000 else { throw AppSurfaceFailure.exhausted }
        return value
    }
}

extension AppSurfaceAdapter {
    /// Bounded, allowlisted values only. Protected controls were redacted by the
    /// registry BEFORE any getter. Never call this from a backend/SDK worker.
    public func capture() throws -> AppSurfaceCapture {
        let snapshot = try read("softmax.app.state", args: .map([]))
        guard case .array(let windows)? = snapshot["windows"] else { throw AppSurfaceFailure.internal }
        var rows: [String: [CBORValue]] = [:], count = 0
        var encodedBytes = try CBORCodec.encodeProfile(snapshot).count
        let expected = registryRevision
        for window in windows {
            guard case .text(let name)? = window["window"] else { throw AppSurfaceFailure.internal }
            var all: [CBORValue] = [], after: CBORValue = .null
            repeat {
                let page = try read("softmax.app.controls", args: .map([.init("window", .text(name)),
                    .init("limit", .unsigned(128)), .init("after", after),
                    .init("revision", after == .null ? .null : .unsigned(expected))]))
                guard page["revision"] == .unsigned(expected), case .array(let controls)? = page["controls"],
                      let next = page["next"] else { throw AppSurfaceFailure.conflict }
                encodedBytes += try CBORCodec.encodeProfile(page).count
                count += controls.count
                guard count <= 4096, encodedBytes <= 1_048_576 else { throw AppSurfaceFailure.exhausted }
                all += controls; after = next
            } while after != .null
            rows[name] = all
        }
        guard registryRevision == expected, snapshot["revision"] == .unsigned(expected) else { throw AppSurfaceFailure.conflict }
        return AppSurfaceCapture(directory: captureCommandDirectory(), revision: expected, state: snapshot, controls: rows)
    }
}

/// Thread-safe native entry fence and bounded copied mailbox. Its caller is the
/// trusted host, not a guest. Activate ONLY after the SDK has durably installed
/// this exact epoch/grant and reserved the source incarnation, before subscribing.
/// No method constructs/expands a grant. One incarnation per instance; stop is
/// terminal after activation. SDK/transport ownership remains off-main.
public final class AppSurfaceMailbox: @unchecked Sendable {
    public struct Ticket: Sendable {
        fileprivate let identity: UUID
        fileprivate let command: AppSurfaceAdapter.CapturedCommand
        public let owner: String
        public let id: String
        public let epoch: UInt64
        public let deadline: UInt64
    }
    public struct Receipt: Sendable {
        public let ticket: Ticket
        public let result: NativeSurfaceResult
    }
    private enum Stage { case queued, entered }
    private struct Pending { let ticket: Ticket; var stage: Stage; var result: NativeSurfaceResult? }
    private let lock = NSLock()
    private var active = false, everActive = false, retired = false
    private var incarnation: UInt64?
    private var epoch: UInt64 = 0
    private var grant: EffectiveSurfaceGrant?
    private var capture: AppSurfaceCapture?
    private var pending: [UUID: Pending] = [:]
    private let clock: @Sendable () -> UInt64
    public init(clock: @escaping @Sendable () -> UInt64 = { DispatchTime.now().uptimeNanoseconds / 1_000 }) { self.clock = clock }

    public func activate(incarnation: UInt64, epoch: UInt64, grant: EffectiveSurfaceGrant, capture: AppSurfaceCapture) throws {
        lock.lock(); defer { lock.unlock() }
        guard !everActive, !retired, grant.expires > clock() else { throw NativeSurfaceError("unavailable") }
        self.incarnation = incarnation; self.epoch = epoch; self.grant = grant; self.capture = capture
        everActive = true; active = true
    }
    public func publish(_ snapshot: AppSurfaceCapture, incarnation: UInt64) throws {
        lock.lock(); defer { lock.unlock() }
        guard active, self.incarnation == incarnation, let old = capture,
              snapshot.directory.sourceIdentity == old.directory.sourceIdentity, snapshot.revision >= old.revision else {
            throw NativeSurfaceError("unavailable")
        }
        // Revision reuse must not reinterpret an outstanding page cursor. Native
        // providers must announce every committed mutation before republishing.
        guard snapshot.revision != old.revision || (snapshot.state == old.state && snapshot.controls == old.controls) else {
            throw NativeSurfaceError("conflict")
        }
        capture = snapshot
    }
    fileprivate func snapshot(requireCapacity: Bool = false) throws -> AppSurfaceCapture {
        lock.lock(); defer { lock.unlock() }
        guard active, let capture, let grant, grant.expires > clock() else { throw NativeSurfaceError("unavailable") }
        // Only the serialized SDK owner can enqueue; native code only completes
        // or drains, so this side-effect-free preflight check needs no reservation.
        if requireCapacity, pending.count >= 16 { throw NativeSurfaceError("resource_exhausted") }
        return capture
    }
    fileprivate func isCurrent(_ snapshot: AppSurfaceCapture) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return active && capture?.cursorIdentity == snapshot.cursorIdentity && (grant?.expires ?? 0) > clock()
    }
    fileprivate func enqueue(command: AppSurfaceAdapter.CapturedCommand, action: CogletTypes.CBORValue, epoch: UInt64) throws {
        guard case .text(let owner)? = action["coglet"], case .text(let id)? = action["id"],
              case .text(let operation)? = action["capability"], case .unsigned(let deadline)? = action["deadline"] else {
            throw NativeSurfaceError("invalid_argument")
        }
        lock.lock(); defer { lock.unlock() }
        guard active, self.epoch == epoch, let grant, grant.coglet == owner,
              grant.operations.contains(operation), grant.expires > clock(), deadline > clock() else { throw NativeSurfaceError("permission_denied") }
        guard pending.count < 16 else { throw NativeSurfaceError("resource_exhausted") }
        let ticket = Ticket(identity: UUID(), command: command, owner: owner, id: id, epoch: epoch, deadline: deadline)
        pending[ticket.identity] = Pending(ticket: ticket, stage: .queued)
    }
    public func queuedCommands() -> [Ticket] {
        lock.lock(); defer { lock.unlock() }
        return pending.values.filter { $0.stage == .queued && $0.result == nil }.map(\.ticket)
    }
    /// Linearization point is the successful `authorized` gate immediately before
    /// the synchronous native callback. Stop cancels not-yet-entered tickets and
    /// never waits on main/native code; it cannot undo an already-entered effect.
    /// No lock is held across getters, callbacks, encoding, disk or transport I/O.
    @MainActor public func execute(_ ticket: Ticket, adapter: AppSurfaceAdapter) {
        lock.lock()
        let queued = pending[ticket.identity].map { $0.stage == .queued && $0.result == nil } == true
        lock.unlock()
        guard queued else { return }
        let result: NativeSurfaceResult
        do {
            let prepared = try adapter.prepareCaptured(ticket.command)
            let value = try adapter.perform(prepared, authorized: {
                self.lock.lock(); defer { self.lock.unlock() }
                let now = self.clock()
                guard self.active, self.epoch == ticket.epoch, let grant = self.grant,
                      grant.coglet == ticket.owner, grant.expires > now, ticket.deadline > now,
                      self.pending[ticket.identity]?.stage == .queued,
                      self.pending[ticket.identity]?.result == nil else { return false }
                self.pending[ticket.identity]?.stage = .entered
                return true
            })
            result = .succeeded(try CogletTypes.CBORCodec.decodeProfile(CBORCodec.encodeProfile(value)))
        } catch { result = .failed(Self.failure(error)) }
        lock.lock(); defer { lock.unlock() }
        // Preserve a queued cancellation. Entered effects retain their actual
        // result; the SDK owns later expiry/kill/revocation disposition.
        if pending[ticket.identity]?.result == nil { pending[ticket.identity]?.result = result }
    }
    public func drainReceipts() -> [Receipt] {
        lock.lock(); defer { lock.unlock() }
        let completed = pending.values.compactMap { row in row.result.map { Receipt(ticket: row.ticket, result: $0) } }
        for receipt in completed { pending[receipt.ticket.identity] = nil }
        return completed
    }
    public func stop() {
        lock.lock(); defer { lock.unlock() }
        active = false; capture = nil; grant = nil
        if everActive { retired = true }
        for id in pending.keys where pending[id]?.stage == .queued && pending[id]?.result == nil {
            pending[id]?.result = .failed(NativeSurfaceError("cancelled"))
        }
    }
    fileprivate static func failure(_ error: any Swift.Error) -> NativeSurfaceError {
        if let native = error as? NativeSurfaceError { return native }
        switch error {
        case AppSurfaceFailure.permissionDenied, ControlRegistryFailure.permissionDenied: return .init("permission_denied")
        case AppSurfaceFailure.conflict, ControlRegistryFailure.conflict: return .init("conflict")
        case AppSurfaceFailure.invalidArguments, ControlRegistryFailure.invalidRegistration: return .init("invalid_argument")
        case AppSurfaceFailure.exhausted, ControlRegistryFailure.exhausted: return .init("resource_exhausted")
        default: return .init("unavailable")
        }
    }
}

/// Serialized SDK control-owner backend. Preflight records only an immutable
/// plan, with no native effect or capacity reservation. Begin consumes that EXACT
/// plan after durable SDK admission, never re-resolving a successor control.
public final class AppSurfaceBackend: SurfaceNativeBackend {
    public let mailbox: AppSurfaceMailbox
    private var prepared: (action: CogletTypes.CBORValue, command: AppSurfaceAdapter.CapturedCommand)?
    public init(mailbox: AppSurfaceMailbox) { self.mailbox = mailbox }
    public func preflight(action: CogletTypes.CBORValue, now: UInt64) throws {
        prepared = nil
        do {
            guard case .text(let operation)? = action["capability"], let args = action["args"] else { throw AppSurfaceFailure.invalidArguments }
            let nativeArgs = try CBORCodec.decodeProfile(CogletTypes.CBORCodec.encodeProfile(args))
            prepared = (action, try mailbox.snapshot(requireCapacity: true).directory.prepare(operation, args: nativeArgs))
        } catch { throw AppSurfaceMailbox.failure(error) }
    }
    public func begin(action: CogletTypes.CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        defer { prepared = nil }
        guard let prepared, prepared.action == action else { throw NativeSurfaceError("conflict") }
        try mailbox.enqueue(command: prepared.command, action: action, epoch: epoch)
        return nil
    }
    public func read(operation: String, args: CogletTypes.CBORValue, owner: String, now: UInt64) throws -> CogletTypes.CBORValue {
        do {
            let capture = try mailbox.snapshot()
            let input = try CBORCodec.decodeProfile(CogletTypes.CBORCodec.encodeProfile(args))
            let result = try capture.read(operation, args: input)
            let value = try CogletTypes.CBORCodec.decodeProfile(CBORCodec.encodeProfile(result))
            guard mailbox.isCurrent(capture) else { throw NativeSurfaceError("unavailable") }
            return value
        } catch { throw AppSurfaceMailbox.failure(error) }
    }
    public func stop(owner: String?) { prepared = nil; mailbox.stop() }
    public func reconcile(_ session: SurfaceSession) throws {
        for receipt in mailbox.drainReceipts() {
            try session.complete(owner: receipt.ticket.owner, id: receipt.ticket.id,
                epoch: receipt.ticket.epoch, result: receipt.result)
        }
    }
}
