#if DEBUG
import Foundation
import Darwin
import CogletSurface
import SoftmaxAppSurface

private struct WireProofFailure: Swift.Error {}

/// Test-only owner of the EXISTING native app registry, not a second app model.
/// The runtime installation is excluded for this exact isolated proof launch.
@MainActor
final class AppSurfaceWireProof {
    private let model: ShellModel
    private let adapter: AppSurfaceAdapter
    private let refresh: @MainActor () throws -> Void
    private let mailbox = AppSurfaceMailbox()
    private let directory: URL
    private var task: Task<Void, Never>?
    private var attached = false
    private var retired = false

    init(model: ShellModel, directory: URL, refresh: @escaping @MainActor () throws -> Void) throws {
        guard model.testing, model.runtime.state == .stopped, model.appControls.onChange == nil,
              let adapter = model.appSurface else { throw WireProofFailure() }
        self.model = model; self.adapter = adapter; self.directory = directory; self.refresh = refresh
    }
    func start() {
        guard task == nil else { return }
        let executor = WireProofExecutor(directory: directory, mailbox: mailbox)
        task = Task { [weak self] in
            guard let self else { return }
            // Fixed lifecycle markers are emitted only AFTER path validation.
            // A normal peer EOF must not latch a native registration failure.
            try? await executor.serve(native: self)
        }
    }
    func stop() { mailbox.retire(); retired = true; task?.cancel(); detach() }
    private func check() throws {
        guard !retired, model.testing, model.runtime.state == .stopped,
              !model.appControlRegistrationFailed else { throw WireProofFailure() }
    }
    func capture() throws -> AppSurfaceCapture {
        try check(); try refresh(); return try adapter.capture()
    }
    func attach(incarnation: UInt64) throws {
        try check()
        guard !attached, model.appControls.onChange == nil else { throw WireProofFailure() }
        model.appControls.onChange = { [mailbox] operation, payload in
            do { try mailbox.observe(operation: operation, payload: payload) }
            catch { mailbox.retire() }
        }
        attached = true
        // This post-attachment baseline MUST precede capabilities/grants egress.
        try mailbox.publish(capture(), incarnation: incarnation)
    }
    func pump(incarnation: UInt64) throws {
        try check()
        guard attached else { throw WireProofFailure() }
        for ticket in mailbox.queuedCommands() { mailbox.execute(ticket, adapter: adapter) }
        try mailbox.publish(capture(), incarnation: incarnation)
        // Independent witness in the real shared native button handler. Not an
        // adapter observation, outcome, test proxy or duplicate side-effect path.
        try Data(String(model.wireProofShortcutPresses).utf8)
            .write(to: directory.appendingPathComponent("native-press-count"), options: .atomic)
    }
    func detach() {
        // Retire synchronously BEFORE dropping the listener or closing the stream.
        mailbox.retire(); retired = true
        if attached { model.appControls.onChange = nil; attached = false }
    }
}

