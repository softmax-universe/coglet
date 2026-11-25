import XCTest
@testable import SoftmaxCore

@MainActor
final class HubEnrollmentCommandTests: XCTestCase {
    private func invitation(device: String? = nil) throws -> HubPairing {
        let deviceField = device.map { ",\"device\":\"\($0)\"" } ?? ""
        return try HubPairing.parse("{\"name\":\"Router\",\"hub\":\"\(String(repeating: "b", count: 64))\",\"addresses\":[\"127.0.0.1:1234\"],\"invite\":\"\(String(repeating: "c", count: 64))\"\(deviceField)}")
    }
    private func ready() -> (HubConnection, MemoryHubStore, FakeHubLink) {
        let store = MemoryHubStore(), link = FakeHubLink()
        store.key = Data(repeating: 8, count: 32)
        let hub = HubConnection(account: HubAccount(store: store), link: link)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        return (hub, store, link)
    }
    func testSignOutCommandPropagatesStorageFailureAndCanRetry() throws {
        let (hub, store, link) = ready()
        let stale = link.callback
        store.fail = true
        XCTAssertThrowsError(try hub.signOutOnce())
        XCTAssertTrue(hub.canSignOut)
        XCTAssertEqual(hub.state, .offline)
        stale?(.connected)
        store.fail = false
        try hub.signOutOnce()
        XCTAssertFalse(hub.canSignOut)
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
    }
    func testEnrollmentWaitsForFreshConnectionAndDurableSave() async throws {
        let (hub, store, link) = ready()
        defer { hub.stop() }
        let target = try invitation(device: String(repeating: "a", count: 64))
        let stale = link.callback
        let started = expectation(description: "fresh enrollment channel")
        link.didStart = { started.fulfill() }
        var done = false
        let task = Task { try await hub.enrollOnce(target); done = true }
        await fulfillment(of: [started], timeout: 2)
        link.didStart = nil
        XCTAssertFalse(done); XCTAssertNil(try store.loadHub())
        stale?(.connected)
        XCTAssertNil(try store.loadHub())
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(link.lastPairing?.invite, target.invite)
        XCTAssertFalse(done, "Queued enrollment is not completed enrollment")
        link.emit(.connected)
        try await task.value
        XCTAssertTrue(done); XCTAssertEqual(hub.state, .connected)
        XCTAssertEqual(try store.loadHub()?.hub, target.hub)
        XCTAssertNil(try store.loadHub()?.invite)
        XCTAssertNil(hub.pairing?.invite)
        link.emit(.offline)
        XCTAssertEqual(hub.state, .offline, "Successful command retains normal live callbacks")
    }
    func testEnrollmentAuthorityLossRefusesDeferredConnectAndOldCallback() async throws {
        let (hub, store, link) = ready()
        defer { hub.stop() }
        let started = expectation(description: "fresh enrollment channel")
        link.didStart = { started.fulfill() }
        var authorized = true
        let target = try invitation()
        let task = Task { try await hub.enrollOnce(target) { guard authorized else { throw HubError.unavailable } } }
        await fulfillment(of: [started], timeout: 2)
        link.didStart = nil
        let stale = link.callback
        authorized = false
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        do { try await task.value; XCTFail("revoked enrollment succeeded") } catch {}
        XCTAssertEqual(link.connects, 0); XCTAssertEqual(hub.state, .offline)
        stale?(.connected)
        XCTAssertNil(try store.loadHub())
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        hub.retryNow()
        XCTAssertEqual(link.connects, 0, "Revoked invitation must not become independent recovery work")
    }
    func testEnrollmentSaveFailureIsNotSuccessAndDoesNotRetainInvitation() async throws {
        let (hub, store, link) = ready()
        defer { hub.stop() }
        let started = expectation(description: "fresh enrollment channel")
        link.didStart = { started.fulfill() }
        let target = try invitation()
        let task = Task { try await hub.enrollOnce(target) }
        await fulfillment(of: [started], timeout: 2)
        link.didStart = nil
        let stale = link.callback
        link.emit(.ready(device: String(repeating: "a", count: 64)))
        store.failHubSave = true
        link.emit(.connected)
        do { try await task.value; XCTFail("failed save succeeded") } catch {}
        store.failHubSave = false
        stale?(.connected)
        XCTAssertNil(try store.loadHub()); XCTAssertNil(hub.pairing)
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        XCTAssertEqual(link.connects, 1, "Failed invitation cannot retry under retired command authority")
    }
    func testEnrollmentCancellationAndReplacementHaveSeparateCustody() async throws {
        let (hub, store, link) = ready()
        defer { hub.stop() }
        let target = try invitation()
        let started = expectation(description: "first enrollment")
        link.didStart = { started.fulfill() }
        let first = Task { try await hub.enrollOnce(target) }
        await fulfillment(of: [started], timeout: 2)
        link.didStart = nil
        let stale = link.callback
        first.cancel()
        do { try await first.value; XCTFail("cancelled enrollment succeeded") } catch {}
        hub.start(); link.emit(.ready(device: String(repeating: "a", count: 64)))
        let successorStarted = expectation(description: "successor enrollment")
        link.didStart = { successorStarted.fulfill() }
        let successor = Task { try await hub.enrollOnce(target) }
        await fulfillment(of: [successorStarted], timeout: 2)
        stale?(.connected); stale?(.failed)
        XCTAssertNil(try store.loadHub()); XCTAssertEqual(hub.state, .connecting)
        link.emit(.ready(device: String(repeating: "a", count: 64))); link.emit(.connected)
        try await successor.value
        XCTAssertEqual(hub.state, .connected)
    }
    func testEnrollmentRechecksDeviceOnFreshHelper() async throws {
        let (hub, store, link) = ready()
        defer { hub.stop() }
        let started = expectation(description: "fresh enrollment channel")
        link.didStart = { started.fulfill() }
        let target = try invitation(device: String(repeating: "a", count: 64))
        let task = Task { try await hub.enrollOnce(target) }
        await fulfillment(of: [started], timeout: 2)
        link.didStart = nil
        link.emit(.ready(device: String(repeating: "d", count: 64)))
        do { try await task.value; XCTFail("wrong device enrollment succeeded") } catch {}
        XCTAssertEqual(link.connects, 0); XCTAssertNil(try store.loadHub())
    }
}
