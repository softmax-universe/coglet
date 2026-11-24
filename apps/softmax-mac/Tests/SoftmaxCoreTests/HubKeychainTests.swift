import Foundation
import XCTest
@testable import SoftmaxCore

@MainActor
final class HubKeychainTests: XCTestCase {
    func testDeviceOnlyKeychainPolicyAndSignOutWithMemoryBackend() throws {
        let backend = MemoryKeychain()
        let suite = "HubKeychainTests." + UUID().uuidString
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = KeychainHubStore(service: suite, defaults: defaults, keychain: backend)
        let account = HubAccount(store: store, generate: { Data(repeating: 42, count: 32) })
        let key = try account.identity()
        XCTAssertEqual(backend.policy, .afterFirstUnlockThisDeviceOnly)
        XCTAssertEqual(try HubAccount(store: store).identity(), key)
        let pairing = try HubPairing.parse(#"{"name":"Hub","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":[],"invite":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
        try store.saveHub(pairing)
        XCTAssertNil(try store.loadHub()?.invite)
        XCTAssertFalse(defaults.data(forKey: suite + ".configuration").map { String(decoding: $0, as: UTF8.self).contains(String(repeating: "b", count: 64)) } ?? true)
        try account.signOut()
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
    }
    func testKeychainFailureDoesNotGenerateOrOverwrite() throws {
        let backend = MemoryKeychain(); backend.fail = true
        let store = KeychainHubStore(service: UUID().uuidString, keychain: backend)
        let account = HubAccount(store: store, generate: { XCTFail("must not rotate"); return Data() })
        XCTAssertThrowsError(try account.identity())
        XCTAssertEqual(backend.adds, 0)
    }
}

@MainActor
private final class MemoryKeychain: HubKeychain {
    var value: Data?
    var policy: HubKeyAccessibility?
    var fail = false
    var adds = 0
    func read(service: String) throws -> Data? { if fail { throw HubError.storage }; return value }
    func add(_ data: Data, service: String, accessibility: HubKeyAccessibility) throws {
        if fail || value != nil { throw HubError.storage }
        adds += 1; value = data; policy = accessibility
    }
    func delete(service: String) throws { if fail { throw HubError.storage }; value = nil }
}
