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
    private let connection: AppSurfaceConnection?
    init(capture: AppSurfaceCapture, clock: BackendClock, allowed: Set<String>? = nil, withConnection: Bool = false) throws {
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
        let reserved = withConnection ? nil : try session.reserveSourceIncarnation()
        try session.start(); _ = try wire.outgoing(session.drain())
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("hello_reply"), "body": obj([
            "selection": obj(["version": .text("0.2")]), "correlation_id": .text(cid)])]), from: .runtime)))
        _ = try wire.outgoing(session.drain())
        if withConnection {
            let attached = try AppSurfaceConnection(session: session, mailbox: mailbox, grant: grant, operations: operations, capture: capture)
            connection = attached; incarnation = attached.incarnation
        } else {
            connection = nil; incarnation = reserved!
            try mailbox.activate(incarnation: incarnation, epoch: session.grantEpoch, grant: grant, capture: capture)
        }
    }
    func revokeGrants() throws -> (before: UInt64, after: UInt64, frames: [W]) {
        let before = session.grantEpoch
        try session.installGrants([])
        return (before, session.grantEpoch, try frames())
    }
    func act(_ capability: String, args: W, id: String = cid, deadline: UInt64 = 1000, epoch: UInt64? = nil) throws -> [W] {
        XCTAssertFalse(Thread.isMainThread)
        let action = obj(["id": .text(id), "coglet": .text(owner), "capability": .text(capability), "args": args,
                          "deadline": .unsigned(deadline), "correlation_id": .text(cid)])
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("act"), "body": obj([
            "action": action, "grant_epoch": .unsigned(epoch ?? session.grantEpoch)])]), from: .runtime, contractProfile: .surface02)))
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
    func eventFrames() throws -> [W] {
        try XCTUnwrap(connection).publishEvents(session)
        return try frames()
    }
    func replaceExtension() throws {
        _ = try session.openExtensionObservationSession(owner: owner, operation: "softmax.app.view_changed", epoch: session.grantEpoch)
    }
    func replaceSpeech() throws { _ = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch) }
    func guardedSend(_ stoppedDuringAttempt: Bool = false) throws -> Bool {
        try XCTUnwrap(connection).emit(session: session) {
            if stoppedDuringAttempt { mailbox.retire() }
            return true
        }
    }
    func queuedSuccessfulRead() throws -> [[UInt8]] {
        try session.receive(wire.incoming(SurfaceWire.encode(obj(["type": .text("read"), "body": obj([
            "coglet": .text(owner), "capability": .text("softmax.app.state"), "args": .map([]), "deadline": .unsigned(1000),
            "grant_epoch": .unsigned(session.grantEpoch), "correlation_id": .text(cid)])]), from: .runtime, contractProfile: .surface02)))
        return session.drain()
    }
}

