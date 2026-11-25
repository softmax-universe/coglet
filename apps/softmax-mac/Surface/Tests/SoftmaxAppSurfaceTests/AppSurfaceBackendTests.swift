import Foundation
import XCTest
@testable import SoftmaxAppSurface
import CogletSurface
import CogletTypes
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// Real authenticated AF_UNIX transport; the other end is a deterministic peer,
/// not a coglet-mock or native UI qualification. Kept on the harness actor.
private final class BackendWire {
    let transport: SurfaceSocketTransport
    let peer: Int32
    private var decoder = SurfaceFrameDecoder()
    init() throws {
        var fds: [Int32] = [-1, -1]
#if canImport(Darwin)
        guard socketpair(AF_UNIX, SOCK_STREAM, 0, &fds) == 0 else { throw NativeSurfaceError("unavailable") }
#else
        guard socketpair(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0, &fds) == 0 else { throw NativeSurfaceError("unavailable") }
#endif
        peer = fds[1]
        transport = try SurfaceSocketTransport(authenticatedDescriptor: fds[0], runtimeUID: getuid(), contractProfile: .surface02)
        _ = fcntl(peer, F_SETFL, O_NONBLOCK)
    }
    deinit {
#if canImport(Darwin)
        _ = Darwin.close(peer)
#else
        _ = Glibc.close(peer)
#endif
    }
    func incoming(_ bytes: [UInt8]) throws -> [UInt8] {
        let framed = try SurfaceWire.frame(bytes)
        let sent = framed.withUnsafeBytes { send(peer, $0.baseAddress!, $0.count, 0) }
        guard sent == framed.count, let received = try transport.receive() else { throw NativeSurfaceError("unavailable") }
        return received
    }
    func outgoing(_ frames: [[UInt8]]) throws -> [W] {
        for frame in frames { try transport.enqueue(frame) }
        guard try transport.flush() else { throw NativeSurfaceError("resource_exhausted") }
        var result: [W] = []
        for _ in 0..<128 {
            var buffer = [UInt8](repeating: 0, count: 4096)
            let n = buffer.withUnsafeMutableBytes { recv(peer, $0.baseAddress!, $0.count, 0) }
            if n < 0, errno == EAGAIN || errno == EWOULDBLOCK { break }
            guard n > 0 else { throw NativeSurfaceError("unavailable") }
            try decoder.consume(buffer.prefix(n)) { result.append(try SurfaceWire.decode($0, from: .surface, contractProfile: .surface02)) }
        }
        guard result.count == frames.count else { throw NativeSurfaceError("unavailable") }
        return result
    }
}

private typealias W = CogletTypes.CBORValue
private typealias N = SoftmaxAppSurface.CBORValue
private func obj(_ values: [String: W]) -> W { .map(values.map { .init($0.key, $0.value) }) }
private let owner = "coglet:" + String(repeating: "b", count: 64)
private let cid = String(repeating: "1", count: 32)

