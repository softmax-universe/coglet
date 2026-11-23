import XCTest
import CogletTypes
@testable import CogletSurface

private final class FailingStore: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var failing = false
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws {
        if failing { throw NativeSurfaceError("unavailable") }
        self.bytes = bytes
    }
}
private final class ReviewBackend: SurfaceNativeBackend {
    var begins = 0
    func preflight(action: CBORValue, now: UInt64) throws {}
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        begins += 1
        return .succeeded(.map([]))
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue { .map([]) }
    func stop(owner: String?) {}
}
final class KillPersistenceReviewTests: XCTestCase {
    private func make(_ store: FailingStore) throws -> SurfaceSession {
        try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64),
            boundCoglets: [], operations: [], store: store, backend: ReviewBackend(), clock: { 0 })
    }

    func testOrderlyCloseAllowsRestartAndRetiresOldSession() throws {
        let store = FailingStore()
        let first = try make(store)
        try first.close()
        XCTAssertFalse(try make(store).isKilled)
        let saved = store.bytes
        XCTAssertThrowsError(try first.kill(reason: "retired"))
        XCTAssertThrowsError(try first.localOperatorResetForNextConnection())
        XCTAssertEqual(store.bytes, saved)
    }

    func testDisconnectDoesNotClearUncertainRestart() throws {
        let store = FailingStore()
        let first = try make(store)
        first.disconnect()
        XCTAssertTrue(try make(store).isKilled)
    }

    func testResetRequiresNewSessionAndRetiresOldSession() throws {
        let store = FailingStore()
        let first = try make(store)
        try first.kill(reason: "operator")
        XCTAssertThrowsError(try first.localOperatorResetForNextConnection())
        first.disconnect()
        try first.localOperatorResetForNextConnection()
        XCTAssertFalse(try make(store).isKilled)
        let saved = store.bytes
        XCTAssertThrowsError(try first.kill(reason: "retired"))
        XCTAssertThrowsError(try first.localOperatorResetForNextConnection())
        XCTAssertEqual(store.bytes, saved)
    }

    func testFailedTerminalSavesRemainStoppedOnRestart() throws {
        for reset in [false, true] {
            let store = FailingStore()
            let first = try make(store)
            try first.kill(reason: "operator")
            first.disconnect()
            store.failing = true
            if reset { XCTAssertThrowsError(try first.localOperatorResetForNextConnection()) }
            else { XCTAssertThrowsError(try first.close()) }
            XCTAssertTrue(first.isKilled)
            store.failing = false
            XCTAssertTrue(try make(store).isKilled)
        }
    }

    func testOrderlyClosePreservesKillLatch() throws {
        let store = FailingStore()
        let first = try make(store)
        try first.kill(reason: "operator")
        try first.close()
        XCTAssertTrue(try make(store).isKilled)
    }

    func testStartupMarkerMustCommitBeforeSessionCanEscape() throws {
        let store = FailingStore()
        let first = try make(store)
        try first.close()
        store.failing = true
        XCTAssertThrowsError(try make(store))
        store.failing = false
        XCTAssertFalse(try make(store).isKilled)
    }

    func testLegacySnapshotWithoutRunningMarkerFailsClosed() throws {
        let store = FailingStore()
        let first = try make(store)
        try first.close()
        guard case .map(let entries) = try CBORCodec.decodeProfile(store.bytes) else { return XCTFail() }
        store.bytes = try CBORCodec.encodeProfile(.map(entries.filter { $0.key != "running" }))
        XCTAssertTrue(try make(store).isKilled)
    }

    func testFailedKillPersistenceCannotRestoreAuthorityOnRestart() throws {
        let store = FailingStore()
        let backend = ReviewBackend()
        let identity = "surface:" + String(repeating: "a", count: 64)
        let owner = "coglet:" + String(repeating: "b", count: 64)
        let id = String(repeating: "1", count: 32)
        func make() throws -> SurfaceSession {
            try SurfaceSession(identity: identity, boundCoglets: [owner],
                operations: SurfaceOperation.builtins.filter { $0.entry == "expression.control" },
                store: store, backend: backend,
                initialGrants: [.init(coglet: owner, operations: ["expression.set_expression"], resources: ["display.face"], expires: 10_000_000)],
                clock: { 0 }, correlation: { id })
        }
        let first = try make()
        store.failing = true
        XCTAssertThrowsError(try first.kill(reason: "physical off switch"))
        XCTAssertTrue(first.isKilled)
        // Restart with the last successfully persisted snapshot; no operator reset.
        store.failing = false
        let reopened = try make()
        XCTAssertTrue(reopened.isKilled, "A failed kill save must not restore authority on restart without explicit local reset")
        try reopened.start(); _ = reopened.drain()
        try reopened.receive(SurfaceWire.encode(.message("hello_reply", .record([
            ("selection", .record([("version", .text("0.1"))])), ("correlation_id", .text(id))])), from: .runtime))
        _ = reopened.drain()
        let action = CBORValue.message("act", .record([
            ("grant_epoch", .unsigned(reopened.grantEpoch)),
            ("action", .record([("id", .text(id)), ("coglet", .text(owner)),
                ("capability", .text("expression.set_expression")),
                ("args", .record([("expression", .text("listening")), ("intensity", .unsigned(500)), ("duration_ms", .unsigned(1000))])),
                ("deadline", .unsigned(1_000_000)), ("correlation_id", .text(id))]))]))
        try reopened.receive(SurfaceWire.encode(action, from: .runtime))
        XCTAssertEqual(backend.begins, 0, "Native effect restarted after kill without local operator reset")
    }
}
