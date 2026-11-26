import Foundation
import XCTest
import CogletTypes
@testable import CogletSurface

final class SourceIncarnationTests: XCTestCase {
    final class Store: SurfaceLedgerStore {
        var bytes = try! CBORCodec.encodeProfile(.map([]))
        var onSave: (() -> Void)?
        var failOnce = false
        var saves = 0
        func load() throws -> [UInt8] { bytes }
        func save(_ bytes: [UInt8]) throws {
            saves += 1
            if failOnce { failOnce = false; throw NativeSurfaceError("unavailable") }
            self.bytes = bytes; onSave?()
        }
    }
    final class Backend: SurfaceNativeBackend {
        func preflight(action: CBORValue, now: UInt64) throws {}
        func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? { .succeeded(.map([])) }
        func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue { .map([]) }
        func stop(owner: String?) {}
    }
    let id = String(repeating: "1", count: 32)
    let owner = "coglet:" + String(repeating: "b", count: 64)

    func session(_ store: Store, clock: @escaping () -> UInt64 = { 10 }) throws -> SurfaceSession {
        let operations = SurfaceOperation.builtins.filter { $0.name == "speech.start" }
        return try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64), boundCoglets: [owner],
            operations: operations, store: store, backend: Backend(),
            initialGrants: [.init(coglet: owner, operations: ["speech.start"], resources: ["audio.mic", "audio.speaker"], expires: 1_000_000)],
            clock: clock, correlation: { self.id })
    }
    func testRollbackAfterReservationSaveConsumesGapButReturnsNoIdentity() throws {
        let store = Store()
        var now: UInt64 = 10
        let surface = try session(store, clock: { now })
        store.onSave = { store.onSave = nil; now = 9 }
        XCTAssertThrowsError(try surface.reserveSourceIncarnation())
        XCTAssertEqual(try persistedSequence(store), 1)
        now = 10
        XCTAssertThrowsError(try surface.reserveSourceIncarnation())
    }
    func negotiate(_ session: SurfaceSession) throws {
        try session.start(); _ = session.drain()
        let reply = CBORValue.message("hello_reply", .record([("selection", .record([("version", .text("0.1"))])),
            ("correlation_id", .text(id))]))
        try session.receive(SurfaceWire.encode(reply, from: .runtime))
        _ = session.drain()
    }
    func persistedSequence(_ store: Store) throws -> UInt64 {
        try XCTUnwrap(CBORCodec.decodeProfile(store.bytes)["sequence"]?.uintValue)
    }
    /// Publishes one speech.start event and returns its wire seq.
    func publish(_ session: SurfaceSession, utterance: String) throws -> UInt64 {
        let token = try session.openNativeObservationSession(owner: owner, epoch: session.grantEpoch)
        let start = CBORValue.record([("utterance", .text(utterance)), ("position_ms", .unsigned(0))])
        XCTAssertTrue(try session.publishObservation(token, kind: "speech.start", payload: start))
        let frames = try session.drain().map { try SurfaceWire.decode($0, from: .surface) }
        XCTAssertEqual(frames.count, 1)
        return try XCTUnwrap(frames[0]["body"]?["seq"]?.uintValue)
    }

    func testCleanReopenReservationIsMonotonicAndInterleavedWithEvents() throws {
        let store = Store()
        let first = try session(store)
        // Works before negotiation, emits nothing, mints no grant frames.
        XCTAssertEqual(try first.reserveSourceIncarnation(), 0)
        XCTAssertTrue(first.drain().isEmpty)
        XCTAssertEqual(try persistedSequence(store), 1) // durable before return
        try negotiate(first)
        XCTAssertEqual(try publish(first, utterance: id), 1)
        XCTAssertEqual(try first.reserveSourceIncarnation(), 2)
        XCTAssertEqual(try publish(first, utterance: String(repeating: "2", count: 32)), 3)
        try first.close()
        let second = try session(store)
        XCTAssertEqual(try second.reserveSourceIncarnation(), 4)
        XCTAssertEqual(try second.reserveSourceIncarnation(), 5)
        XCTAssertEqual(try persistedSequence(store), 6)
        try second.close()
        let third = try session(store)
        try negotiate(third)
        XCTAssertEqual(try publish(third, utterance: id), 6) // no collision with 0,2,4,5
        XCTAssertEqual(try third.reserveSourceIncarnation(), 7)
        try third.close()
        // No extra stored keys: schema is unchanged.
        guard case .map(let entries) = try CBORCodec.decodeProfile(store.bytes) else { return XCTFail() }
        XCTAssertEqual(Set(entries.map(\.key)), ["identity", "contract_profile", "catalog_pin", "epoch", "sequence", "killed", "running", "records"])
    }

    func testDisconnectFromSaveCallbackReturnsNoIncarnationAndKeepsConsumedGap() throws {
        let store = Store()
        let s = try session(store)
        XCTAssertEqual(try s.reserveSourceIncarnation(), 0)
        store.onSave = { store.onSave = nil; s.disconnect() }
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        XCTAssertEqual(try persistedSequence(store), 2) // 1 consumed, never rolled back
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        XCTAssertEqual(try persistedSequence(store), 2)
        // Reopened after an unclean stop: never hands out a value from a killed ledger.
        let reopened = try session(store)
        XCTAssertTrue(reopened.isKilled)
        XCTAssertThrowsError(try reopened.reserveSourceIncarnation())
    }

    func testKillFromSaveCallbackReturnsNoIncarnation() throws {
        let store = Store()
        let s = try session(store)
        store.onSave = { store.onSave = nil; try? s.kill(reason: "test") }
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        XCTAssertTrue(s.isKilled)
        XCTAssertEqual(try persistedSequence(store), 1)
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
    }

    func testCloseFromSaveCallbackReturnsNoIncarnation() throws {
        let store = Store()
        let s = try session(store)
        store.onSave = { store.onSave = nil; try? s.close() }
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        XCTAssertEqual(try persistedSequence(store), 1)
        let saved = store.bytes, saves = store.saves
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        XCTAssertEqual(store.bytes, saved); XCTAssertEqual(store.saves, saves) // retired: no write
        let reopened = try session(store) // orderly close: healthy restart, gap retained
        XCTAssertFalse(reopened.isKilled)
        XCTAssertEqual(try reopened.reserveSourceIncarnation(), 1)
        try reopened.close()
    }

    func testSaveFailureIsTerminalAndNoValueReturned() throws {
        let store = Store()
        let s = try session(store)
        store.failOnce = true
        XCTAssertThrowsError(try s.reserveSourceIncarnation())
        // Terminal disconnect's own save succeeded and persisted the consumed gap.
        XCTAssertEqual(try persistedSequence(store), 1)
        XCTAssertThrowsError(try s.reserveSourceIncarnation()) // healthy store does not revive it
        XCTAssertEqual(try persistedSequence(store), 1)
        XCTAssertThrowsError(try s.publishNothing())
    }

    func testKilledOrRetiredSessionRefusesWithoutWriting() throws {
        let store = Store()
        let killed = try session(store)
        try killed.kill(reason: "test")
        var saves = store.saves
        XCTAssertThrowsError(try killed.reserveSourceIncarnation())
        XCTAssertGreaterThanOrEqual(store.saves, saves) // terminal disconnect may save; sequence untouched
        XCTAssertEqual(try persistedSequence(store), 0)
        let other = Store()
        let retired = try session(other)
        try retired.close()
        saves = other.saves
        XCTAssertThrowsError(try retired.reserveSourceIncarnation())
        XCTAssertEqual(other.saves, saves)
        XCTAssertEqual(try persistedSequence(other), 0)
    }

    func testExhaustionRefusesTerminallyWithoutWrapping() throws {
        let store = Store()
        try session(store).close()
        guard case .map(let entries) = try CBORCodec.decodeProfile(store.bytes) else { return XCTFail() }
        store.bytes = try CBORCodec.encodeProfile(.map(entries.map {
            $0.key == "sequence" ? CBORMapEntry("sequence", .unsigned(UInt64.max - 1)) : $0 }))
        let s = try session(store)
        XCTAssertEqual(try s.reserveSourceIncarnation(), UInt64.max - 1)
        XCTAssertEqual(try persistedSequence(store), UInt64.max)
        XCTAssertThrowsError(try s.reserveSourceIncarnation()) { XCTAssertEqual($0 as? SurfaceFailure, .exhausted) }
        XCTAssertEqual(try persistedSequence(store), UInt64.max)
        XCTAssertThrowsError(try s.reserveSourceIncarnation()) // disconnected now
    }
}

private extension SurfaceSession {
    /// Proves the failed reservation left the session disconnected.
    func publishNothing() throws { try tick() }
}
