import Foundation
import CogletSurface
import CogletTypes

/// Session/backend/timer/link must outlive a nonisolated @MainActor deinit so
/// stop and destruction run on MainActor, including forgotten close.
private final class FaceHostLifetime: @unchecked Sendable {
    var backend: FaceSurfaceBackend?
    var session: SurfaceSession?
    var timer: Task<Void, Never>?
    var link: (any FaceControlLink)?
    @MainActor func release() {
        // Retired sessions already stopped under explicit close/reset. Never
        // repeat their native stop after a successor has acquired the device.
        if let session { session.disconnect() } else { backend?.stop(owner: nil) }
        timer?.cancel(); timer = nil
        link?.close(); link = nil
        session = nil
        backend = nil
    }
}

/// Embeddable serialized Face host. The embedding app supplies authenticated
/// transport bytes, durable ledger, monotonic microsecond clock and local policy.
/// It must never dispatch guest execution on MainActor. No network route or
/// product credentials are selected here.
@MainActor public final class FaceSurfaceHost {
    public static let operations = SurfaceOperation.builtins.filter {
        $0.entry == "speech.control" || $0.entry == "expression.control"
    }
    private let lifetime = FaceHostLifetime()
    private var backend: FaceSurfaceBackend { lifetime.backend! }
    private var session: SurfaceSession { lifetime.session! }
    private var timer: Task<Void, Never>? {
        get { lifetime.timer }
        set { lifetime.timer = newValue }
    }
    private var link: (any FaceControlLink)? {
        get { lifetime.link }
        set { lifetime.link = newValue }
    }
    private let owner: String
    private let clock: () -> UInt64
    private var grants: [EffectiveSurfaceGrant]
    private var token: NativeObservationToken?
    private var pendingWire: [[UInt8]] = []
    private var transportEpoch: UInt64 = 0
    private var writePending = false
    private var started = false
    public private(set) var lastFailure: (any Swift.Error)?