private final class BackendClock: @unchecked Sendable {
    private let lock = NSLock()
    private var value: UInt64 = 10
    func now() -> UInt64 { lock.lock(); defer { lock.unlock() }; return value }
    func advance(_ to: UInt64) { lock.lock(); value = to; lock.unlock() }
}
private final class BackendStore: SurfaceLedgerStore {
    var bytes = try! CogletTypes.CBORCodec.encodeProfile(.map([]))
    func load() throws -> [UInt8] { bytes }
    func save(_ value: [UInt8]) throws { bytes = value }
}
/// Only this off-main actor touches the real SDK session/backend/store. The
/// native Host fixture is separate MainActor code with shared user callbacks.
private actor BackendHarness {
    let mailbox: AppSurfaceMailbox
    let backend: AppSurfaceBackend
    let session: SurfaceSession
    let clock: BackendClock
    let incarnation: UInt64
    private let wire: BackendWire
    init(capture: AppSurfaceCapture, clock: BackendClock, allowed: Set<String>? = nil) throws {
        wire = try BackendWire()
        self.clock = clock
        mailbox = AppSurfaceMailbox(clock: { clock.now() }); backend = AppSurfaceBackend(mailbox: mailbox)
        let root = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent()
        let raw = Array(try Data(contentsOf: root.appendingPathComponent("catalog.v1.json")))
        let pin = try String(contentsOf: root.appendingPathComponent("catalog.v1.sha256"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)
        let catalog = try TrustedExtensionCatalog(json: raw, sha256: pin, contractProfile: .surface02)
        let operations = allowed.map { a in catalog.operations.filter { a.contains($0.name) } } ?? catalog.operations.filter { $0.entry != "softmax.app_sensitive" }
        let grant = EffectiveSurfaceGrant(coglet: owner, operations: Set(operations.map(\.name)), resources: Set(operations.flatMap(\.resources)), expires: 100_000)
        session = try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64), boundCoglets: [owner],
            operations: operations, store: BackendStore(), backend: backend, contractProfile: .surface02,
            trustedCatalog: catalog, initialGrants: [grant], clock: { clock.now() }, correlation: { cid })
        incarnation = try session.reserveSourceIncarnation()
        try session.start(); _ = try wire.outgoing(session.drain())
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("hello_reply"), "body": obj([
            "selection": obj(["version": .text("0.2")]), "correlation_id": .text(cid)])]), from: .runtime)))
        _ = try wire.outgoing(session.drain())
        try mailbox.activate(incarnation: incarnation, epoch: session.grantEpoch, grant: grant, capture: capture)
    }
    func act(_ capability: String, args: W, id: String = cid, deadline: UInt64 = 1000) throws -> [W] {
        XCTAssertFalse(Thread.isMainThread)
        let action = obj(["id": .text(id), "coglet": .text(owner), "capability": .text(capability), "args": args,
                          "deadline": .unsigned(deadline), "correlation_id": .text(cid)])
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("act"), "body": obj([
            "action": action, "grant_epoch": .unsigned(session.grantEpoch)])]), from: .runtime, contractProfile: .surface02)))
        return try frames()
    }
    func read(_ operation: String, args: W = .map([])) throws -> W {
        XCTAssertFalse(Thread.isMainThread)
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("read"), "body": obj([
            "coglet": .text(owner), "capability": .text(operation), "args": args, "deadline": .unsigned(1000),
            "grant_epoch": .unsigned(session.grantEpoch), "correlation_id": .text(cid)])]), from: .runtime, contractProfile: .surface02)))
        return try XCTUnwrap(frames().first)
    }
    func publish(_ capture: AppSurfaceCapture) throws { try mailbox.publish(capture, incarnation: incarnation) }
    func reconcile() throws -> [W] { try backend.reconcile(session); return try frames() }
    func stop() { mailbox.stop() }
    func close() throws { try session.close() }
    func tick() throws -> [W] { try session.tick(); return try frames() }
    func frames() throws -> [W] { try wire.outgoing(session.drain()) }
}

