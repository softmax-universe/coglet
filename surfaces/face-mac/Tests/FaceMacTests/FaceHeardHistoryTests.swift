import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class HeardLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    var saves = 0
    var failSaves = false
    var failLoad = false
    func load() throws -> [UInt8] { if failLoad { throw SurfaceFailure.closed }; return bytes }
    func save(_ bytes: [UInt8]) throws {
        if failSaves { throw SurfaceFailure.closed }
        self.bytes = bytes; saves += 1
    }
}


private func iid(_ n: Int) -> String { String(repeating: "0", count: 24) + String(format: "%08x", n) }
private let owner = "coglet:" + String(repeating: "a", count: 64)
private let other = "coglet:" + String(repeating: "b", count: 64)
private func code(_ body: () throws -> Void, file: StaticString = #filePath, line: UInt = #line) -> String? {
    do { try body(); XCTFail("expected failure", file: file, line: line); return nil }
    catch let error as NativeSurfaceError { return error.code }
    catch { XCTFail("unexpected \(error)", file: file, line: line); return nil }
}
private func fails(_ body: () throws -> Void) -> Bool { do { try body(); return false } catch { return true } }
private func text(_ value: CBORValue) -> String? { if case .text(let t)? = value["text"] { return t }; return nil }

final class FaceHeardHistoryTests: XCTestCase {
    func testDuplicatePlaybackDoesNotRewriteAlreadyDurableSnapshot() async throws {
        try await MainActor.run {
            let store = HeardLedger(), history = try FaceHeardHistory(store: store)
            try history.register(item: iid(1), owner: owner, epoch: 1)
            try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 10, stopped: false)
            let saves = store.saves
            store.failSaves = true
            try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 10, stopped: false)
            XCTAssertEqual(store.saves, saves)
            XCTAssertTrue(history.isAvailable)
            XCTAssertThrowsError(try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 11, stopped: false))
            XCTAssertFalse(history.isAvailable)
        }
    }

    func testDurableReopenKeepsPrefixAndPristineStoreIsEmpty() async throws {
        try await MainActor.run {
            let store = HeardLedger()
            var history: FaceHeardHistory? = try FaceHeardHistory(store: store)
            XCTAssertEqual(store.saves, 0)
            try history!.register(item: iid(1), owner: owner, epoch: 1)
            try history!.append(item: iid(1), owner: owner, epoch: 1,
                                spans: [.init(endMS: 100, text: "Hello "), .init(endMS: 200, text: "world")])
            try history!.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 150, stopped: false)
            XCTAssertEqual(store.saves, 3)
            history = nil
            let reopened = try FaceHeardHistory(store: store)
            let heard = try reopened.heard(item: iid(1), owner: owner, epoch: 1)
            XCTAssertEqual(text(heard), "Hello ")
            XCTAssertEqual(heard["played_ms"], .unsigned(150))
            // Continues extending after reopen; never rewrites.
            try reopened.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 200, stopped: false)
            XCTAssertEqual(text(try reopened.heard(item: iid(1), owner: owner, epoch: 1)), "Hello world")
        }
    }

    func testNilStoreIsInMemory() async throws {
        try await MainActor.run {
            let history = try FaceHeardHistory(store: nil)
            try history.register(item: iid(1), owner: owner, epoch: 1)
            XCTAssertEqual(text(try history.heard(item: iid(1), owner: owner, epoch: 1)), "")
        }
    }

    func testConservativePrefixAndQueuedZero() async throws {
        try await MainActor.run {
            let history = try FaceHeardHistory(store: HeardLedger())
            try history.register(item: iid(1), owner: owner, epoch: 1)
            XCTAssertEqual(text(try history.heard(item: iid(1), owner: owner, epoch: 1)), "")
            try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 100, text: "ab"), .init(endMS: 200, text: "cd")])
            XCTAssertEqual(text(try history.heard(item: iid(1), owner: owner, epoch: 1)), "")
            try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 199, stopped: false)
            XCTAssertEqual(text(try history.heard(item: iid(1), owner: owner, epoch: 1)), "ab")
            XCTAssertEqual(code { try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 200, text: "x")]) }, "invalid_argument")
            XCTAssertEqual(code { try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 10, stopped: false) }, "invalid_argument")
        }
    }

    func testFrozenStopNeverAdvances() async throws {
        try await MainActor.run {
            let store = HeardLedger()
            let history = try FaceHeardHistory(store: store)
            try history.register(item: iid(1), owner: owner, epoch: 1)
            try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 100, text: "ab"), .init(endMS: 200, text: "cd")])
            try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 100, stopped: true)
            XCTAssertEqual(code { try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 300, text: "e")]) }, "conflict")
            XCTAssertEqual(code { try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 200, stopped: true) }, "invalid_argument")
            XCTAssertEqual(code { try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 100, stopped: false) }, "invalid_argument")
            try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 100, stopped: true) // idempotent
            let reopened = try FaceHeardHistory(store: store)
            XCTAssertEqual(code { try reopened.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 200, stopped: true) }, "invalid_argument")
            XCTAssertEqual(text(try reopened.heard(item: iid(1), owner: owner, epoch: 1)), "ab")
        }
    }

    func testOwnerEpochAndUnknown() async throws {
        try await MainActor.run {
            let history = try FaceHeardHistory(store: HeardLedger())
            try history.register(item: iid(1), owner: owner, epoch: 2)
            XCTAssertEqual(code { _ = try history.heard(item: iid(1), owner: other, epoch: 2) }, "permission_denied")
            XCTAssertEqual(code { _ = try history.heard(item: iid(1), owner: owner, epoch: 1) }, "permission_denied")
            XCTAssertEqual(code { try history.append(item: iid(1), owner: other, epoch: 2, spans: []) }, "permission_denied")
            XCTAssertEqual(code { try history.playback(item: iid(1), owner: owner, epoch: 3, playedMS: 1, stopped: false) }, "permission_denied")
            XCTAssertEqual(code { _ = try history.heard(item: iid(5000), owner: owner, epoch: 2) }, "not_found")
            XCTAssertEqual(code { try history.append(item: iid(5000), owner: owner, epoch: 2, spans: []) }, "not_found")
            XCTAssertEqual(code { try history.playback(item: iid(5000), owner: owner, epoch: 2, playedMS: 1, stopped: false) }, "not_found")
            XCTAssertEqual(code { try history.register(item: iid(1), owner: other, epoch: 5) }, "conflict")
            XCTAssertEqual(code { try history.register(item: iid(2), owner: owner, epoch: 0) }, "conflict")
        }
    }

    func testFailedSaveLatchesUnavailable() async throws {
        try await MainActor.run {
            let store = HeardLedger()
            let history = try FaceHeardHistory(store: store)
            try history.register(item: iid(1), owner: owner, epoch: 1)
            store.failSaves = true
            XCTAssertTrue(fails { try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 1, text: "a")]) })
            store.failSaves = false // recovery of the disk must not revive this instance
            XCTAssertEqual(code { _ = try history.heard(item: iid(1), owner: owner, epoch: 1) }, "unavailable")
            XCTAssertEqual(code { try history.register(item: iid(2), owner: owner, epoch: 1) }, "unavailable")
            XCTAssertEqual(code { try history.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 1, stopped: false) }, "unavailable")
            XCTAssertEqual(code { try history.revokeAll() }, "unavailable")
            // The failed mutation is not durable; a fresh instance sees pre-failure state only.
            let fresh = try FaceHeardHistory(store: store)
            XCTAssertEqual(text(try fresh.heard(item: iid(1), owner: owner, epoch: 1)), "")
            try fresh.playback(item: iid(1), owner: owner, epoch: 1, playedMS: 1, stopped: false)
        }
    }

    func testRevokeAllPersistsAndFailureLatches() async throws {
        try await MainActor.run {
            let store = HeardLedger()
            let history = try FaceHeardHistory(store: store)
            try history.register(item: iid(1), owner: owner, epoch: 1)
            try history.revokeAll()
            XCTAssertEqual(code { _ = try history.heard(item: iid(1), owner: owner, epoch: 1) }, "not_found")
            let reopened = try FaceHeardHistory(store: store)
            XCTAssertEqual(code { _ = try reopened.heard(item: iid(1), owner: owner, epoch: 1) }, "not_found")
            try reopened.register(item: iid(1), owner: other, epoch: 9)

            store.failSaves = true
            XCTAssertTrue(fails { try reopened.revokeAll() })
            store.failSaves = false
            XCTAssertEqual(code { try reopened.register(item: iid(9), owner: owner, epoch: 1) }, "unavailable")
            XCTAssertEqual(code { _ = try reopened.heard(item: iid(1), owner: other, epoch: 9) }, "unavailable")
        }
    }

    func testCorruptStoreIsFatalWithoutFallback() async throws {
        try await MainActor.run {
            let good = HeardLedger()
            let h = try FaceHeardHistory(store: good)
            try h.register(item: iid(1), owner: owner, epoch: 1)
            try h.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 5, text: "a")])
            let valid = good.bytes
            var corrupt: [[UInt8]] = [[], [0x00], [0xff, 0xff], Array(valid.dropLast()), valid + [0x00]]
            // Unknown version, wrong shape, and semantically invalid states.
            func encoded(_ value: CBORValue) throws -> [UInt8] { try CBORCodec.encodeProfile(value) }
            func row(o: String = owner, epoch: UInt64 = 1, spans: [CBORValue] = [], played: UInt64 = 0, stopped: Bool = false) -> CBORValue {
                .record([("owner", .text(o)), ("epoch", .unsigned(epoch)), ("spans", .array(spans)),
                         ("played_ms", .unsigned(played)), ("stopped", .boolean(stopped))])
            }
            func span(_ end: UInt64, _ t: String) -> CBORValue { .record([("end_ms", .unsigned(end)), ("text", .text(t))]) }
            func doc(version: UInt64 = 1, items: [(String, CBORValue)]) -> CBORValue {
                .record([("version", .unsigned(version)), ("items", .map(items.sorted { $0.0 < $1.0 }.map { CBORMapEntry($0.0, $0.1) }))])
            }
            corrupt.append(try encoded(doc(version: 2, items: [])))
            corrupt.append(try encoded(.record([("version", .unsigned(1))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(epoch: 0))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(o: "bad owner"))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(spans: [span(5, "a"), span(5, "b")]))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(spans: [span(0, "a")]))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(spans: [span(5, String(repeating: "x", count: 16_385))]))])))
            corrupt.append(try encoded(doc(items: [(iid(1), row(spans: [span(5, "a")], played: 9, stopped: false)), (iid(2), .text("no"))])))
            corrupt.append(try encoded(.map([CBORMapEntry("unexpected", .unsigned(1))])))
            for (index, bytes) in corrupt.enumerated() {
                let store = HeardLedger(); store.bytes = bytes
                XCTAssertThrowsError(try FaceHeardHistory(store: store), "corrupt case \(index)")
                XCTAssertEqual(store.bytes, bytes, "must not rewrite a corrupt store (case \(index))")
            }
            let failing = HeardLedger(); failing.failLoad = true
            XCTAssertThrowsError(try FaceHeardHistory(store: failing))
            // The valid snapshot still opens.
            let ok = HeardLedger(); ok.bytes = valid
            XCTAssertNoThrow(try FaceHeardHistory(store: ok))
        }
    }

    func testCapacityRefusesWithoutEvictionAndSurvivesReopen() async throws {
        try await MainActor.run {
            let store = HeardLedger()
            let history = try FaceHeardHistory(store: store)
            XCTAssertEqual(FaceHeardHistory.itemCapacity, 1024)
            for i in 0..<FaceHeardHistory.itemCapacity { try history.register(item: iid(i), owner: owner, epoch: 1) }
            XCTAssertEqual(code { try history.register(item: iid(99999), owner: owner, epoch: 1) }, "resource_exhausted")
            // Refusal is not a persistence failure: reads/mutations still work, nothing evicted.
            XCTAssertEqual(text(try history.heard(item: iid(0), owner: owner, epoch: 1)), "")
            let reopened = try FaceHeardHistory(store: store)
            XCTAssertEqual(code { try reopened.register(item: iid(99999), owner: owner, epoch: 1) }, "resource_exhausted")
            XCTAssertEqual(text(try reopened.heard(item: iid(1023), owner: owner, epoch: 1)), "")
        }
    }

    func testSpanLimitsMatchSpeechObservations() async throws {
        try await MainActor.run {
            let history = try FaceHeardHistory(store: HeardLedger())
            try history.register(item: iid(1), owner: owner, epoch: 1)
            XCTAssertEqual(code { try history.append(item: iid(1), owner: owner, epoch: 1,
                                                     spans: [.init(endMS: 1, text: String(repeating: "x", count: 16_385))]) }, "resource_exhausted")
            XCTAssertEqual(code { try history.append(item: iid(1), owner: owner, epoch: 1, spans: [.init(endMS: 0, text: "x")]) }, "invalid_argument")
        }
    }
}
