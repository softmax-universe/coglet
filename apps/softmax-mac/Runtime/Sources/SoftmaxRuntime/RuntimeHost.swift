import Foundation
import Combine
import Darwin
import CogletSurface
import CogletTypes
import CogletRuntime

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
}
private struct HostFailure: Error, CustomStringConvertible {
    let description: String
    init(_ description: String) { self.description = description }
}

/// Captures dispatch without pretending that text was physically rendered.
/// Until a product speech sink is installed, terminal outcomes say unavailable.
private final class PendingFaceSink: SurfaceNativeBackend {
    var actions: [RuntimeAction] = []
    func preflight(action: CBORValue, now: UInt64) throws {}
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        actions.append(RuntimeAction(capability: action["capability"]!.textValue!,
            text: action["args"]?["text"]?.textValue,
            expression: action["args"]?["expression"]?.textValue))
        return .failed(NativeSurfaceError("unavailable"))
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        throw NativeSurfaceError("unavailable")
    }
    func stop(owner: String?) {}
}

/// Every runtime pointer and SDK object is confined to this background actor.
/// No ABI entry happens on MainActor or a real-time audio callback.
private actor RuntimeExecutor {
    let root: URL
    private var link: EmbeddedSurfaceTransport?
    private var session: SurfaceSession?
    private let sink = PendingFaceSink()
    private var owner: String?
    private var token: NativeObservationToken?
    private var queued: [[UInt8]] = []
    private var lockFD: Int32 = -1
    private var active = false
    private var lastRenewal: UInt64 = 0
    init(root: URL) { self.root = root }
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
    func start(localReset: Bool) throws -> [InstalledCoglet] {
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
            let names = Set((manifest["subscriptions"] as? [String] ?? []) + (manifest["required_capabilities"] as? [String] ?? []))
            let supported = Set(["speech.start", "speech.end", "speech.speak_chunk", "expression.set_expression"])
            guard names.isSubset(of: supported) else { throw HostFailure("Installed coglet requires unavailable surface operations") }
            let operationNames = names.union(["speech.start", "speech.end"])
            let operations = SurfaceOperation.builtins.filter { operationNames.contains($0.name) }
            try check(sendJSON(binding, coglet_runtime_configure), "configure")
            try check(sendJSON(configuration, coglet_runtime_start), "start")
            active = true
            let ledgerDir = root.appendingPathComponent("surface")
            let exists = fm.fileExists(atPath: ledgerDir.path)
            let store = try FileSurfaceLedger(directory: ledgerDir, newIdentity: !exists)
            let resources = Set(manifest["exclusive_outputs"] as? [String] ?? []).union(["audio.mic", "audio.speaker"])
            let clock: () -> UInt64 = { DispatchTime.now().uptimeNanoseconds / 1_000 }
            func makeSession() throws -> SurfaceSession {
                try SurfaceSession(identity: identity, boundCoglets: [id], operations: operations, store: store,
                    backend: sink, initialGrants: [.init(coglet: id, operations: operationNames, resources: resources, expires: now() + 86_400_000_000)], clock: clock)
            }
            var surface = try makeSession()
            if surface.isKilled {
                guard localReset else { try surface.close(); throw HostFailure("Unclean surface restart: use Restart runtime for explicit local reconciliation") }
                try surface.localOperatorResetForNextConnection()
                surface = try makeSession()
            }
            owner = id; session = surface
            link = try EmbeddedSurfaceTransport(abi: .init(open: coglet_surface_open, send: coglet_surface_send, recv: coglet_surface_recv, close: coglet_surface_close))
            try surface.start(); try pump()
            token = try surface.openNativeObservationSession(owner: id, epoch: surface.grantEpoch)
            lastRenewal = now()
            log("runtime started")
            return [.init(id: id, version: manifest["version"] as? String ?? "unknown", status: .running)]
        } catch {
            cleanup(); log("runtime startup failed"); throw error
        }
    }
    func pump() throws {
        guard let surface = session, let link else { return }
        for _ in 0..<128 {
            queued += surface.drain()
            while let first = queued.first {
                guard try link.send(first) else { return }
                queued.removeFirst()
            }
            if let raw = try link.receive() { try surface.receive(raw) }
            else { break }
        }
        try check(coglet_runtime_status(), "runtime/guest")
    }
    func tick() throws -> [RuntimeAction] {
        guard let surface = session else { return [] }
        do {
            try pump()
            // Never auto-renew authority: local grants are finite. Operator Restart
            // refreshes the lease. No policy/permission expansion is inferred here.
            try surface.tick(); try pump()
            let result = sink.actions; sink.actions = []; return result
        } catch { cleanup(); log("runtime fault; restart required"); throw error }
    }
    func transcript(_ text: String) throws -> [RuntimeAction] {
        guard let session, let token else { throw HostFailure("Runtime not connected") }
        do {
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
        sink.stop(owner: nil)
        try? session?.close(); session = nil; token = nil; owner = nil; queued = []
        link?.close(); link = nil
        if active { _ = coglet_runtime_stop() }; active = false
        if lockFD >= 0 { _ = flock(lockFD, LOCK_UN); _ = Darwin.close(lockFD); lockFD = -1 }
    }
    #if COGLET_RUNTIME_TEST_HOOKS
    func injectPanic() throws {
        try check(coglet_runtime_test_panic(), "injected panic")
    }
    #endif
}

@MainActor public final class RuntimeHost: ObservableObject {
    @Published public private(set) var state: RuntimeState = .stopped
    @Published public private(set) var errorMessage: String?
    @Published public private(set) var coglets: [InstalledCoglet] = []
    @Published public private(set) var actions: [RuntimeAction] = []
    public let directory: URL
    public var onAction: ((RuntimeAction) -> Void)?
    private let executor: RuntimeExecutor
    private var polling: Task<Void, Never>?
    private var busy = false
    public init(directory: URL = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Softmax/runtime", isDirectory: true)) {
        self.directory = directory; executor = RuntimeExecutor(root: directory)
    }
    public func start() async { await boot(localReset: false) }
    public func restart() async {
        guard !busy else { return }
        polling?.cancel(); polling = nil
        try? await executor.stop()
        await boot(localReset: true)
    }
    private func boot(localReset: Bool) async {
        guard !busy else { return }; busy = true; defer { busy = false }
        do {
            coglets = try await executor.start(localReset: localReset)
            state = .running; errorMessage = nil
            polling = Task { [weak self] in
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(100))
                    guard !Task.isCancelled, let self else { return }
                    await self.poll()
                }
            }
        } catch { fail(error) }
    }
    public func stop() async {
        guard !busy else { return }; busy = true; defer { busy = false }
        polling?.cancel(); polling = nil
        do { try await executor.stop(); state = .stopped; coglets = coglets.map { .init(id: $0.id, version: $0.version, status: .stopped) } }
        catch { fail(error) }
    }
    public func submitTranscript(_ text: String) async {
        guard state == .running, !busy else { return }
        do { deliver(try await executor.transcript(text)) } catch { fail(error) }
    }
    public func poll() async {
        guard state == .running, !busy else { return }
        do { deliver(try await executor.tick()) } catch { fail(error) }
    }
    private func deliver(_ received: [RuntimeAction]) {
        for action in received { actions.append(action); if actions.count > 128 { actions.removeFirst() }; onAction?(action) }
    }
    private func fail(_ error: Error) {
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
