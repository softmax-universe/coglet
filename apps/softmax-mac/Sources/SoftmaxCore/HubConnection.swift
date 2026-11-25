import Foundation

public enum HubLinkEvent: Sendable { case ready(device: String), connected, offline, failed }
public enum HubConnectionState: String, Sendable { case offline, connecting, connected }
@MainActor
public protocol HubLink: AnyObject {
    func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws
    func connect(_ pairing: HubPairing) throws
    func stop()
}

@MainActor
public final class HubConnection {
    public private(set) var state: HubConnectionState = .offline
    public private(set) var deviceID: String?
    public private(set) var pairing: HubPairing?
    public private(set) var errorMessage: String?
    /// Shared projection of the Settings sign-out action's visibility.
    public var canSignOut: Bool { deviceID != nil || pairing != nil || errorMessage != nil }
    public var changed: (() -> Void)?
    private let account: HubAccount
    private let link: any HubLink
    private var generation: UInt64 = 0
    private var running = false
    private var ready = false
    private var retry: Task<Void, Never>?
    private var attempts = 0
    private var pending: HubPairing?
    private struct ReconnectCommand {
        let id: UUID
        let target: HubPairing
        let authorize: @MainActor () throws -> Void
        let completion: CheckedContinuation<Void, any Error>
    }
    private var reconnectCommand: ReconnectCommand?
    public init(account: HubAccount, link: any HubLink) { self.account = account; self.link = link }