final class AppSurfaceBackendTests: XCTestCase, @unchecked Sendable {
    @MainActor private final class Suspension {
        let entered = XCTestExpectation(description: "native async command started")
        var waits: [CheckedContinuation<Void, Never>] = []
        func wait() async {
            await withCheckedContinuation { continuation in waits.append(continuation); entered.fulfill() }
        }
        func resume() { let all = waits; waits = []; for continuation in all { continuation.resume() } }
    }
    private func asyncHost(_ suspension: Suspension) async throws -> (Host, AppSurfaceCapture, ControlRegistry.Mount) {
        try await MainActor.run {
            let host = try Host()
            let mount = try host.registry.mount(id: "async.control", window: "settings", route: "general", role: "button", label: "Deferred",
                verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
                invokeAsync: { _, context in
                    await suspension.wait() // Deliberately ignores Task cancellation.
                    try context.perform { host.pressCalls += 1 }
                }, invoke: { _ in XCTFail("async control used sync fallback") })
            return (host, try host.adapter.capture(), mount)
        }
    }
    private func awaitResult(_ harness: BackendHarness) async throws -> [W] {
        for task in harness.mailbox.runningCommandTasks() { await task.value }
        return try await harness.reconcile()
    }
    func testAsyncCommandAcceptsBeforeCompletionAndCompletesExactlyOnce() async throws {
        let suspension = Suspension()
        let (host, capture, _) = try await asyncHost(suspension)
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        let accepted = try await harness.act("softmax.app.invoke", args: invoke("async.control"))
        XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
        let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
        await MainActor.run {
            harness.mailbox.execute(ticket, adapter: host.adapter)
            harness.mailbox.execute(ticket, adapter: host.adapter)
        }
        await fulfillment(of: [suspension.entered], timeout: 2)
        let before = try await harness.reconcile()
        XCTAssertTrue(before.isEmpty)
        await MainActor.run { XCTAssertEqual(host.pressCalls, 0); suspension.resume() }
        let done = try await awaitResult(harness)
        XCTAssertEqual(done.first?["body"]?["payload"]?["status"], .text("succeeded"))
        await MainActor.run { XCTAssertEqual(host.pressCalls, 1); harness.mailbox.execute(ticket, adapter: host.adapter) }
        let duplicate = try await harness.reconcile()
        XCTAssertTrue(duplicate.isEmpty)
        try await harness.close()
    }
    func testAsyncDeferredEffectFencedByStopDeadlineAndMountReplacement() async throws {
        for reason in ["stop", "deadline", "mount", "route"] {
            let suspension = Suspension()
            let (host, capture, mount) = try await asyncHost(suspension)
            let clock = BackendClock()
            let harness = try BackendHarness(capture: capture, clock: clock)
            _ = try await harness.act("softmax.app.invoke", args: invoke("async.control"), deadline: 50)
            let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
            await MainActor.run { harness.mailbox.execute(ticket, adapter: host.adapter) }
            await fulfillment(of: [suspension.entered], timeout: 2)
            if reason == "stop" { harness.mailbox.stop() }
            if reason == "deadline" { clock.advance(50) }
            try await MainActor.run {
                if reason == "mount" {
                    try host.registry.unmount(mount)
                    _ = try host.registry.mount(id: "async.control", window: "settings", route: "general", role: "button", label: "Replacement",
                        verbs: ["press"], value: { .map([.init("kind", .text("none"))]) }, invoke: { _ in host.pressCalls += 100 })
                }
                if reason == "route" { host.windows["settings"] = "audio" }
                suspension.resume()
            }
            let done = try await awaitResult(harness)
            XCTAssertNotEqual(done.first?["body"]?["payload"]?["status"], .text("succeeded"))
            await MainActor.run { XCTAssertEqual(host.pressCalls, 0) }
            try await harness.close()
        }
    }
    func testCancelledSuspendedCommandsRetainCapacityUntilActuallyReturned() async throws {
        let suspension = Suspension()
        await MainActor.run { suspension.entered.expectedFulfillmentCount = 16 }
        let (host, capture, _) = try await asyncHost(suspension)
        let harness = try BackendHarness(capture: capture, clock: BackendClock())
        for index in 1...16 {
            let id = String(format: "%032x", index)
            _ = try await harness.act("softmax.app.invoke", args: invoke("async.control"), id: id)
        }
        let tickets = harness.mailbox.queuedCommands()
        await MainActor.run { for ticket in tickets { harness.mailbox.execute(ticket, adapter: host.adapter) } }
        await fulfillment(of: [suspension.entered], timeout: 2)
        harness.mailbox.cancelPending(owner: owner)
        let cancellations = try await harness.reconcile()
        XCTAssertEqual(cancellations.count, 16)
        let refused = try await harness.act("softmax.app.invoke", args: invoke("async.control"), id: String(repeating: "e", count: 32))
        XCTAssertEqual(refused.first?["body"]?["accepted"], .boolean(false))
        await MainActor.run { suspension.resume() }
        for task in harness.mailbox.runningCommandTasks() { await task.value }
        let recovered = try await harness.act("softmax.app.invoke", args: invoke("async.control"), id: String(repeating: "d", count: 32))
        XCTAssertEqual(recovered.first?["body"]?["accepted"], .boolean(true))
        await MainActor.run { XCTAssertEqual(host.pressCalls, 0) }
        try await harness.close()
    }
    func testOwnerLocalDeadlineCancelsTicketsWithoutRetiringReadOrEventStream() async throws {
        let clock = BackendClock()
        let (host, capture) = try await MainActor.run { let host = try Host(); return (host, try host.adapter.capture()) }
        let h = try BackendHarness(capture: capture, clock: clock, withConnection: true)
        _ = try await h.act("softmax.app.invoke", args: invoke("settings.general.button"), deadline: 50)
        let old = try XCTUnwrap(h.mailbox.queuedCommands().first)
        clock.advance(50)
        let expiry = try await h.tick()
        XCTAssertEqual(expiry.first?["body"]?["payload"]?["status"], .text("expired"))
        await MainActor.run { h.mailbox.execute(old, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 0) }
        let stillLive = try await h.guardedSend()
        XCTAssertTrue(stillLive)
        _ = try await h.reconcile()
        let next = try await h.act("softmax.app.invoke", args: invoke("settings.general.button"), id: String(repeating: "f", count: 32))
        XCTAssertEqual(next.first?["body"]?["accepted"], .boolean(true))
        let ticket = try XCTUnwrap(h.mailbox.queuedCommands().first)
        await MainActor.run { h.mailbox.execute(ticket, adapter: host.adapter); XCTAssertEqual(host.pressCalls, 1) }
        let completed = try await h.reconcile()
        XCTAssertEqual(completed.first?["body"]?["payload"]?["status"], .text("succeeded"))
        try await h.close()
    }
    func testRealProductEventDoesNotInventASelectedReadRequirement() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let h = try BackendHarness(capture: capture, clock: BackendClock(), allowed: ["softmax.app.view_changed"], withConnection: true)
        try h.mailbox.observe(operation: "softmax.app.view_changed", payload: .map([
            .init("revision", .unsigned(capture.revision)), .init("window", .text("settings")), .init("route", .text("general"))]))
        let events = try await h.eventFrames()
        XCTAssertEqual(events.count, 1) // actual product catalog has NO selection.reads
        try await h.close()
    }
    func testConnectionPublishesNativeEventsWithReservedSourceAndClosesOldCustody() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let h = try BackendHarness(capture: capture, clock: BackendClock(), withConnection: true)
        try h.mailbox.observe(operation: "softmax.app.view_changed", payload: .map([
            .init("revision", .unsigned(capture.revision)), .init("window", .text("settings")), .init("route", .text("general"))]))
        let events = try await h.eventFrames()
        XCTAssertEqual(events.count, 1)
        XCTAssertEqual(events.first?["body"]?["kind"], .text("softmax.app.view_changed"))
        guard case .unsigned(let seq)? = events.first?["body"]?["seq"] else { return XCTFail("missing event sequence") }
        XCTAssertGreaterThan(seq, h.incarnation)
        let sent = try await h.guardedSend()
        XCTAssertTrue(sent)
        try await h.replaceExtension() // SAME epoch, distinct SDK stream custody
        do { _ = try await h.guardedSend(); XCTFail("old stream emitted") } catch { }
        try await h.close()
    }
    func testAlreadyDrainedPrivateReadCannotBeSentAfterSpeechReplacement() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let h = try BackendHarness(capture: capture, clock: BackendClock(), withConnection: true)
        let drained = try await h.queuedSuccessfulRead()
        XCTAssertEqual(drained.count, 1)
        XCTAssertEqual(try SurfaceWire.decode(drained[0], from: .surface, contractProfile: .surface02)["body"]?["ok"], .boolean(true))
        try await h.replaceSpeech()
        do { _ = try await h.guardedSend(); XCTFail("already drained private reply emitted") } catch { }
        try await h.close()
    }
    func testNativeStopDoesNotWaitForEnteredEmissionAndRefusesNextAttempt() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let h = try BackendHarness(capture: capture, clock: BackendClock(), withConnection: true)
        // Stop reenters the gate from INSIDE the ABI-attempt closure. The entered
        // attempt is not claimed undone, but its result and next send are fenced.
        do { _ = try await h.guardedSend(true); XCTFail("stopped stream reused") } catch { }
        do { _ = try await h.guardedSend(); XCTFail("next attempt entered") } catch { }
        try await h.close()
    }
    func testNativeEventQueueOverflowRetiresInsteadOfDroppingObservations() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let h = try BackendHarness(capture: capture, clock: BackendClock(), withConnection: true)
        let event = N.map([.init("revision", .unsigned(capture.revision)), .init("window", .text("settings")), .init("route", .text("general"))])
        for _ in 0..<64 { try h.mailbox.observe(operation: "softmax.app.view_changed", payload: event) }
        XCTAssertThrowsError(try h.mailbox.observe(operation: "softmax.app.view_changed", payload: event))
        do { _ = try await h.eventFrames(); XCTFail("overflow stream remained active") } catch { }
        XCTAssertTrue(h.mailbox.drainEvents().isEmpty)
        try await h.close()
    }
    func testIndependentStartupRetirementIsNotAnSDKPreparationStop() async throws {
        let capture = try await MainActor.run { try Host().adapter.capture() }
        let m = AppSurfaceMailbox(clock: { 10 })
        m.retire()
        XCTAssertThrowsError(try m.activate(incarnation: 0, epoch: 1,
            grant: .init(coglet: owner, operations: [], resources: [], expires: 1000), capture: capture))
    }
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
    // These exercise the real SDK/socketpair with fake product callbacks only:
    // no permissions prompt, login item, identity reset, update or quit is run.
    func testSensitiveGrantedFakeCallbackRunsOnceAndDirectionBypassesRefuse() async throws {
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: BackendClock(),
            allowed: ["softmax.app.invoke", "softmax.app.invoke_sensitive"])
        let accepted = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"))
        XCTAssertEqual(accepted.count, 1)
        XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
        let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
        await MainActor.run {
            harness.mailbox.execute(ticket, adapter: host.adapter)
            harness.mailbox.execute(ticket, adapter: host.adapter)
            XCTAssertEqual(host.loginCalls, 1)
            XCTAssertEqual(host.pressCalls, 0)
        }
        let completed = try await harness.reconcile()
        XCTAssertEqual(completed.count, 1)
        XCTAssertEqual(completed.first?["body"]?["payload"]?["status"], .text("succeeded"))
        let retry = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"))
        XCTAssertEqual(retry.count, 2)
        XCTAssertEqual(retry.first?["body"]?["accepted"], .boolean(true))
        XCTAssertEqual(retry.last?["body"]?["payload"]?["status"], .text("succeeded"))
        for (operation, control, id) in [
            ("softmax.app.invoke", "settings.general.login_enable", "2"),
            ("softmax.app.invoke_sensitive", "settings.general.button", "3")
        ] {
            let refused = try await harness.act(operation, args: invoke(control), id: String(repeating: id, count: 32))
            XCTAssertEqual(refused.count, 1)
            XCTAssertEqual(refused.first?["body"]?["accepted"], .boolean(false))
            XCTAssertEqual(refused.first?["body"]?["error"]?["code"], .text("permission_denied"))
            XCTAssertTrue(harness.mailbox.queuedCommands().isEmpty)
        }
        await MainActor.run { XCTAssertEqual(host.loginCalls, 1); XCTAssertEqual(host.pressCalls, 0) }
        let duplicate = try await harness.reconcile()
        XCTAssertTrue(duplicate.isEmpty)
        try await harness.close()
    }

    func testSensitiveQueuedGrantReplacementAndExpiryFenceNativeEntry() async throws {
        for expires in [false, true] {
            let clock = BackendClock()
            let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
            let harness = try BackendHarness(capture: capture, clock: clock,
                allowed: ["softmax.app.invoke_sensitive"])
            let accepted = try await harness.act("softmax.app.invoke_sensitive",
                args: invoke("settings.general.login_enable"), deadline: 200_000)
            XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
            let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
            if expires {
                clock.advance(100_000) // exact exclusive grant expiry, before action deadline
                let frames = try await harness.tick()
                XCTAssertEqual(frames.first?["type"], .text("grants"))
            } else {
                let replacement = try await harness.revokeGrants()
                XCTAssertGreaterThan(replacement.after, replacement.before)
                XCTAssertEqual(replacement.frames.first?["type"], .text("grants"))
            }
            await MainActor.run {
                harness.mailbox.execute(ticket, adapter: host.adapter)
                harness.mailbox.execute(ticket, adapter: host.adapter)
                XCTAssertEqual(host.loginCalls, 0)
                XCTAssertEqual(host.pressCalls, 0)
            }
            // installGrants retires the mailbox and cancels old-epoch ledger
            // records without delivering their outcomes into the new epoch.
            let staleCompletion = try await harness.reconcile()
            XCTAssertTrue(staleCompletion.isEmpty)
            let stale = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"),
                id: String(repeating: "2", count: 32), deadline: 200_000, epoch: ticket.epoch)
            XCTAssertEqual(stale.first?["body"]?["accepted"], .boolean(false))
            let fresh = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"),
                id: String(repeating: "3", count: 32), deadline: 200_000)
            XCTAssertEqual(fresh.first?["body"]?["accepted"], .boolean(false))
            XCTAssertTrue(harness.mailbox.queuedCommands().isEmpty)
            await MainActor.run { XCTAssertEqual(host.loginCalls, 0) }
            try await harness.close()
        }
    }

    func testSensitiveQueuedEffectStillAllowedImmediatelyBeforeGrantExpiry() async throws {
        let clock = BackendClock()
        let (host, capture) = try await MainActor.run { let h = try Host(); return (h, try h.adapter.capture()) }
        let harness = try BackendHarness(capture: capture, clock: clock, allowed: ["softmax.app.invoke_sensitive"])
        let accepted = try await harness.act("softmax.app.invoke_sensitive", args: invoke("settings.general.login_enable"), deadline: 200_000)
        XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
        let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
        clock.advance(99_999)
        let beforeExpiry = try await harness.tick()
        XCTAssertTrue(beforeExpiry.isEmpty)
        await MainActor.run { harness.mailbox.execute(ticket, adapter: host.adapter); XCTAssertEqual(host.loginCalls, 1) }
        let completion = try await harness.reconcile()
        XCTAssertEqual(completion.count, 1)
        XCTAssertEqual(completion.first?["body"]?["payload"]?["status"], .text("succeeded"))
        try await harness.close()
    }

    func testSensitiveSuspendedGrantRevocationFencesNoncooperativeEffect() async throws {
        for revoked in [false, true] {
        let suspension = Suspension()
        let (host, capture) = try await MainActor.run { () -> (Host, AppSurfaceCapture) in
            let host = try Host()
            _ = try host.registry.mount(id: "sensitive.deferred", window: "settings", route: "general",
                role: "button", label: "Fake sensitive deferred", sensitive: true, verbs: ["press"],
                value: { .map([.init("kind", .text("none"))]) },
                invokeAsync: { _, context in
                    await suspension.wait() // intentionally ignores Task cancellation
                    try context.perform { host.loginCalls += 1 }
                }, invoke: { _ in XCTFail("sensitive async command used sync fallback") })
            return (host, try host.adapter.capture())
        }
        let harness = try BackendHarness(capture: capture, clock: BackendClock(), allowed: ["softmax.app.invoke_sensitive"])
        let accepted = try await harness.act("softmax.app.invoke_sensitive", args: invoke("sensitive.deferred"))
        XCTAssertEqual(accepted.first?["body"]?["accepted"], .boolean(true))
        let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
        await MainActor.run { harness.mailbox.execute(ticket, adapter: host.adapter) }
        await fulfillment(of: [suspension.entered], timeout: 2)
        if revoked {
            let replacement = try await harness.revokeGrants()
            XCTAssertGreaterThan(replacement.after, replacement.before)
        }
        await MainActor.run { suspension.resume() }
        let completion = try await awaitResult(harness)
        if revoked {
            XCTAssertTrue(completion.isEmpty)
        } else {
            XCTAssertEqual(completion.count, 1)
            XCTAssertEqual(completion.first?["body"]?["payload"]?["status"], .text("succeeded"))
        }
        await MainActor.run { XCTAssertEqual(host.loginCalls, revoked ? 0 : 1); XCTAssertEqual(host.pressCalls, 0) }
        XCTAssertTrue(harness.mailbox.queuedCommands().isEmpty)
        try await harness.close()
        }
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

    func testAsyncFocusTicketRunsOnceHonoursDeadlineAndCompletesOnce() async throws {
        for reason in ["success", "deadline", "stop"] {
            let clock = BackendClock()
            let (host, capture, counter, suspension) = try await MainActor.run { () -> (Host, AppSurfaceCapture, FocusCounter, Suspension) in
                let host = try Host()
                let counter = FocusCounter()
                let suspension = Suspension()
                let instant = ContinuousClock.now
                host.registry.focusSettling = .init(timeout: .seconds(2), interval: .milliseconds(10), now: { instant },
                    sleep: { _ in
                        counter.sleeps += 1
                        // Deliberately ignores cancellation; only explicit resume
                        // completes this sleep, so stop cannot win before entry.
                        await suspension.wait()
                    })
                _ = try host.registry.mount(id: "settings.search", window: "settings", route: "general", role: "text_field", label: "Search",
                    verbs: ["type_text"], value: { .map([.init("kind", .text("none"))]) },
                    focus: { counter.calls += 1; return true }, focused: { counter.focused }, focusSettles: true, invoke: { _ in })
                return (host, try host.adapter.capture(), counter, suspension)
            }
            let harness = try BackendHarness(capture: capture, clock: clock)
            let accepted = try await harness.act("softmax.app.set_focus", args: obj(["window": .text("settings"), "control": .text("settings.search")]), deadline: 50)
            XCTAssertEqual(accepted.count, 1)
            XCTAssertEqual(try XCTUnwrap(accepted.first)["body"]?["accepted"], .boolean(true))
            let ticket = try XCTUnwrap(harness.mailbox.queuedCommands().first)
            await MainActor.run { harness.mailbox.execute(ticket, adapter: host.adapter); harness.mailbox.execute(ticket, adapter: host.adapter) }
            await fulfillment(of: [suspension.entered], timeout: 2)
            let tasks = harness.mailbox.runningCommandTasks()
            XCTAssertEqual(tasks.count, 1)
            await MainActor.run {
                XCTAssertEqual(counter.calls, 1)
                XCTAssertEqual(counter.sleeps, 1)
                XCTAssertEqual(suspension.waits.count, 1)
            }
            let pending = try await harness.reconcile()
            XCTAssertTrue(pending.isEmpty, "no terminal outcome before explicit resume")
            if reason == "stop" { harness.mailbox.stop() }
            if reason == "deadline" { clock.advance(50) }
            await MainActor.run {
                // A true witness after ticket expiry must still be refused;
                // the local monotonic settling window has NOT expired.
                counter.focused = true
                suspension.resume()
            }
            for task in tasks { await task.value }
            let done = try await harness.reconcile()
            XCTAssertEqual(done.count, 1, reason)
            let frame = try XCTUnwrap(done.first)
            XCTAssertEqual(frame["body"]?["kind"], .text("action.outcome"))
            let payload = try XCTUnwrap(frame["body"]?["payload"])
            let status = try XCTUnwrap(payload["status"])
            switch reason {
            case "success":
                XCTAssertEqual(status, .text("succeeded"))
                XCTAssertEqual(payload["result"], .map([]))
                XCTAssertNil(payload["error"])
            case "deadline":
                XCTAssertEqual(status, .text("expired"))
                XCTAssertEqual(payload["error"]?["code"], .text("deadline_exceeded"))
            default:
                XCTAssertEqual(status, .text("failed"))
                XCTAssertEqual(payload["error"]?["code"], .text("cancelled"))
            }
            await MainActor.run {
                XCTAssertEqual(counter.calls, 1, reason)
                XCTAssertEqual(counter.sleeps, 1, reason)
                harness.mailbox.execute(ticket, adapter: host.adapter)
            }
            let duplicate = try await harness.reconcile()
            XCTAssertTrue(duplicate.isEmpty, "terminal outcome must be delivered exactly once")
            try await harness.close()
        }
    }
}