    public init(identity: String, owner: String, store: any SurfaceLedgerStore, backend: FaceSurfaceBackend,
                grants: [EffectiveSurfaceGrant], clock: @escaping () -> UInt64,
                historyStore: (any SurfaceLedgerStore)? = nil) throws {
        self.owner = owner; self.grants = grants; self.clock = clock
        if let historyStore, ObjectIdentifier(historyStore) == ObjectIdentifier(store) {
            throw NativeSurfaceError("invalid_argument")
        }
        let operations = Self.operations + (historyStore == nil ? [] : SurfaceOperation.builtins.filter { $0.entry == "speech.history" })
        lifetime.backend = backend
        lifetime.session = try SurfaceSession(identity: identity, boundCoglets: [owner], operations: operations,
            store: store, backend: backend, initialGrants: grants, clock: clock)
        try backend.configureHistory(store: historyStore)
        backend.retainClosedStreams(from: try session.retainedAdmittedActions())
        backend.installCaptureClock(clock)
        backend.installEffectAuthority { [weak self] in
            guard let self else { throw NativeSurfaceError("unavailable") }
            // SDK tick expires grants and pending actions before we judge them.
            try self.session.tick()
            let now = self.clock()
            guard let grant = self.grants.first(where: { $0.coglet == self.owner }), grant.expires > now else {
                throw NativeSurfaceError("permission_denied")
            }
            return (now, self.session.grantEpoch)
        }
    }
    deinit {
        let lifetime = lifetime
        Task { @MainActor in lifetime.release() }
    }
    public var grantEpoch: UInt64 { session.grantEpoch }
    /// Canonical capture-frame timeline for configured native recognition routes.
    public var capturePositionMS: UInt64 { backend.capturePositionMS }
    /// Attach once, before start. Authenticated socket and embedded ABI adapters
    /// use exactly the same host and deadline/native-stop paths.
    public func attach(link: any FaceControlLink) throws {
        guard !started, self.link == nil else { throw NativeSurfaceError("conflict") }
        self.link = link; transportEpoch = session.grantEpoch
    }
    public func start() throws {
        guard !started else { throw NativeSurfaceError("conflict") }
        started = true
        try guarded { try session.start(); try serviceLink() }
        timer = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .milliseconds(20)) } catch { return }
                guard let self else { return }
                do { try self.pump() } catch { self.lastFailure = error; self.disconnect(); return }
            }
        }
    }
    public func drain() -> [[UInt8]] { link == nil ? session.drain() : [] }
    public func receive(_ bytes: [UInt8]) throws {
        try guarded { try session.tick(); try session.receive(bytes); try deliverCompletions() }
    }
    /// Local permission is explicit and revocable. OS microphone permission is
    /// checked by the injected audio kit as well. Split-owner duplex is refused.
    public func activateDuplex(localConsent: Bool, remoteAudioConsent: Bool = false) throws {
        guard localConsent, let grant = grants.first(where: { $0.coglet == owner }),
              grant.expires > clock(), Set(["audio.mic", "audio.speaker"]).isSubset(of: grant.resources),
              Set(["speech.start", "speech.partial", "speech.end", "speech.position", "speech.barge_in"]).isSubset(of: grant.operations)
        else { throw NativeSurfaceError("permission_denied") }
        try guarded {
            // Replacing native observation custody also fences the external link
            // backlog, even when the effective grant epoch stays unchanged.
            if writePending, let link { writePending = try !link.flush() }
            guard pendingWire.isEmpty, !writePending else { throw SurfaceFailure.closed }
            let captured = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
            token = captured
            let session = self.session
            backend.activate(owner: owner, epoch: session.grantEpoch, remoteAudioConsent: remoteAudioConsent) { [weak session] kind, payload in
                _ = try session?.publishObservation(captured, kind: kind, payload: payload)
            }
            try backend.startDuplex()
        }
    }
    /// Hold-to-talk / native VAD entrypoint. The route implementation is supplied
    /// by trusted product configuration, never by a coglet action.
    @discardableResult public func beginCapture(recognition: any FaceRecognition, positionMS: UInt64) throws -> String {
        guard token != nil else { throw NativeSurfaceError("permission_denied") }
        let utterance = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        try guarded {
            try session.tick()
            try backend.beginCapture(recognition: recognition, utterance: utterance, positionMS: positionMS, now: clock())
            try deliverCompletions()
        }
        return utterance
    }
    public func enableVoiceDetection(recognition: any FaceRecognition) throws {
        guard let captured = token else { throw NativeSurfaceError("permission_denied") }
        backend.setVoiceActivityHook { [weak self] active in
            guard let self, self.token == captured else { return }
            do {
                if active {
                    _ = try self.beginCapture(recognition: recognition, positionMS: self.backend.capturePositionMS)
                } else { self.finishCapture() }
            } catch { self.lastFailure = error; self.disconnect() }
        }
    }
    public func finishCapture() { backend.finishCapture(now: clock()) }
    public func installGrants(_ replacement: [EffectiveSurfaceGrant]) throws {
        try guarded { try session.installGrants(replacement); grants = replacement; token = nil }
    }
    /// Native timer order matters: expire grants/actions BEFORE any new audio or
    /// completion is released. Session.complete independently resamples deadlines.
    public func pump() throws {
        try guarded {
            try session.tick(); try backend.tick(now: clock())
            try backend.pollCapture(); try deliverCompletions()
            try serviceLink()
        }
    }
    private func serviceLink() throws {
        guard let link else { return }
        // An asynchronous embedded worker may have drained the previous mailbox
        // since the last native tick. Resample before evaluating epoch fencing;
        // never erase queued bytes merely because authority changed.
        if writePending { writePending = try !link.flush() }
        try synchronizeTransportEpoch()
        // Bound work per native tick so a busy control peer cannot starve reflex
        // processing. The SDK separately bounds frames and message queues.
        for _ in 0..<64 {
            guard let bytes = try link.receive() else { break }
            try session.tick(); try session.receive(bytes); try deliverCompletions()
        }
        try synchronizeTransportEpoch()
        let outgoing = session.drain()
        guard pendingWire.count + outgoing.count <= SurfaceWire.maximumQueuedMessages else { throw SurfaceFailure.exhausted }
        pendingWire.append(contentsOf: outgoing)
        while let bytes = pendingWire.first {
            guard try link.send(bytes) else { break }
            pendingWire.removeFirst()
        }
        writePending = try !link.flush()
    }
    private func synchronizeTransportEpoch() throws {
        guard transportEpoch != session.grantEpoch else { return }
        // A partially framed socket message cannot safely be purged. Fail closed
        // rather than release old-owner private bytes after a grant fence.
        guard pendingWire.isEmpty, !writePending else { throw SurfaceFailure.closed }
        transportEpoch = session.grantEpoch
    }
    private func closeLink() {
        link?.close(); pendingWire = []; writePending = false
    }
    private func deliverCompletions() throws {
        for completion in backend.takeCompletions() {
            try session.complete(owner: completion.owner, id: completion.id, epoch: completion.epoch, result: completion.result)
        }
    }
    public func localOffSwitch() throws {
        backend.stop(owner: nil) // synchronous native stop before ledger I/O
        timer?.cancel(); timer = nil; token = nil
        defer { closeLink() }
        try session.kill(reason: "local Face off switch")
        // Best effort on the existing byte stream only. Never discard a partial
        // frame tail then append a kill frame, nor release private retry backlog.
        if let link, !writePending, pendingWire.isEmpty {
            for bytes in session.drain() {
                guard (try? link.send(bytes)) == true else { break }
            }
            _ = try? link.flush()
        }
    }
    public func disconnect() {
        backend.stop(owner: nil); timer?.cancel(); timer = nil; token = nil
        closeLink()
        session.disconnect()
    }
    public func close() throws {
        backend.stop(owner: nil); timer?.cancel(); timer = nil; token = nil
        closeLink()
        try session.close()
    }
    private func guarded(_ body: () throws -> Void) throws {
        do { try body() } catch { disconnect(); throw error }
    }
}