final class AppSurfaceBackendTests: XCTestCase, @unchecked Sendable {
    func testMailboxClockRollbackCannotExtendQueuedAuthority() async throws {
        let clock = BackendClock()
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: clock)
        _ = try await harness.act("softmax.app.invoke", args: invoke("settings.general.button"))
        let mailbox = harness.mailbox
        let ticket = try XCTUnwrap(mailbox.queuedCommands().first)
        clock.advance(9)
        await MainActor.run { mailbox.execute(ticket, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 0) }
        clock.advance(10)
        let frames = try await harness.reconcile()
        XCTAssertEqual(frames.first?["body"]?["payload"]?["status"], .text("failed"))
        let refused = try await harness.read("softmax.app.state")
        XCTAssertEqual(refused["body"]?["ok"], .boolean(false))
        XCTAssertThrowsError(try mailbox.activate(incarnation: 2, epoch: 1,
            grant: .init(coglet: owner, operations: [], resources: [], expires: 1000), capture: capture))
        try await harness.close()
    }

    func testSaturationRejectsBeforeAdmissionAndRecoversAfterDrain() async throws {
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        for i in 1...16 {
            let frames = try await harness.act("softmax.app.invoke", args: invoke("settings.general.button"), id: String(format: "%032x", i))
            XCTAssertEqual(frames.first?["body"]?["accepted"], .boolean(true))
        }
        let full = try await harness.act("softmax.app.invoke", args: invoke("settings.general.button"), id: String(repeating: "e", count: 32))
        XCTAssertEqual(full.first?["body"]?["accepted"], .boolean(false))
        XCTAssertEqual(full.first?["body"]?["error"]?["code"], .text("resource_exhausted"))
        let mailbox = harness.mailbox
        XCTAssertEqual(mailbox.queuedCommands().count, 16)
        let first = try XCTUnwrap(mailbox.queuedCommands().first)
        await MainActor.run { mailbox.execute(first, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 1) }
        _ = try await harness.reconcile()
        let afterDrain = try await harness.act("softmax.app.invoke", args: invoke("settings.general.button"), id: String(repeating: "f", count: 32))
        XCTAssertEqual(afterDrain.first?["body"]?["accepted"], .boolean(true))
        try await harness.close()
    }
    func testReentrantStopPreservesEnteredResultAndCancelsQueuedWithoutDeadlock() async throws {
        let mailbox = AppSurfaceMailbox(clock: { 10 })
        let (host, capture) = try await MainActor.run {
            let h = try Host()
            _ = try h.registry.mount(id: "stop.inside", window: "settings", route: "general", role: "button", label: "Stop",
                verbs: ["press"], value: { Host.obj(["kind": .text("none")]) }, invoke: { _ in mailbox.stop(); h.pressCalls += 1 })
            return (h, try h.adapter.capture())
        }
        // Direct backend fixture isolates reentrant native-stop entry; normal
        // SurfaceSession constructor stop would precede this explicit activation.
        let backend = AppSurfaceBackend(mailbox: mailbox)
        let grant = EffectiveSurfaceGrant(coglet: owner, operations: ["softmax.app.invoke"], resources: ["softmax.app_control"], expires: 1000)
        try mailbox.activate(incarnation: 0, epoch: 1, grant: grant, capture: capture)
        let action = obj(["coglet": .text(owner), "id": .text(cid), "capability": .text("softmax.app.invoke"),
                          "deadline": .unsigned(100), "args": invoke("stop.inside")])
        try backend.preflight(action: action, now: 10)
        XCTAssertNil(try backend.begin(action: action, epoch: 1, now: 10))
        let ticket = try XCTUnwrap(mailbox.queuedCommands().first)
        await MainActor.run { mailbox.execute(ticket, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 1) }
        let receipts = mailbox.drainReceipts()
        XCTAssertEqual(receipts.count, 1)
        if case .succeeded? = receipts.first?.result {} else { XCTFail("entered effect result was replaced by cancellation") }
        XCTAssertThrowsError(try backend.preflight(action: action, now: 10))
    }

    private func invoke(_ control: String) -> W { obj(["window": .text("settings"), "control": .text(control), "verb": .text("press")]) }
    private func native(_ body: W) throws -> N { try SoftmaxAppSurface.CBORCodec.decodeProfile(CogletTypes.CBORCodec.encodeProfile(body)) }

    func testRealSDKDurablePendingDispatchUsesMainOwnerAndReadsCopiedState() async throws {
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        let accepted = try await harness.act("softmax.app.set_setting", args: obj(["key": .text("floating_orb"), "value": .boolean(false)]))
        XCTAssertEqual(accepted.count, 1) // no fabricated success at queue admission
        XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
        let mailbox = harness.mailbox
        let ticket = try XCTUnwrap(mailbox.queuedCommands().first)
        await MainActor.run {
            XCTAssertTrue(host.orb)
            mailbox.execute(ticket, adapter: host.adapter)
            XCTAssertFalse(host.orb)
            mailbox.execute(ticket, adapter: host.adapter) // no second native entry
        }
        let outcome = try await harness.reconcile()
        XCTAssertEqual(outcome.first?["body"]?["payload"]?["status"], .text("succeeded"))
        let refreshed = try await MainActor.run { try host.adapter.capture() }
        try await harness.publish(refreshed)
        let read = try await harness.read("softmax.app.state")
        XCTAssertEqual(read["body"]?["ok"], .boolean(true))
        XCTAssertEqual(read["body"]?["value"]?["settings"]?["floating_orb"], .boolean(false))
        try await harness.close()
    }
    func testPreflightCapturedBeforeDurableAdmissionCannotBorrowSuccessorMount() async throws {
        let (host, capture, oldMount) = try await MainActor.run {
            let h = try Host()
            let mount = try h.registry.mount(id: "queued.control", window: "settings", route: "general", role: "button", label: "Original",
                verbs: ["press"], value: { Host.obj(["kind": .text("none")]) }, invoke: { _ in h.pressCalls += 1 })
            return (h, try h.adapter.capture(), mount)
        }
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        _ = try await harness.act("softmax.app.invoke", args: invoke("queued.control"))
        let mailbox = harness.mailbox
        let ticket = try XCTUnwrap(mailbox.queuedCommands().first)
        try await MainActor.run {
            try host.registry.unmount(oldMount)
            _ = try host.registry.mount(id: "queued.control", window: "settings", route: "general", role: "button", label: "Successor",
                verbs: ["press"], value: { Host.obj(["kind": .text("none")]) }, invoke: { _ in host.pressCalls += 100 })
            mailbox.execute(ticket, adapter: host.adapter)
            XCTAssertEqual(host.pressCalls, 0)
        }
        let frames = try await harness.reconcile()
        XCTAssertEqual(frames.first?["body"]?["payload"]?["status"], .text("failed"))
        XCTAssertEqual(frames.first?["body"]?["payload"]?["error"]?["code"], .text("conflict"))
        try await harness.close()
    }
    func testDeadlineAndIndependentStopFenceQueuedNativeEffects() async throws {
        for stop in [false, true] {
            let clock = BackendClock()
            let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
            let harness = try BackendHarness(capture: capture, clock: clock)
            _ = try await harness.act("softmax.app.invoke", args: invoke("settings.general.button"), deadline: 50)
            let mailbox = harness.mailbox
            let ticket = try XCTUnwrap(mailbox.queuedCommands().first)
            if stop { mailbox.stop() } else { clock.advance(50) }
            await MainActor.run { mailbox.execute(ticket, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 0) }
            let frames = try await harness.reconcile()
            XCTAssertEqual(frames.first?["body"]?["payload"]?["status"], .text(stop ? "failed" : "expired"))
            try await harness.close()
        }
    }
    func testSensitiveOperationRefusedByActualSDKWithoutSeparateCapability() async throws {
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: BackendClock(), allowed: ["softmax.app.invoke"])
        let frames = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"))
        XCTAssertEqual(frames.first?["body"]?["accepted"], .boolean(false))
        let mailbox = harness.mailbox
        XCTAssertTrue(mailbox.queuedCommands().isEmpty)
        await MainActor.run { XCTAssertEqual(host.loginCalls, 0) }
        // Even ordinary invoke can't reach that mounted sensitive callback.
        let refused = try await harness.act("softmax.app.invoke", args: invoke("settings.general.login_enable"), id: String(repeating: "2", count: 32))
        XCTAssertEqual(refused.first?["body"]?["accepted"], .boolean(false))
        try await harness.close()
    }
    func testSnapshotPaginationRejectsReplacedCaptureAndStopIsTerminal() async throws {
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        let args = obj(["window": .text("settings"), "after": .null, "revision": .null, "limit": .unsigned(1)])
        let first = try await harness.read("softmax.app.controls", args: args)
        let page = try XCTUnwrap(first["body"]?["value"])
        let next = try XCTUnwrap(page["next"])
        let identical = try await MainActor.run { try host.adapter.capture() }
        try await harness.publish(identical)
        let same = try await harness.read("softmax.app.controls", args: obj(["window": .text("settings"), "after": next,
            "revision": try XCTUnwrap(page["revision"]), "limit": .unsigned(1)]))
        XCTAssertEqual(same["body"]?["ok"], .boolean(true))
        let foreign = try await MainActor.run { try Host().adapter.capture() }
        do { try await harness.publish(foreign); XCTFail("foreign native host capture accepted") } catch {}
        let refreshed = try await MainActor.run { try host.registry.nativeStateChanged(); return try host.adapter.capture() }
        try await harness.publish(refreshed)
        let stale = try await harness.read("softmax.app.controls", args: obj(["window": .text("settings"), "after": next,
            "revision": try XCTUnwrap(page["revision"]), "limit": .unsigned(1)]))
        XCTAssertEqual(stale["body"]?["ok"], .boolean(false))
        let mailbox = harness.mailbox
        mailbox.stop()
        let refused = try await harness.read("softmax.app.state")
        XCTAssertEqual(refused["body"]?["ok"], .boolean(false))
        XCTAssertThrowsError(try mailbox.activate(incarnation: 2, epoch: 1,
            grant: .init(coglet: owner, operations: [], resources: [], expires: 1000), capture: refreshed))
        try await harness.close()
    }
}
