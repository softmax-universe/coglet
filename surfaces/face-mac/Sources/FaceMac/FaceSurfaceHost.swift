import Foundation
import CogletSurface
import CogletTypes

/// Embeddable serialized Face host. The embedding app supplies authenticated
/// transport bytes, durable ledger, monotonic microsecond clock and local policy.
/// It must never dispatch guest execution on MainActor. No network route or
/// product credentials are selected here.
@MainActor public final class FaceSurfaceHost {
    public static let operations = SurfaceOperation.builtins.filter {
        $0.entry == "speech.control" || $0.entry == "expression.control"
    }
    private let backend: FaceSurfaceBackend
    private let session: SurfaceSession
    private let owner: String
    private let clock: () -> UInt64
    private var grants: [EffectiveSurfaceGrant]
    private var token: NativeObservationToken?
    private var timer: Task<Void, Never>?
    public private(set) var lastFailure: (any Swift.Error)?

    public init(identity: String, owner: String, store: any SurfaceLedgerStore, backend: FaceSurfaceBackend,
                grants: [EffectiveSurfaceGrant], clock: @escaping () -> UInt64) throws {
        self.owner = owner; self.backend = backend; self.grants = grants; self.clock = clock
        session = try SurfaceSession(identity: identity, boundCoglets: [owner], operations: Self.operations,
            store: store, backend: backend, initialGrants: grants, clock: clock)
        backend.retainClosedStreams(from: try session.retainedAdmittedActions())
    }
    public var grantEpoch: UInt64 { session.grantEpoch }
    public func start() throws {
        try session.start()
        timer = Task { @MainActor [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .milliseconds(20)) } catch { return }
                guard let self else { return }
                do { try self.pump() } catch { self.lastFailure = error; self.disconnect(); return }
            }
        }
    }
    public func drain() -> [[UInt8]] { session.drain() }
    public func receive(_ bytes: [UInt8]) throws {
        try guarded { try session.tick(); try session.receive(bytes); try deliverCompletions() }
    }
    /// Local permission is explicit and revocable. OS microphone permission is
    /// checked by the injected audio kit as well. Split-owner duplex is refused.
    public func activateDuplex(localConsent: Bool) throws {
        guard localConsent, let grant = grants.first(where: { $0.coglet == owner }),
              grant.expires > clock(), Set(["audio.mic", "audio.speaker"]).isSubset(of: grant.resources),
              Set(["speech.start", "speech.partial", "speech.end", "speech.position", "speech.barge_in"]).isSubset(of: grant.operations)
        else { throw NativeSurfaceError("permission_denied") }
        try guarded {
            let captured = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
            token = captured
            backend.activate(owner: owner, epoch: session.grantEpoch) { [weak session] kind, payload in
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
            try backend.beginCapture(recognition: recognition, utterance: utterance, positionMS: positionMS)
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
    public func finishCapture() { backend.finishCapture() }
    public func installGrants(_ replacement: [EffectiveSurfaceGrant]) throws {
        try guarded { try session.installGrants(replacement); grants = replacement; token = nil }
    }
    /// Native timer order matters: expire grants/actions BEFORE any new audio or
    /// completion is released. Session.complete independently resamples deadlines.
    public func pump() throws {
        try guarded {
            try session.tick(); try backend.tick(now: clock())
            try backend.pollCapture(); try deliverCompletions()
        }
    }
    private func deliverCompletions() throws {
        for completion in backend.takeCompletions() {
            try session.complete(owner: completion.owner, id: completion.id, epoch: completion.epoch, result: completion.result)
        }
    }
    public func localOffSwitch() throws {
        backend.stop(owner: nil) // synchronous native stop before ledger I/O
        timer?.cancel(); timer = nil; token = nil
        try session.kill(reason: "local Face off switch")
    }
    public func disconnect() {
        backend.stop(owner: nil); timer?.cancel(); timer = nil; token = nil
        session.disconnect()
    }
    public func close() throws {
        backend.stop(owner: nil); timer?.cancel(); timer = nil; token = nil
        try session.close()
    }
    private func guarded(_ body: () throws -> Void) throws {
        do { try body() } catch { disconnect(); throw error }
    }
}
