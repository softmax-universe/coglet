import XCTest
import Foundation
import CogletTypes
@testable import CogletSurface

private let identity = "surface:" + String(repeating: "a", count: 64)
private let owner = "coglet:" + String(repeating: "b", count: 64)
private let requestID = String(repeating: "1", count: 32)
private let expression = CBORValue.record([("expression", .text("listening")), ("intensity", .unsigned(500)), ("duration_ms", .unsigned(1000))])
private final class MemoryLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var failing = false
    var writes = 0
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws {
        if failing { throw NativeSurfaceError("unavailable") }
        writes += 1; self.bytes = bytes
    }
}
private final class Clock { var now: UInt64 = 0 }
private final class Backend: SurfaceNativeBackend {
    var begins = 0, stops = 0
    var result: NativeSurfaceResult? = .succeeded(.map([]))
    var readValue: CBORValue?
    var onBegin: (() throws -> Void)?
    func preflight(action: CBORValue, now: UInt64) throws {}
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        try onBegin?(); begins += 1; return result
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        readValue ?? .record([("expression", .text("neutral")), ("intensity", .unsigned(0)), ("remaining_ms", .unsigned(0))])
    }
    func stop(owner: String?) { stops += 1 }
}
private func grant(expires: UInt64 = 10_000_000) -> EffectiveSurfaceGrant {
    .init(coglet: owner, operations: ["expression.set_expression", "expression.state"], resources: ["display.face"], expires: expires)
}
private func make(_ store: MemoryLedger, _ backend: Backend, _ clock: Clock) throws -> SurfaceSession {
    try SurfaceSession(identity: identity, boundCoglets: [owner],
        operations: SurfaceOperation.builtins.filter { $0.entry == "expression.control" },
        store: store, backend: backend, initialGrants: [grant()], clock: { clock.now }, correlation: { requestID })
}
private func negotiate(_ session: SurfaceSession) throws {
    try session.start()
    XCTAssertEqual(session.drain().count, 1)
    try session.receive(SurfaceWire.encode(.message("hello_reply", .record([
        ("selection", .record([("version", .text("0.1"))])), ("correlation_id", .text(requestID))])), from: .runtime))
    XCTAssertEqual(session.drain().count, 2)
}
private func action(epoch: UInt64 = 1, id: String = requestID, deadline: UInt64 = 1_000_000,
                    coglet: String = owner, args: CBORValue = expression) throws -> [UInt8] {
    try SurfaceWire.encode(.message("act", .record([("grant_epoch", .unsigned(epoch)), ("action", .record([
        ("id", .text(id)), ("coglet", .text(coglet)), ("capability", .text("expression.set_expression")),
        ("args", args), ("deadline", .unsigned(deadline)), ("correlation_id", .text(requestID))]))])), from: .runtime)
}
private func drainedSurfaceMessages(_ session: SurfaceSession) throws -> [CBORValue] {
    try session.drain().map { try SurfaceWire.decode($0, from: .surface) }
}

