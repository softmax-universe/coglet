import XCTest
@testable import SoftmaxCore

@MainActor
final class HubConnectionTests: XCTestCase {
    func testKeychainIdentitySurvivesRestartAndSignOutErasesIt() throws {
        let store = MemoryHubStore()
        let first = HubAccount(store: store, generate: { Data(repeating: 7, count: 32) })
        XCTAssertEqual(try first.identity(), Data(repeating: 7, count: 32))
        let restarted = HubAccount(store: store, generate: { XCTFail("must not rotate"); return Data() })
        XCTAssertEqual(try restarted.identity(), try first.identity())
        try restarted.signOut()
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
    }
    func testUnreadableKeyNeverSilentlyRotates() {
        let store = MemoryHubStore(); store.fail = true
        let account = HubAccount(store: store, generate: { XCTFail("must not generate"); return Data() })
        XCTAssertThrowsError(try account.identity())
    }
    func testOfflineReconnectAndLateCallbackAfterSignOut() throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let hub = HubConnection(account: HubAccount(store: store, generate: { Data(repeating: 8, count: 32) }), link: link)
        hub.start()
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        let pairing = try HubPairing.parse(#"{"name":"Test hub","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"],"invite":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
        try hub.enroll(pairing)
        link.emit(.connected)
        XCTAssertEqual(hub.state, .connected)
        XCTAssertNil(try store.loadHub()?.invite)
        link.emit(.offline)
        XCTAssertEqual(hub.state, .offline)
        hub.retryNow()
        XCTAssertNil(link.lastPairing?.invite)
        link.emit(.connected)
        XCTAssertEqual(hub.state, .connected)
        let stale = link.callback
        hub.signOut()
        stale?(.connected)
        XCTAssertEqual(hub.state, .offline)
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
    }
    func testHelperCrashBeforePairingRestartsWithoutRotatingIdentity() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let hub = HubConnection(account: HubAccount(store: store, generate: { Data(repeating: 9, count: 32) }), link: link)
        hub.start()
        defer { hub.stop() }
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        let identity = try store.loadKey()
        link.emit(.failed)
        XCTAssertEqual(hub.state, .offline)
        let deadline = ContinuousClock.now + .seconds(3)
        while link.starts < 2, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(link.starts, 2, "An unpaired runtime must recover its pairing channel after a helper crash")
        XCTAssertEqual(try store.loadKey(), identity)
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(hub.deviceID, String(repeating: "a", count: 64))
    }

    func testTransientHelperLaunchFailureRestartsWithoutRotatingIdentity() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let key = Data(repeating: 6, count: 32)
        link.failStarts = 1
        let hub = HubConnection(account: HubAccount(store: store, generate: { key }), link: link)
        hub.start()
        defer { hub.stop() }
        XCTAssertEqual(hub.state, .offline)
        let deadline = ContinuousClock.now + .seconds(3)
        while link.starts < 2, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(link.starts, 2, "A transient helper launch failure must re-arm startup")
        XCTAssertEqual(try store.loadKey(), key)
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(hub.deviceID, String(repeating: "a", count: 64))
    }

    func testPairingRejectsMalformedAndMismatchedDevice() throws {
        XCTAssertThrowsError(try HubPairing.parse("123456"))
        XCTAssertThrowsError(try HubPairing.parse(#"{"name":"Hub","hub":"invalid","addresses":[]}"#))
        let store = MemoryHubStore(), link = FakeHubLink()
        let hub = HubConnection(account: HubAccount(store: store, generate: { Data(repeating: 1, count: 32) }), link: link)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        let pairing = try HubPairing.parse(#"{"name":"Hub","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":[],"device":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
        XCTAssertThrowsError(try hub.enroll(pairing))
        hub.signOut()
    }
}

@MainActor
final class MemoryHubStore: HubStore {
    var key: Data?, hub: HubPairing?, fail = false
    func loadKey() throws -> Data? { if fail { throw HubError.storage }; return key }
    func saveKey(_ value: Data) throws { if fail { throw HubError.storage }; key = value }
    func deleteKey() throws { if fail { throw HubError.storage }; key = nil }
    func loadHub() throws -> HubPairing? { hub }
    func saveHub(_ value: HubPairing?) throws { hub = value }
}
@MainActor
final class FakeHubLink: HubLink {
    var callback: ((HubLinkEvent) -> Void)?
    var lastPairing: HubPairing?
    var starts = 0
    var failStarts = 0
    func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws {
        starts += 1
        if failStarts > 0 { failStarts -= 1; throw HubError.unavailable }
        callback = event
    }
    func connect(_ pairing: HubPairing) throws { lastPairing = pairing }
    func stop() { callback = nil }
    func emit(_ event: HubLinkEvent) { callback?(event) }
}