    public func start() {
        guard !running else { return }
        generation &+= 1
        let current = generation
        errorMessage = nil
        do {
            let key = try account.identity()
            pairing = try account.store.loadHub()?.withoutInvite
            running = true
            try link.start(key: key) { [weak self] event in
                guard let self, self.running, self.generation == current else { return }
                self.receive(event)
            }
        } catch {
            stopLink()
            errorMessage = "Cannot load device identity or start hub connection. Retrying…"
            scheduleRetry(restart: true)
        }
        changed?()
    }
    public func enroll(_ value: HubPairing) throws {
        guard running, ready, let deviceID,
              value.device == nil || value.device == deviceID else { throw HubError.pairing }
        guard pairing == nil else { throw HubError.pairing }
        if pending != nil {
            guard state == .offline else { throw HubError.pairing }
            // A replacement must not accept a late success from the original channel.
            retry?.cancel(); retry = nil
            stopLink()
            pending = value; attempts = 0
            start()
            return
        }
        pending = value
        attempts = 0
        try connect(value)
    }
    private func connect(_ value: HubPairing) throws {
        retry?.cancel(); retry = nil
        state = .connecting; errorMessage = nil; changed?()
        do { try link.connect(value) }
        catch { state = .offline; errorMessage = "Could not contact the hub."; scheduleRetry(); changed?(); throw error }
    }
    public func retryNow() {
        guard reconnectCommand == nil else { return }
        guard running else { return }
        guard ready else {
            // A failed process needs a new generation; a connecting one must finish startup.
            return
        }
        guard let target = pending ?? pairing else { return }
        try? connect(target)
    }
    /// One explicit user/adapter reconnect, bound to a fresh helper generation.
    /// It does not swallow errors or complete merely because connect was queued.
    /// The caller must gate its original custody at every deferred native effect.
    public func reconnectOnce(authorized: @escaping @MainActor () throws -> Void = {}) async throws {
        try Task.checkCancellation()
        guard reconnectCommand == nil, pending == nil, let target = pairing else { throw HubError.unavailable }
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                var installed = false
                do {
                    try authorized()
                    retry?.cancel(); retry = nil
                    stopLink()
                    reconnectCommand = ReconnectCommand(id: id, target: target, authorize: authorized, completion: continuation)
                    installed = true
                    let key = try account.identity()
                    let current = generation
                    running = true; state = .connecting; errorMessage = nil
                    try link.start(key: key) { [weak self] event in
                        guard let self, self.running, self.generation == current else { return }
                        if self.reconnectCommand?.id == id { self.receiveReconnect(event, id: id) }
                        else { self.receive(event) }
                    }
                    changed?()
                } catch {
                    if installed { finishReconnect(id: id, error: error) }
                    else { continuation.resume(throwing: error) }
                }
            }
        } onCancel: {
            Task { @MainActor [weak self] in self?.finishReconnect(id: id, error: CancellationError()) }
        }
    }
    private func receiveReconnect(_ event: HubLinkEvent, id: UUID) {
        guard let command = reconnectCommand, command.id == id else { return }
        do {
            try command.authorize()
            switch event {
            case .ready(let device):
                guard HubPairing.isHex(device) else { throw HubError.unavailable }
                guard !ready else { return }
                deviceID = device; ready = true
                try link.connect(command.target)
            case .connected:
                guard ready else { throw HubError.unavailable }
                try account.store.saveHub(command.target.withoutInvite)
                try command.authorize() // persistence can block past native authority
                pairing = command.target.withoutInvite; state = .connected; attempts = 0; errorMessage = nil
                // Keep this exact generation as the normal live connection.
                reconnectCommand = nil
                command.completion.resume()
            case .offline, .failed: throw HubError.unavailable
            }
        } catch { finishReconnect(id: id, error: error) }
        changed?()
    }
    private func finishReconnect(id: UUID, error: any Error) {
        guard let command = reconnectCommand, command.id == id else { return }
        reconnectCommand = nil
        retry?.cancel(); retry = nil
        stopLink(); state = .offline
        errorMessage = "Could not complete router reconnection."
        command.completion.resume(throwing: error)
        changed?()
    }
    private func receive(_ event: HubLinkEvent) {
        switch event {
        case .ready(let device):
            guard HubPairing.isHex(device) else { receive(.failed); return }
            deviceID = device; ready = true
            retryNow()
        case .connected:
            guard let target = pending ?? pairing else { return }
            do {
                try account.store.saveHub(target.withoutInvite)
                pairing = target.withoutInvite; pending = nil
                state = .connected; attempts = 0; errorMessage = nil
            } catch {
                // Keep the pending invitation only in memory for ambiguous-success recovery.
                state = .offline; errorMessage = "Cannot save hub enrollment."; stopLink()
                scheduleRetry(restart: true)
            }
        case .offline:
            state = .offline
            if pending != nil { errorMessage = "Hub unavailable or pairing was not accepted. Retrying…" }
            scheduleRetry()
        case .failed:
            state = .offline; ready = false; errorMessage = "Hub connection stopped. Retrying…"
            stopLink()
            scheduleRetry(restart: true)
        }
        changed?()
    }
    private func stopLink() {
        generation &+= 1; link.stop(); running = false; ready = false
    }
    private func scheduleRetry(restart: Bool = false) {
        retry?.cancel()
        // The local pairing channel must also recover before enrollment.
        guard restart || pending != nil || pairing != nil else { return }
        let delay = min(30.0, pow(2.0, Double(min(attempts, 5))) * Double.random(in: 0.8...1.2))
        attempts = min(attempts + 1, 6)
        let current = generation
        retry = Task { [weak self] in
            do { try await Task.sleep(for: .seconds(delay)) } catch { return }
            guard let self, self.generation == current, !Task.isCancelled else { return }
            if restart { self.start() } else { self.retryNow() }
        }
    }
    public func signOut() {
        if let id = reconnectCommand?.id { finishReconnect(id: id, error: CancellationError()) }
        retry?.cancel(); retry = nil
        stopLink(); pending = nil; state = .offline
        // Keep deviceID until cleanup succeeds so the UI's Sign out stays reachable for retry.
        do { try account.signOut(); pairing = nil; deviceID = nil; errorMessage = nil }
        catch { errorMessage = "Sign out could not remove local identity or hub data. Try again." }
        changed?()
    }
    public func stop() {
        if let id = reconnectCommand?.id { finishReconnect(id: id, error: CancellationError()) }
        retry?.cancel(); retry = nil; stopLink(); pending = nil; state = .offline; changed?()
    }
}