final class SessionTests: XCTestCase {
    func testNativeObservationsShareSequenceNormalizeAndFenceSessionAndEpoch() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let operations = SurfaceOperation.builtins.filter { $0.entry == "speech.control" || $0.entry == "expression.control" }
        let grant = EffectiveSurfaceGrant(coglet: owner, operations: Set(operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)
        let session = try SurfaceSession(identity: identity, boundCoglets: [owner], operations: operations,
            store: store, backend: backend, initialGrants: [grant], clock: { clock.now }, correlation: { requestID })
        try negotiate(session)
        let token = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
        let start = CBORValue.record([("utterance", .text(requestID)), ("position_ms", .unsigned(0))])
        let final = CBORValue.record([("utterance", .text(requestID)), ("revision", .unsigned(1)), ("text", .text("final")), ("position_ms", .unsigned(10))])
        XCTAssertTrue(try session.publishObservation(token, kind: "speech.start", payload: start))
        XCTAssertTrue(try session.publishObservation(token, kind: "speech.end", payload: final))
        XCTAssertFalse(try session.publishObservation(token, kind: "speech.partial", payload: .record([
            ("utterance", .text(requestID)), ("revision", .unsigned(2)), ("text", .text("late"))])))
        try session.receive(action())
        let events = try drainedSurfaceMessages(session).filter { $0["type"] == .text("event") }
        XCTAssertEqual(events.map { $0["body"]!["seq"]! }, [.unsigned(0), .unsigned(1), .unsigned(2)])
        let replacement = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
        XCTAssertFalse(try session.publishObservation(token, kind: "speech.start", payload: start))
        XCTAssertTrue(try session.publishObservation(replacement, kind: "speech.start", payload: start))
        session.closeNativeObservationSession(replacement)
        XCTAssertTrue(session.drain().isEmpty) // close purges queued private observation
        XCTAssertFalse(try session.publishObservation(replacement, kind: "speech.end", payload: final))
        let latest = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
        try session.installGrants([]); _ = session.drain()
        XCTAssertFalse(try session.publishObservation(latest, kind: "speech.start", payload: start))
    }
    func testHeardResponseMustMatchRetainedQueryItem() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let operations = SurfaceOperation.builtins.filter { $0.name == "speech.heard" }
        let grant = EffectiveSurfaceGrant(coglet: owner, operations: ["speech.heard"], resources: ["audio.speaker"], expires: 10_000_000)
        let session = try SurfaceSession(identity: identity, boundCoglets: [owner], operations: operations,
            store: store, backend: backend, initialGrants: [grant], clock: { clock.now }, correlation: { requestID })
        try negotiate(session)
        backend.readValue = .record([("item", .text(String(repeating: "2", count: 32))), ("played_ms", .unsigned(10)), ("text", .text("wrong item"))])
        let query = CBORValue.message("read", .record([("coglet", .text(owner)), ("capability", .text("speech.heard")),
            ("args", .record([("item", .text(requestID))])), ("grant_epoch", .unsigned(1)),
            ("deadline", .unsigned(1_000_000)), ("correlation_id", .text(requestID))]))
        XCTAssertThrowsError(try session.receive(SurfaceWire.encode(query, from: .runtime)))
        XCTAssertTrue(session.drain().isEmpty)
    }
    func testAdmissionDurableBeforeEffectAndHistoricalRetry() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let session = try make(store, backend, clock)
        try negotiate(session)
        backend.onBegin = {
            let persisted = try CBORCodec.decodeProfile(store.bytes)
            XCTAssertEqual(persisted["records"]?.arrayValue, nil)
            guard case .map(let rows) = persisted["records"] else { return XCTFail("no durable admission") }
            XCTAssertEqual(rows.count, 1)
            XCTAssertEqual(rows[0].value["accept"]?["accepted"], .boolean(true))
        }
        let bytes = try action()
        try session.receive(bytes)
        let first = try drainedSurfaceMessages(session)
        XCTAssertEqual(first.count, 2)
        XCTAssertEqual(first[1]["body"]?["payload"]?["status"], .text("succeeded"))
        try session.installGrants([]); _ = session.drain()
        clock.now = 2_000_000
        try session.receive(bytes)
        let retried = try drainedSurfaceMessages(session)
        XCTAssertEqual(retried[0], first[0])
        XCTAssertEqual(retried[1]["body"]?["payload"], first[1]["body"]?["payload"])
        XCTAssertNotEqual(retried[1]["body"]?["seq"], first[1]["body"]?["seq"])
        XCTAssertEqual(backend.begins, 1)
        try session.receive(action(epoch: session.grantEpoch))
        XCTAssertEqual(try drainedSurfaceMessages(session).first?["body"]?["error"]?["code"], .text("conflict"))
    }
    func testPendingDuplicateAndCapturedEpochFence() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        backend.result = nil
        let session = try make(store, backend, clock)
        try negotiate(session)
        try session.receive(action()); try session.receive(action())
        XCTAssertEqual(backend.begins, 1)
        XCTAssertEqual(session.drain().count, 2)
        try session.installGrants([]); _ = session.drain()
        try session.complete(owner: owner, id: requestID, epoch: 1, result: .succeeded(.map([])))
        XCTAssertTrue(session.drain().isEmpty)
        try session.receive(action())
        let historical = try drainedSurfaceMessages(session)
        XCTAssertEqual(historical.count, 2)
        XCTAssertEqual(historical[1]["body"]?["payload"]?["status"], .text("cancelled"))
    }
    func testCrashRecoveryNeverReexecutesPendingAndPersistsSequence() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        backend.result = nil
        var session: SurfaceSession? = try make(store, backend, clock)
        try negotiate(session!)
        try session!.receive(action()); _ = session!.drain()
        // Simulate a crash by reconstructing from the admitted on-disk bytes
        // before graceful disconnect can rewrite the pending record.
        let crashedBytes = store.bytes
        session = nil
        store.bytes = crashedBytes
        let recovered = try make(store, backend, clock)
        try negotiate(recovered)
        XCTAssertEqual(recovered.grantEpoch, 2)
        try recovered.receive(action())
        let replay = try drainedSurfaceMessages(recovered)
        XCTAssertEqual(replay[1]["body"]?["payload"]?["status"], .text("failed"))
        XCTAssertEqual(replay[1]["body"]?["payload"]?["error"]?["code"], .text("unavailable"))
        XCTAssertEqual(backend.begins, 1)
        let recoveredAgain = try make(store, backend, clock)
        try negotiate(recoveredAgain)
        try recoveredAgain.receive(action())
        XCTAssertEqual(try drainedSurfaceMessages(recoveredAgain)[1]["body"]?["seq"], .unsigned(1))
    }
    func testDeadlineEqualityStaleEpochMissingOwnerAndTypedArguments() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let session = try make(store, backend, clock)
        try negotiate(session)
        let cases: [(UInt64, UInt64, String, CBORValue, String)] = [
            (1, 0, owner, expression, "deadline_exceeded"),
            (2, 100, owner, expression, "permission_denied"),
            (1, 60_000_001, owner, expression, "invalid_argument"),
            (1, 100, owner, .map([]), "invalid_argument"),
            (1, 100, "coglet:" + String(repeating: "c", count: 64), expression, "permission_denied")]
        for (index, row) in cases.enumerated() {
            try session.receive(action(epoch: row.0, id: String(format: "%032x", index + 2), deadline: row.1, coglet: row.2, args: row.3))
            let response = try drainedSurfaceMessages(session)
            XCTAssertEqual(response.count, 1)
            XCTAssertEqual(response[0]["body"]?["error"]?["code"], .text(row.4))
        }
        XCTAssertEqual(backend.begins, 0)
    }
    func testInvalidGrantSnapshotDoesNotRevokePreviousAuthority() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let session = try make(store, backend, clock)
        try negotiate(session)
        XCTAssertThrowsError(try session.installGrants([grant(), grant()]))
        XCTAssertEqual(session.grantEpoch, 1)
        try session.receive(action())
        XCTAssertEqual(try drainedSurfaceMessages(session).first?["body"]?["accepted"], .boolean(true))
    }
    func testNativeDeadlineStopsAndOneTerminal() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        backend.result = nil
        let session = try make(store, backend, clock)
        try negotiate(session)
        try session.receive(action(deadline: 100))
        _ = session.drain()
        clock.now = 100
        try session.tick()
        let terminal = try drainedSurfaceMessages(session)
        XCTAssertEqual(terminal.count, 1)
        XCTAssertEqual(terminal[0]["body"]?["payload"]?["status"], .text("expired"))
        try session.complete(owner: owner, id: requestID, epoch: 1, result: .succeeded(.map([])))
        XCTAssertTrue(session.drain().isEmpty)
        XCTAssertGreaterThan(backend.stops, 1)
    }
    func testLatchedKillStopsBeforeNotificationAndSurvivesReconnect() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let session = try make(store, backend, clock)
        try negotiate(session)
        try session.kill(reason: "local operator")
        let notices = try drainedSurfaceMessages(session)
        XCTAssertEqual(notices.map { $0["type"]! }, [.text("kill"), .text("health")])
        XCTAssertEqual(notices[1]["body"]?["state"], .text("stopped"))
        try session.receive(action())
        XCTAssertEqual(try drainedSurfaceMessages(session)[0]["body"]?["error"]?["code"], .text("permission_denied"))
        let reopened = try make(store, backend, clock)
        XCTAssertTrue(reopened.isKilled)
        XCTAssertEqual(backend.begins, 0)
    }
    func testStorageFailureHasNoEffectAndDisconnectIsTerminal() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let session = try make(store, backend, clock)
        try negotiate(session)
        store.failing = true
        XCTAssertThrowsError(try session.receive(action()))
        XCTAssertEqual(backend.begins, 0)
        XCTAssertTrue(session.drain().isEmpty)
        XCTAssertThrowsError(try session.receive(action()))
    }
    func testProtocolLifecycleAndClockHealthFailClosed() throws {
        let store = MemoryLedger(), backend = Backend(), clock = Clock()
        let early = try make(store, backend, clock)
        try early.start(); _ = early.drain()
        XCTAssertThrowsError(try early.receive(action()))
        let session = try make(MemoryLedger(), backend, clock)
        try negotiate(session)
        clock.now = 1_000_000
        try session.tick()
        XCTAssertEqual(try drainedSurfaceMessages(session)[0]["type"], .text("health"))
        clock.now = 3_000_000
        XCTAssertThrowsError(try session.tick())
        let fresh = try make(MemoryLedger(), backend, clock)
        try negotiate(fresh)
        clock.now = 2_000_000
        XCTAssertThrowsError(try fresh.tick())
        XCTAssertEqual(backend.begins, 0)
    }
    func testFileLedgerPrivateReopenAndLossIsFatal() throws {
        let base = ProcessInfo.processInfo.environment["TMPDIR"] ?? FileManager.default.temporaryDirectory.path
        let dir = URL(fileURLWithPath: base).appendingPathComponent("swift-ledger-" + UUID().uuidString)
        let store = try FileSurfaceLedger(directory: dir, newIdentity: true)
        defer { try? FileManager.default.removeItem(at: dir) }
        let ledger = try SurfaceLedger(store: store, identity: identity)
        ledger.epoch = 42; try ledger.save()
        let reopened = try SurfaceLedger(store: FileSurfaceLedger(directory: dir), identity: identity)
        XCTAssertEqual(reopened.epoch, 42)
        XCTAssertThrowsError(try FileSurfaceLedger(directory: dir, newIdentity: true))
        try FileManager.default.removeItem(at: dir.appendingPathComponent("ledger.cbor"))
        XCTAssertThrowsError(try FileSurfaceLedger(directory: dir))
    }
}