/// Every SDK/ledger/socket call is confined to ONE off-main owner. No session
/// exists during the potentially slow UI-test peer startup. No reconnect/reset.
private actor WireProofExecutor {
    let directory: URL
    let mailbox: AppSurfaceMailbox
    private var listener: SurfaceSocketListener?
    private var transport: SurfaceSocketTransport?
    private var session: SurfaceSession?
    private var connection: AppSurfaceConnection?
    private var backend: AppSurfaceBackend?
    private var grant: EffectiveSurfaceGrant?
    private var operations: [SurfaceOperation] = []
    private var catalog: TrustedExtensionCatalog?
    private var running = false
    private var pathValidated = false
    init(directory: URL, mailbox: AppSurfaceMailbox) { self.directory = directory; self.mailbox = mailbox }
    private func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds / 1_000 }

    private func prepare() throws {
        // lstat rejects a symlink, even one resolving into the private temp root.
        var info = stat()
        let root = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().path + "/"
        guard directory.path == directory.resolvingSymlinksInPath().path,
              directory.path.hasPrefix(root), lstat(directory.path, &info) == 0,
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR), info.st_mode & 0o777 == 0o700,
              info.st_uid == getuid(),
              (directory.path + "/s/surface.sock").utf8.count < 100,
              let resource = Bundle.main.url(forResource: "catalog.v1", withExtension: "json") else {
            throw WireProofFailure()
        }
        pathValidated = true
        // The pin is reviewed product source, never derived from CLI/peer bytes.
        catalog = try TrustedExtensionCatalog(json: Array(Data(contentsOf: resource)),
            sha256: AppSurfaceProvisioning.catalogPin, contractProfile: .surface02)
        let names: Set<String> = ["softmax.app.state", "softmax.app.controls", "softmax.app.navigate",
            "softmax.app.open_window", "softmax.app.close_window", "softmax.app.set_focus",
            "softmax.app.invoke", "softmax.app.set_setting", "softmax.app.view_changed",
            "softmax.app.focus_changed", "softmax.app.control_changed", "softmax.app.window_changed"]
        operations = catalog!.operations.filter { names.contains($0.name) }
        guard operations.count == names.count else { throw WireProofFailure() }
        listener = try SurfaceSocketListener(directory: directory.appendingPathComponent("s").path,
            runtimeUID: getuid(), contractProfile: .surface02)
        try Data(listener!.path.utf8).write(to: directory.appendingPathComponent("socket-path"), options: .atomic)
    }
    private func accept() throws -> Bool {
        guard let accepted = try listener?.acceptConnection() else { return false }
        transport = accepted; listener?.close(); listener = nil
        // Ledger child is NEW, never the installed runtime directory/host.lock.
        let ledger = directory.appendingPathComponent("l")
        let store = try FileSurfaceLedger(directory: ledger, newIdentity: true)
        let owner = "coglet:" + String(repeating: "b", count: 64)
        let authority = EffectiveSurfaceGrant(coglet: owner, operations: Set(operations.map(\.name)),
            resources: ["softmax.app_control"], expires: now() + 600_000_000)
        grant = authority
        let native = AppSurfaceBackend(mailbox: mailbox); backend = native
        let surface = try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64),
            boundCoglets: [owner], operations: operations, store: store, backend: native,
            contractProfile: .surface02, trustedCatalog: catalog, initialGrants: [authority],
            clock: { DispatchTime.now().uptimeNanoseconds / 1_000 })
        session = surface
        try surface.start()
        let hello = surface.drain()
        guard hello.count == 1 else { throw WireProofFailure() }
        try accepted.enqueue(hello[0])
        // A blocked/partial writer is terminal. Never retain it across an await.
        guard try accepted.flush() else { throw WireProofFailure() }
        return true
    }
    private func select() throws -> Bool {
        guard let session, let transport else { throw WireProofFailure() }
        guard let selection = try transport.receive() else { return false }
        try session.receive(selection)
        // Capabilities/grants queued by receive stay INSIDE SDK until attach.
        return true
    }
    private func activate(capture: AppSurfaceCapture) throws -> UInt64 {
        guard let session, let grant else { throw WireProofFailure() }
        let active = try AppSurfaceConnection(session: session, mailbox: mailbox,
            grant: grant, operations: operations, capture: capture)
        connection = active
        return active.incarnation
    }
    private func flush() throws {
        guard let session, let transport, let connection else { throw WireProofFailure() }
        try connection.validate(session)
        let custody = session.egressCustody
        for frame in session.drain() { try transport.enqueue(frame) }
        try connection.validate(session)
        guard session.egressCustody == custody, try transport.flush() else { throw WireProofFailure() }
        try connection.validate(session)
    }
    private func pump() throws {
        guard let session, let transport, let connection, let backend else { throw WireProofFailure() }
        try connection.validate(session)
        // Native receipts and events are copied by the preceding MainActor turn.
        try backend.reconcile(session)
        try connection.publishEvents(session)
        for _ in 0..<64 {
            guard let frame = try transport.receive() else { break }
            try session.receive(frame)
            try connection.validate(session)
        }
        try session.tick()
        try flush()
    }
    func serve(native: AppSurfaceWireProof) async throws {
        guard !running else { throw WireProofFailure() }
        running = true
        do {
            try prepare()
            let acceptUntil = now() + 30_000_000
            while try !accept() {
                guard now() < acceptUntil else { throw WireProofFailure() }
                try await Task.sleep(for: .milliseconds(20))
            }
            let selectUntil = now() + 2_500_000
            while try !select() {
                guard now() < selectUntil else { throw WireProofFailure() }
                try await Task.sleep(for: .milliseconds(10))
            }
            // No socket read, flush or tick can interleave these MainActor hops:
            // serve is this actor's ONLY externally callable operation.
            let capture = try await native.capture()
            let incarnation = try activate(capture: capture)
            try await native.attach(incarnation: incarnation)
            try flush()
            try Data("active".utf8).write(to: directory.appendingPathComponent("status"), options: .atomic)
            while !Task.isCancelled {
                try await native.pump(incarnation: incarnation)
                try pump()
                try await Task.sleep(for: .milliseconds(20))
            }
            throw WireProofFailure()
        } catch {
            mailbox.retire()
            await native.detach()
            session?.disconnect()
            transport?.close(); listener?.close()
            var closeSucceeded = false
            if let session {
                do { try session.close(); closeSucceeded = true } catch { /* fixed failure marker below */ }
            }
            connection = nil; session = nil; backend = nil; transport = nil; listener = nil
            running = false
            if pathValidated {
                // Fixed local lifecycle evidence only; never encode error data.
                try? Data((closeSucceeded ? "closed" : "close_failed").utf8)
                    .write(to: directory.appendingPathComponent("status"), options: .atomic)
            }
            throw error
        }
    }
}
#endif
