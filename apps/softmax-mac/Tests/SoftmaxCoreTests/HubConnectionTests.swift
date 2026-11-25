import XCTest
@testable import SoftmaxCore

@MainActor
final class HubConnectionTests: XCTestCase {
    func testExplicitReconnectHasFreshCorrelationAndActualCompletion() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        store.key = Data(repeating: 8, count: 32)
        store.hub = try HubPairing.parse(#"{"name":"Stored router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"]}"#)
        let hub = HubConnection(account: HubAccount(store: store), link: link)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        let old = link.callback
        let started = expectation(description: "fresh helper started")
        link.didStart = { started.fulfill() }
        var completed = false
        let task = Task { try await hub.reconnectOnce(); completed = true }
        await fulfillment(of: [started], timeout: 2)
        XCTAssertEqual(link.starts, 2)
        XCTAssertFalse(completed)
        old?(.connected)
        XCTAssertEqual(hub.state, .connecting, "Original channel cannot complete new operation")
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertFalse(completed, "Queued connect is not terminal success")
        link.emit(.connected)
        try await task.value
        XCTAssertTrue(completed)
        XCTAssertEqual(hub.state, .connected)
        link.emit(.offline)
        XCTAssertEqual(hub.state, .offline, "Normal callbacks survive successful operation retirement")
        hub.stop()
    }
    func testExplicitReconnectRefusesDeferredEffectAfterCustodyRevoked() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        store.key = Data(repeating: 8, count: 32)
        store.hub = try HubPairing.parse(#"{"name":"Stored router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"]}"#)
        let hub = HubConnection(account: HubAccount(store: store), link: link)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        let initialConnects = link.connects
        let started = expectation(description: "fresh helper started")
        link.didStart = { started.fulfill() }
        var live = true
        let task = Task { try await hub.reconnectOnce { guard live else { throw HubError.unavailable } } }
        await fulfillment(of: [started], timeout: 2)
        let stale = link.callback
        live = false
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        do { try await task.value; XCTFail("revoked reconnect succeeded") } catch {}
        XCTAssertEqual(link.connects, initialConnects)
        XCTAssertEqual(hub.state, .offline)
        stale?(.connected)
        XCTAssertEqual(hub.state, .offline)
        hub.stop()
    }
    func testExplicitReconnectCancellationStopsOnlyOriginalAttempt() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        store.key = Data(repeating: 8, count: 32)
        store.hub = try HubPairing.parse(#"{"name":"Stored router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"]}"#)
        let hub = HubConnection(account: HubAccount(store: store), link: link)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        let started = expectation(description: "fresh helper started")
        link.didStart = { started.fulfill() }
        let task = Task { try await hub.reconnectOnce() }
        await fulfillment(of: [started], timeout: 2)
        let stale = link.callback
        task.cancel()
        do { try await task.value; XCTFail("cancelled reconnect succeeded") } catch {}
        XCTAssertEqual(hub.state, .offline)
        stale?(.connected)
        XCTAssertEqual(hub.state, .offline)
        let successorStarted = expectation(description: "successor helper started")
        link.didStart = { successorStarted.fulfill() }
        let successor = Task { try await hub.reconnectOnce() }
        await fulfillment(of: [successorStarted], timeout: 2)
        stale?(.failed)
        XCTAssertEqual(hub.state, .connecting)
        link.emit(.ready(device: String(repeating: "a", count: 64))); link.emit(.connected)
        try await successor.value
        hub.stop()
    }
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

    func testEnrollmentSaveFailureRestartsChannelAndRecoversSameIdentity() async throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let key = Data(repeating: 4, count: 32)
        let hub = HubConnection(account: HubAccount(store: store, generate: { key }), link: link)
        hub.start()
        defer { hub.stop() }
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        let pairing = try HubPairing.parse(#"{"name":"Test router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"],"invite":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
        try hub.enroll(pairing)
        let stale = link.callback
        store.failHubSave = true
        link.emit(.connected)
        XCTAssertEqual(hub.state, .offline)
        XCTAssertNil(try store.loadHub())
        store.failHubSave = false
        let deadline = ContinuousClock.now + .seconds(3)
        while link.starts < 2, ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertEqual(link.starts, 2, "Enrollment persistence failure must restart the recovery channel")
        XCTAssertEqual(try store.loadKey(), key)
        stale?(.connected)
        XCTAssertNil(try store.loadHub(), "The old channel cannot persist an enrollment")
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(link.lastPairing?.hub, pairing.hub)
        link.emit(.connected)
        XCTAssertEqual(hub.state, .connected)
        XCTAssertNil(try store.loadHub()?.invite)
    }

