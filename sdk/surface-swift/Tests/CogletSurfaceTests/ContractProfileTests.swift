import Foundation
import XCTest
import CogletTypes
@testable import CogletSurface

final class ContractProfileTests: XCTestCase {
    final class Store: SurfaceLedgerStore {
        var bytes = try! CBORCodec.encodeProfile(.map([]))
        func load() throws -> [UInt8] { bytes }
        func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
    }
    final class Backend: SurfaceNativeBackend {
        var calls = 0
        func preflight(action: CBORValue, now: UInt64) throws {}
        func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? { calls += 1; return .succeeded(.map([])) }
        func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue { .map([]) }
        func stop(owner: String?) {}
    }
    let id = String(repeating: "1", count: 32)
    let owner = "coglet:" + String(repeating: "b", count: 64)
    func catalog() throws -> TrustedExtensionCatalog {
        let uri = "https://example.org/fixture/empty"
        let operation: [String: Any] = ["entry": "softmax.app", "direction": "action", "input": uri, "output": uri,
            "resources": ["softmax.app_control"], "single_owner": true]
        let object: [String: Any] = ["version": 1, "namespace": "softmax", "rules": ["empty": ["op": "record", "fields": []]],
            "schemas": [uri: "empty"], "entries": ["softmax.app": ["softmax.app_control"]],
            "operations": ["softmax.app.invoke": operation,
                "softmax.app.view_changed": ["entry": "softmax.app", "direction": "event", "input": uri, "resources": [], "single_owner": true],
                "softmax.app.focus_changed": ["entry": "softmax.app", "direction": "event", "input": uri, "resources": [], "single_owner": true]]]
        let raw = Array(try JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]))
        return try TrustedExtensionCatalog(json: raw, sha256: catalogDigest(raw), contractProfile: .surface02)
    }
    func session(_ store: Store, _ backend: Backend, catalog: TrustedExtensionCatalog? = nil,
                 profile: SurfaceContractProfile = .surface01) throws -> SurfaceSession {
        let operations = SurfaceOperation.builtins.filter { ["expression.state"].contains($0.name) } + (catalog?.operations ?? [])
        return try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64), boundCoglets: [owner], operations: operations, store: store, backend: backend,
            contractProfile: profile, trustedCatalog: catalog,
            initialGrants: [.init(coglet: owner, operations: Set(operations.map(\.name)), resources: Set(operations.flatMap(\.resources)), expires: 1_000_000)],
            clock: { 10 }, correlation: { self.id })
    }
    func negotiate(_ session: SurfaceSession, version: String) throws -> [CBORValue] {
        try session.start()
        let hello = try SurfaceWire.decode(session.drain()[0], from: .surface)
        XCTAssertTrue(hello["body"]?["offer"]?["versions"]?.arrayValue?.contains(.text(version)) == true)
        let reply = CBORValue.message("hello_reply", .record([("selection", .record([("version", .text(version))])),
            ("correlation_id", .text(id))]))
        try session.receive(SurfaceWire.encode(reply, from: .runtime))
        return try session.drain().map { try SurfaceWire.decode($0, from: .surface, contractProfile: version == "0.2" ? .surface02 : .surface01) }
    }
    func action() -> CBORValue {
        .message("act", .record([("grant_epoch", .unsigned(1)), ("action", .record([
            ("id", .text(id)), ("coglet", .text(owner)), ("capability", .text("softmax.app.invoke")),
            ("args", .map([])), ("deadline", .unsigned(100)), ("correlation_id", .text(id))]))]))
    }
    func testLocalProfileStrictDefaultsAndWire() throws {
        XCTAssertNil(SurfaceContractProfile(rawValue: ""))
        XCTAssertNil(SurfaceContractProfile(rawValue: "surface-0.3"))
        XCTAssertThrowsError(try SurfaceContractProfile.surface01.validateName("softmax.app.invoke"))
        try SurfaceContractProfile.surface02.validateName("softmax.app.invoke")
        XCTAssertThrowsError(try SurfaceContractProfile.surface02.validateName("softmax.app.invoke.extra"))
        XCTAssertThrowsError(try SurfaceWire.encode(action(), from: .runtime))
        let raw = try SurfaceWire.encode(action(), from: .runtime, contractProfile: .surface02)
        XCTAssertThrowsError(try SurfaceWire.decode(raw, from: .runtime))
        XCTAssertEqual(try SurfaceWire.encode(SurfaceWire.decode(raw, from: .runtime, contractProfile: .surface02), from: .runtime, contractProfile: .surface02), raw)
    }
    func testExtensionOperationsRequireOneOwnerEvenWithoutResources() throws {
        let cat = try catalog(), store = Store(), backend = Backend()
        let second = "coglet:" + String(repeating: "c", count: 64)
        for owners: Set<String> in [[], [owner, second]] {
            XCTAssertThrowsError(try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64), boundCoglets: owners,
                operations: cat.operations.filter { $0.direction == .event }, store: store, backend: backend,
                contractProfile: .surface02, trustedCatalog: cat, clock: { 10 }))
        }
        XCTAssertEqual(backend.calls, 0)
    }

    func testCatalogProfilePairAndLegacyLedgerFence() throws {
        let cat = try catalog(), store = Store(), backend = Backend()
        XCTAssertThrowsError(try session(store, backend, catalog: cat))
        let selected = try session(store, backend, catalog: cat, profile: .surface02)
        try selected.close()
        XCTAssertThrowsError(try session(store, backend, profile: .surface02))
        XCTAssertThrowsError(try session(store, backend))
        let reopened = try session(store, backend, catalog: cat, profile: .surface02)
        try reopened.close()
        let legacy = Store()
        let old = try session(legacy, backend); try old.close()
        let root = try CBORCodec.decodeProfile(legacy.bytes)
        guard case .map(let entries) = root else { return XCTFail() }
        legacy.bytes = try CBORCodec.encodeProfile(.map(entries.filter { !["contract_profile", "catalog_pin"].contains($0.key) }))
        XCTAssertThrowsError(try session(legacy, backend, profile: .surface02))
        try session(legacy, backend).close()
    }
    func testSelectedCatalogAdvertisementDispatchAndDurableRetry() throws {
        let cat = try catalog(), store = Store(), backend = Backend()
        let selected = try session(store, backend, catalog: cat, profile: .surface02)
        let frames = try negotiate(selected, version: "0.2")
        let entries = frames[0]["body"]!["entries"]!.arrayValue!
        let entry = try XCTUnwrap(entries.first { $0["name"] == .text("softmax.app") })
        XCTAssertEqual(entry["schemas"]?["softmax.app.invoke"], .text("https://example.org/fixture/empty"))
        let raw = try SurfaceWire.encode(action(), from: .runtime, contractProfile: .surface02)
        try selected.receive(raw)
        XCTAssertEqual(backend.calls, 1)
        let results = try selected.drain().map { try SurfaceWire.decode($0, from: .surface, contractProfile: .surface02) }
        XCTAssertEqual(results[0]["body"]?["accepted"], .boolean(true))
        XCTAssertEqual(results[1]["body"]?["payload"]?["status"], .text("succeeded"))
        try selected.receive(raw); _ = selected.drain(); XCTAssertEqual(backend.calls, 1)
        try selected.close()
        let next = try session(store, backend, catalog: cat, profile: .surface02)
        _ = try negotiate(next, version: "0.2")
        try next.receive(raw); _ = next.drain(); XCTAssertEqual(backend.calls, 1)
        try next.close()
    }
    func testDowngradeKeepsBuiltinAndProvisionedIdentity() throws {
        let cat = try catalog(), store = Store(), backend = Backend()
        let selected = try session(store, backend, catalog: cat, profile: .surface02)
        let frames = try negotiate(selected, version: "0.1")
        let entries = frames[0]["body"]!["entries"]!.arrayValue!
        XCTAssertEqual(entries.map { $0["name"]! }, [.text("expression.control")])
        let grant = frames[1]["body"]!["entries"]!.arrayValue!.first!
        XCTAssertEqual(grant["operations"], .array([.text("expression.state")]))
        XCTAssertEqual(grant["resources"], .array([.text("display.face")]))
        let read = CBORValue.message("read", .record([("coglet", .text(owner)), ("capability", .text("expression.state")),
            ("args", .map([])), ("deadline", .unsigned(100)), ("grant_epoch", .unsigned(selected.grantEpoch)), ("correlation_id", .text(id))]))
        // Backend intentionally lacks expression schema; invalid backend output
        // must fail closed, proving this reaches the built-in read path.
        XCTAssertThrowsError(try selected.receive(SurfaceWire.encode(read, from: .runtime)))
        XCTAssertEqual(try CBORCodec.decodeProfile(store.bytes)["contract_profile"], .text("surface-0.2"))
        XCTAssertEqual(try CBORCodec.decodeProfile(store.bytes)["catalog_pin"], .text(cat.pin))
    }

    func testOperationChannelsCoexistWithSpeechAndShareDurableSequence() throws {
        let cat = try catalog(), store = Store(), backend = Backend()
        let operations = cat.operations + SurfaceOperation.builtins.filter { $0.name == "speech.start" }
        let grants = [EffectiveSurfaceGrant(coglet: owner, operations: Set(operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "softmax.app_control"], expires: 1_000_000)]
        let selected = try SurfaceSession(identity: "surface:" + String(repeating: "a", count: 64), boundCoglets: [owner],
            operations: operations, store: store, backend: backend, contractProfile: .surface02, trustedCatalog: cat,
            initialGrants: grants, clock: { 10 }, correlation: { self.id })
        _ = try negotiate(selected, version: "0.2")
        let view = try selected.openExtensionObservationSession(owner: owner, operation: "softmax.app.view_changed", epoch: selected.grantEpoch)
        let focus = try selected.openExtensionObservationSession(owner: owner, operation: "softmax.app.focus_changed", epoch: selected.grantEpoch)
        let speech = try selected.openNativeObservationSession(owner: owner, epoch: selected.grantEpoch)
        XCTAssertTrue(try selected.publishExtensionObservation(view, payload: .map([]))) // seq 0
        let start = CBORValue.record([("utterance", .text(id)), ("position_ms", .unsigned(0))])
        XCTAssertTrue(try selected.publishObservation(speech, kind: "speech.start", payload: start)) // seq 1
        selected.closeNativeObservationSession(speech) // removes only seq 1
        XCTAssertTrue(selected.extensionObservationIsCurrent(view))
        XCTAssertTrue(try selected.publishExtensionObservation(focus, payload: .map([]))) // seq 2
        let frames = try selected.drain().map { try SurfaceWire.decode($0, from: .surface, contractProfile: .surface02) }
        XCTAssertEqual(frames.map { $0["body"]!["seq"]! }, [.unsigned(0), .unsigned(2)])
        XCTAssertTrue(try selected.publishExtensionObservation(view, payload: .map([]))) // seq 3
        XCTAssertTrue(try selected.publishExtensionObservation(focus, payload: .map([]))) // seq 4
        let replacement = try selected.openExtensionObservationSession(owner: owner, operation: "softmax.app.view_changed", epoch: selected.grantEpoch)
        XCTAssertFalse(try selected.publishExtensionObservation(view, payload: .map([])))
        selected.closeExtensionObservationSession(view) // stale close does not retire replacement
        XCTAssertTrue(selected.extensionObservationIsCurrent(replacement))
        let retained = try selected.drain().map { try SurfaceWire.decode($0, from: .surface, contractProfile: .surface02) }
        XCTAssertEqual(retained.map { $0["body"]!["seq"]! }, [.unsigned(4)])
        try selected.installGrants(grants)
        XCTAssertFalse(selected.extensionObservationIsCurrent(replacement))
        XCTAssertFalse(try selected.publishExtensionObservation(focus, payload: .map([])))
        let current = try selected.openExtensionObservationSession(owner: owner, operation: "softmax.app.view_changed", epoch: selected.grantEpoch)
        _ = selected.drain()
        XCTAssertTrue(try selected.publishExtensionObservation(current, payload: .map([])))
        try selected.kill(reason: "test")
        XCTAssertFalse(try selected.publishExtensionObservation(current, payload: .map([])))
        let killed = try selected.drain().map { try SurfaceWire.decode($0, from: .surface, contractProfile: .surface02) }
        XCTAssertFalse(killed.contains { $0["type"] == .text("event") })
        try selected.close()
    }
}
