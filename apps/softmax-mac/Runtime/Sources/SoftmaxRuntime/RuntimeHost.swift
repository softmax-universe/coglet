import Foundation
import Combine
import Darwin
import CogletSurface
import CogletTypes
import CogletRuntime
import class SoftmaxAppSurface.AppSurfaceAdapter
import class SoftmaxAppSurface.AppSurfaceMailbox
import class SoftmaxAppSurface.AppSurfaceConnection
import struct SoftmaxAppSurface.AppSurfaceCapture
import struct SoftmaxAppSurface.AppSurfaceProvisioning

public enum RuntimeState: String, Sendable { case running, stopped, error }
public struct InstalledCoglet: Identifiable, Sendable {
    public let id: String
    public let version: String
    public let status: RuntimeState
}
public struct RuntimeAction: Sendable, Equatable {
    public let capability: String
    public let text: String?
    public let expression: String?
    public let stream: String?
    public let chunk: UInt64?
    public let final: Bool
    public let intensity: UInt64?
    public let durationMS: UInt64?
    public let owner: String
    public let id: String
    public let epoch: UInt64
    public let deadline: UInt64
    let ticket = UUID()
    init(action: CBORValue, epoch: UInt64) {
        capability = action["capability"]!.hostText!
        text = action["args"]?["text"]?.hostText
        expression = action["args"]?["expression"]?.hostText
        stream = action["args"]?["stream"]?.hostText
        if case .unsigned(let value) = action["args"]?["chunk"] { chunk = value } else { chunk = nil }
        final = action["args"]?["final"] == .boolean(true)
        if case .unsigned(let value) = action["args"]?["intensity"] { intensity = value } else { intensity = nil }
        if case .unsigned(let value) = action["args"]?["duration_ms"] { durationMS = value } else { durationMS = nil }
        owner = action["coglet"]!.hostText!; id = action["id"]!.hostText!; self.epoch = epoch
        if case .unsigned(let value) = action["deadline"] { deadline = value } else { deadline = 0 }
    }
}
private extension CBORValue {
    var hostText: String? { if case .text(let value) = self { value } else { nil } }
}

private struct HostFailure: Swift.Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}