    func testOfflinePendingInvitationCanBeReplacedWithoutAcceptingOldCallbacks() throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let key = Data(repeating: 3, count: 32)
        let hub = HubConnection(account: HubAccount(store: store, generate: { key }), link: link)
        hub.start()
        defer { hub.stop() }
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        let original = try HubPairing.parse(#"{"name":"Original router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"],"invite":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
        let replacement = try HubPairing.parse(#"{"name":"Replacement router","hub":"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc","addresses":["127.0.0.1:4321"],"invite":"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}"#)
        try hub.enroll(original)
        XCTAssertThrowsError(try hub.enroll(replacement), "An active enrollment cannot be replaced implicitly")
        link.emit(.offline)
        let stale = link.callback
        try hub.enroll(replacement)
        XCTAssertEqual(link.starts, 2, "Replacement must fence the original channel")
        stale?(.connected)
        XCTAssertNil(try store.loadHub(), "A late original response must not enroll the replacement")
        XCTAssertEqual(try store.loadKey(), key)
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(link.lastPairing?.hub, replacement.hub)
        link.emit(.connected)
        XCTAssertEqual(hub.pairing?.name, replacement.name)
        XCTAssertEqual(hub.state, .connected)
        XCTAssertNil(try store.loadHub()?.invite)
        XCTAssertThrowsError(try hub.enroll(original), "An enrolled router still requires explicit sign out")
    }

    func testFailedSignOutKeepsRetryReachableAndLaterSucceeds() throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        let hub = HubConnection(account: HubAccount(store: store, generate: { Data(repeating: 9, count: 32) }), link: link)
        hub.start()
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertNotNil(hub.deviceID)
        store.fail = true
        hub.signOut()
        XCTAssertNotNil(hub.deviceID, "Sign out must stay reachable after a failed cleanup")
        XCTAssertNotNil(hub.errorMessage)
        store.fail = false
        hub.signOut()
        XCTAssertNil(hub.deviceID)
        XCTAssertNil(hub.errorMessage)
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
    }
    func testStartupIdentityFailureKeepsSignOutReachableWithoutRotatingKey() throws {
        let store = MemoryHubStore(), link = FakeHubLink()
        store.hub = try HubPairing.parse(#"{"name":"Stored router","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":["127.0.0.1:1234"]}"#)
        let hub = HubConnection(account: HubAccount(store: store, generate: {
            XCTFail("A missing enrolled identity must not silently rotate")
            return Data(repeating: 1, count: 32)
        }), link: link)
        hub.start()
        defer { hub.stop() }
        XCTAssertEqual(link.starts, 0)
        XCTAssertNil(hub.deviceID)
        XCTAssertNotNil(hub.errorMessage)
        XCTAssertTrue(hub.canSignOut, "Startup identity errors must expose the explicit local reset action")
        store.failHubSave = true
        hub.signOut()
        XCTAssertTrue(hub.canSignOut, "Failed cleanup must leave the action reachable")
        store.failHubSave = false
        hub.signOut()
        XCTAssertFalse(hub.canSignOut)
        XCTAssertNil(hub.errorMessage)
        XCTAssertNil(try store.loadHub())
        XCTAssertNil(try store.loadKey())
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
    var failHubSave = false
    func loadKey() throws -> Data? { if fail { throw HubError.storage }; return key }
    func saveKey(_ value: Data) throws { if fail { throw HubError.storage }; key = value }
    func deleteKey() throws { if fail { throw HubError.storage }; key = nil }
    func loadHub() throws -> HubPairing? { hub }
    func saveHub(_ value: HubPairing?) throws { if failHubSave { throw HubError.storage }; hub = value }
}
@MainActor
final class FakeHubLink: HubLink {
    var callback: ((HubLinkEvent) -> Void)?
    var lastPairing: HubPairing?
    var starts = 0
    var connects = 0
    var didStart: (() -> Void)?
    var failStarts = 0
    func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws {
        starts += 1
        if failStarts > 0 { failStarts -= 1; throw HubError.unavailable }
        callback = event
        didStart?()
    }
    func connect(_ pairing: HubPairing) throws { connects += 1; lastPairing = pairing }
    func stop() { callback = nil }
    func emit(_ event: HubLinkEvent) { callback?(event) }
}