/// Every runtime pointer and SDK object is confined to this background actor.
/// No ABI entry happens on MainActor or a real-time audio callback.
private actor RuntimeExecutor {
    let root: URL
    private var link: EmbeddedSurfaceTransport?
    private var session: SurfaceSession?
    private let sink: PendingFaceSink
    private var backend: RuntimeSurfaceBackend?
    private var appConnection: AppSurfaceConnection?
    private var owner: String?
    private var token: NativeObservationToken?
    private var queued: [[UInt8]] = []
    private var lockFD: Int32 = -1
    private var active = false
    private var generation: UInt64?
    init(root: URL, output: RuntimeFaceOutput) { self.root = root; sink = PendingFaceSink(output: output) }
    private func now() -> UInt64 { DispatchTime.now().uptimeNanoseconds / 1_000 }
    private func log(_ message: String) {
        // Diagnostics never contain transcript/config/guest bytes.
        let url = root.appendingPathComponent("logs/runtime.log")
        let raw = Data(("\(Date().ISO8601Format()) \(message)\n").utf8)
        if !FileManager.default.fileExists(atPath: url.path) {
            FileManager.default.createFile(atPath: url.path, contents: nil, attributes: [.posixPermissions: 0o600])
        }
        if let file = try? FileHandle(forWritingTo: url) { defer { try? file.close() }; try? file.seekToEnd(); try? file.write(contentsOf: raw) }
    }
    private func check(_ result: Int32, _ operation: String) throws {
        guard result == 0 else { throw HostFailure("\(operation) failed (\(result)); see runtime log") }
    }
    private func sendJSON(_ data: Data, _ call: (UnsafePointer<UInt8>?, Int) -> Int32) -> Int32 {
        data.withUnsafeBytes { call($0.bindMemory(to: UInt8.self).baseAddress, data.count) }
    }
    func start(localReset: Bool, generation: UInt64, appMailbox: AppSurfaceMailbox? = nil,
               appCapture: AppSurfaceCapture? = nil) throws -> [InstalledCoglet] {
        guard !active else { throw HostFailure("Runtime already running") }
        let fm = FileManager.default
        try fm.createDirectory(at: root, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try fm.createDirectory(at: root.appendingPathComponent("logs"), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        lockFD = Darwin.open(root.appendingPathComponent("host.lock").path, O_CREAT | O_RDWR | O_CLOEXEC, 0o600)
        guard lockFD >= 0, flock(lockFD, LOCK_EX | LOCK_NB) == 0 else { cleanup(); throw HostFailure("Runtime directory already supervised") }
        do {
            let binding = try Data(contentsOf: root.appendingPathComponent("binding.json"))
            let configuration = try Data(contentsOf: root.appendingPathComponent("runtime.json"))
            guard let config = try JSONSerialization.jsonObject(with: configuration) as? [String: Any],
                  let setup = config["setup"] as? [String: Any], let manifest = setup["manifest"] as? [String: Any],
                  let id = manifest["id"] as? String,
                  let wal = config["wal"] as? String,
                  URL(fileURLWithPath: wal).standardizedFileURL.deletingLastPathComponent() == root.standardizedFileURL,
                  let bindingValue = try JSONSerialization.jsonObject(with: binding) as? [String: Any],
                  let identity = bindingValue["surface"] as? String,
                  bindingValue["permitted_coglets"] as? [String] == [id] else {
                throw HostFailure("Invalid installed runtime configuration (one pinned coglet per coordinator)")
            }
            let provision = try AppSurfaceProvisioning(configuration: configuration)
            guard !provision.enablesApp || (appMailbox != nil && appCapture != nil) else {
                throw HostFailure("App surface native owner not installed")
            }
            let operationNames = provision.operationNames, operations = provision.operations
            let composite = RuntimeSurfaceBackend(face: sink, mailbox: provision.enablesApp ? appMailbox : nil)
            backend = composite
            try check(sendJSON(binding, coglet_runtime_configure), "configure")
            try check(sendJSON(configuration, coglet_runtime_start), "start")
            active = true
            let ledgerDir = root.appendingPathComponent("surface")
            // Provisioning is an explicit installer operation, never inferred from
            // missing history at launch (including loss of the whole directory).
            let store = try FileSurfaceLedger(directory: ledgerDir, newIdentity: false)
            let resources = provision.resources
            let clock: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds / 1_000 }
            let grant = EffectiveSurfaceGrant(coglet: id, operations: operationNames, resources: resources, expires: now() + 86_400_000_000)
            func makeSession() throws -> SurfaceSession {
                try SurfaceSession(identity: identity, boundCoglets: [id], operations: operations, store: store,
                    backend: composite, contractProfile: provision.profile, trustedCatalog: provision.catalog,
                    initialGrants: [grant], clock: clock)
            }
            var surface = try makeSession()
            if surface.isKilled {
                guard localReset else { try surface.close(); throw HostFailure("Unclean surface restart: use Restart runtime for explicit local reconciliation") }
                surface.disconnect()
                try surface.localOperatorResetForNextConnection()
                surface = try makeSession()
            }
            owner = id; session = surface
            link = try EmbeddedSurfaceTransport(abi: .init(open: coglet_surface_open, send: coglet_surface_send, recv: coglet_surface_recv, close: coglet_surface_close), contractProfile: provision.profile)
            // Bootstrap sends ONLY hello, then consumes its selection. Caps and
            // grants stay in the SDK until all native/token custody is installed.
            try surface.start()
            guard let link, let hello = surface.drain().first, try link.send(hello), let selection = try link.receive() else {
                throw HostFailure("Embedded surface negotiation unavailable")
            }
            try surface.receive(selection)
            token = try surface.openNativeObservationSession(owner: id, epoch: surface.grantEpoch)
            if provision.enablesApp, let appMailbox, let appCapture {
                appConnection = try AppSurfaceConnection(session: surface, mailbox: appMailbox, grant: grant,
                    operations: operations, capture: appCapture)
            }
            sink.output.resume()
            self.generation = generation
            try pump()
            log("runtime started")
            return [.init(id: id, version: manifest["version"] as? String ?? "unknown", status: .running)]
        } catch {
            cleanup(); log("runtime startup failed"); throw error
        }
    }
    func pump() throws {
        guard let surface = session, let link else { return }
        try appConnection?.validate(surface)
        try backend?.reconcile(surface)
        try appConnection?.publishEvents(surface)
        for _ in 0..<128 {
            try appConnection?.validate(surface)
            let generated = surface.drain()
            guard queued.count + generated.count <= 128 else { throw HostFailure("Surface control queue exhausted") }
            queued += generated
            while let first = queued.first {
                // A full runtime outbound queue causes send=would-block. Keep
                // identical bytes, but drain inbound frames before retrying.
                let sent: Bool
                if let appConnection { sent = try appConnection.emit(session: surface) { try link.send(first) } }
                else { sent = try link.send(first) }
                guard sent else { break }
                queued.removeFirst()
            }
            try appConnection?.validate(surface)
            if let raw = try link.receive() {
                try appConnection?.validate(surface)
                try surface.receive(raw)
                try appConnection?.validate(surface)
            }
            else { break }
        }
        try check(coglet_runtime_status(), "runtime/guest")
    }
    func tick(generation expected: UInt64) throws -> [RuntimeAction] {
        guard generation == expected, let surface = session else { return [] }
        do {
            try pump()
            // No in-session renewal: local grants are finite. A healthy start or
            // explicit Restart issues the configured lease; neither expands policy.
            try surface.tick(); try pump()
            let result = sink.actions; sink.actions = []; return result
        } catch { cleanup(); log("runtime fault; restart required"); throw error }
    }
    func appSource(generation expected: UInt64) -> (incarnation: UInt64, epoch: UInt64)? {
        guard generation == expected, let appConnection else { return nil }
        return (appConnection.incarnation, appConnection.epoch)
    }
    func transcript(_ text: String, generation expected: UInt64, outputEpoch: UInt64) throws -> [RuntimeAction] {
        guard generation == expected else { return [] }
        guard let session, let token else { throw HostFailure("Runtime not connected") }
        do {
            // Drain old guest output while reflex cancellation is still fenced.
            try pump()
            guard sink.output.resume(ifCurrent: outputEpoch) else { return [] }
            let utterance = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
            guard try session.publishObservation(token, kind: "speech.start", payload: .record([("utterance", .text(utterance)), ("position_ms", .unsigned(0))])) else { throw HostFailure("Speech grant expired or revoked") }
            guard try session.publishObservation(token, kind: "speech.end", payload: .record([("utterance", .text(utterance)), ("revision", .unsigned(1)), ("text", .text(text)), ("position_ms", .unsigned(0))])) else { throw HostFailure("Speech grant expired or revoked") }
            try pump()
            let actions = sink.actions; sink.actions = []; return actions
        } catch { cleanup(); log("runtime speech delivery fault"); throw error }
    }
    func stop() throws {
        cleanup()
        try check(coglet_runtime_status() == 1 ? 0 : -2, "stop")
        log("runtime stopped")
    }
    private func cleanup() {
        backend?.stop(owner: nil); sink.stop(owner: nil); generation = nil; sink.actions = []
        // Fence native effects first, then persist settled BACK CHANNEL receipts
        // while the ADAPTER session is connected. close() cancels pending actions.
        // SDK completion failures kill the session; teardown must still finish.
        if let session {
            do { try backend?.reconcile(session) }
            catch { log("runtime output reconciliation failed; restart required") }
        }
        try? session?.close(); session = nil; token = nil; owner = nil; queued = []
        link?.close(); link = nil
        appConnection = nil; backend = nil
        if active { _ = coglet_runtime_stop() }; active = false
        if lockFD >= 0 { _ = flock(lockFD, LOCK_UN); _ = Darwin.close(lockFD); lockFD = -1 }
    }
    #if COGLET_RUNTIME_TEST_HOOKS
    func injectPanic() throws {
        do { try check(coglet_runtime_test_panic(), "injected panic") }
        catch { cleanup(); log("caught injected runtime panic"); throw error }
    }
    #endif
}

@MainActor public final class RuntimeHost: ObservableObject {
    @Published public private(set) var state: RuntimeState = .stopped
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var coglets: [InstalledCoglet] = []
    @Published public private(set) var actions: [RuntimeAction] = []
    public let directory: URL
    /// Capture BEFORE asynchronous recognition; lifecycle changes fence old input.
    public private(set) var generation: UInt64 = 0
    public var onAction: ((RuntimeAction) -> Void)?
    public var onStopOutput: (() -> Void)? {
        get { outputStop.callback }
        set { outputStop.callback = newValue }
    }
    private let outputStop: RuntimeOutputStop
    private let output: RuntimeFaceOutput
    private let executor: RuntimeExecutor
    private var polling: Task<Void, Never>?
    // Internal manual-pump mode isolates lifecycle tests from background ticks.
    // Production hosts always keep the default.
    var automaticallyPolls = true
    // Deterministic cancellation at the submission/actor handoff boundary.
    var beforeTranscriptHandoff: (() -> Void)?
    private var busy = false
    private var appAdapter: AppSurfaceAdapter?
    private var refreshApp: (() throws -> Void)?
    private var setAppEvents: (((@MainActor (String, SoftmaxAppSurface.CBORValue) -> Void)?) -> Void)?
    private var appMailbox: AppSurfaceMailbox?
    private var appIncarnation: UInt64?
    /// Install before startup. Replacing the native owner retires its whole old
    /// stream, never lends an old callback the successor's observation token.
    public func installAppSurface(_ adapter: AppSurfaceAdapter, refresh: @escaping () throws -> Void,
        events: @escaping ((@MainActor (String, SoftmaxAppSurface.CBORValue) -> Void)?) -> Void) {
        detachAppSurface()
        appAdapter = adapter; refreshApp = refresh; setAppEvents = events
    }
    private func detachAppSurface() {
        appMailbox?.retire(); setAppEvents?(nil); appMailbox = nil; appIncarnation = nil
    }
    private func refreshAppSurface() throws {
        guard let appMailbox, let appIncarnation, let appAdapter else { return }
        for ticket in appMailbox.queuedCommands() { appMailbox.execute(ticket, adapter: appAdapter) }
        // Commands may synchronously mutate the actual UI. Refresh before the
        // actor reconciles their receipts or services another immutable read.
        try refreshApp?()
        try appMailbox.publish(appAdapter.capture(), incarnation: appIncarnation)
    }
    public init(directory: URL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Softmax/runtime", isDirectory: true)) {
        self.directory = directory
        let stop = RuntimeOutputStop(); outputStop = stop
        let output = RuntimeFaceOutput { stop.stop() }; self.output = output
        executor = RuntimeExecutor(root: directory, output: output)
    }
    public func start() async { await boot(localReset: false) }
    public func restart() async {
        guard !busy else { return }
        busy = true; generation &+= 1; state = .stopped
        detachAppSurface()
        cancelOutput()
        polling?.cancel(); polling = nil
        try? await executor.stop()
        busy = false
        await boot(localReset: true)
    }
    private func boot(localReset: Bool) async {
        guard !busy else { return }; busy = true; defer { busy = false }
        generation &+= 1
        do {
            let mailbox = appAdapter.map { _ in AppSurfaceMailbox() }
            appMailbox = mailbox
            // Native capture failures do not break legacy built-in-only hosts;
            // the control owner refuses app-required startup without a capture.
            let capture: AppSurfaceCapture?
            do { try refreshApp?(); capture = try appAdapter?.capture() } catch { capture = nil }
            coglets = try await executor.start(localReset: localReset, generation: generation, appMailbox: mailbox, appCapture: capture)
            if let source = await executor.appSource(generation: generation), let mailbox {
                appIncarnation = source.incarnation
                // The synchronous native subscription captures this incarnation's
                // mailbox only. Encoding/copy failure permanently fences it.
                setAppEvents?({ [mailbox] operation, payload in
                    do { try mailbox.observe(operation: operation, payload: payload) }
                    catch { mailbox.retire() }
                })
                try refreshAppSurface()
            } else { appMailbox = nil }
            state = .running; errorMessage = nil
            if automaticallyPolls {
                polling = Task { [weak self] in
                    while !Task.isCancelled {
                        try? await Task.sleep(for: .milliseconds(100))
                        guard !Task.isCancelled, let self else { return }
                        await self.poll()
                    }
                }
            }
        } catch {
            detachAppSurface()
            try? await executor.stop()
            fail(error)
        }
    }
    public func stop() async {
        detachAppSurface() // even while an asynchronous boot still owns `busy`
        guard !busy else { return }; busy = true; defer { busy = false }
        generation &+= 1
        polling?.cancel(); polling = nil
        cancelOutput()
        do { try await executor.stop(); state = .stopped; coglets = coglets.map { .init(id: $0.id, version: $0.version, status: .stopped) } }
        catch { fail(error) }
    }
    public func submitTranscript(_ text: String, generation expected: UInt64) async {
        guard state == .running, !busy, generation == expected else { return }
        let outputEpoch = output.epoch
        beforeTranscriptHandoff?()
        do {
            try refreshAppSurface()
            let received = try await executor.transcript(text, generation: expected, outputEpoch: outputEpoch)
            if generation == expected { deliver(received) }
        } catch { if generation == expected { detachAppSurface(); try? await executor.stop(); fail(error) } }
    }
    public func poll() async {
        guard state == .running, !busy else { return }
        let expected = generation
        do {
            try refreshAppSurface()
            let received = try await executor.tick(generation: expected)
            if generation == expected { deliver(received) }
        } catch { if generation == expected { detachAppSurface(); try? await executor.stop(); fail(error) } }
    }
    private func deliver(_ received: [RuntimeAction]) {
        for action in received where output.isLive(action) {
            guard DispatchTime.now().uptimeNanoseconds / 1000 < action.deadline else {
                complete(action, error: "deadline_exceeded"); continue
            }
            actions.append(action); if actions.count > 128 { actions.removeFirst() }
            if let onAction { onAction(action) } else { complete(action, error: "unavailable") }
        }
    }
    /// Called only by the native renderer, never on synthesis completion alone.
    public func complete(_ action: RuntimeAction, error: String? = nil) { output.complete(action, error: error) }
    /// Synchronous local off-switch; SDK reconciliation follows on its actor.
    public func cancelOutput() { output.suspend() }
    private func fail(_ error: any Swift.Error) {
        detachAppSurface()
        cancelOutput()
        state = .error; errorMessage = String(describing: error)
        coglets = coglets.map { .init(id: $0.id, version: $0.version, status: .error) }
        polling?.cancel(); polling = nil
    }
    #if COGLET_RUNTIME_TEST_HOOKS
    public func injectCaughtPanicForTesting() async {
        do { try await executor.injectPanic() } catch { fail(error) }
    }
    #endif
}
