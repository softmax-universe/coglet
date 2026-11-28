#if DEBUG
import Foundation
import XCTest
@testable import SoftmaxCore

@MainActor
final class DevelopmentHubStoreTests: XCTestCase {
    private func isolated(_ test: (URL) throws -> Void) throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try test(directory)
    }
    func testPersistsAcrossStoreInstancesAndRefusesOverwrite() throws {
        try isolated { directory in
            let store = DevelopmentHubStore(directory: directory)
            let key = try HubAccount(store: store).identity()
            XCTAssertEqual(try HubAccount(store: DevelopmentHubStore(directory: directory)).identity(), key)
            XCTAssertThrowsError(try store.saveKey(Data(repeating: 3, count: 32)))
            XCTAssertEqual(try store.loadKey(), key)
            let permissions = try FileManager.default.attributesOfItem(atPath: directory.path)
            XCTAssertEqual((permissions[.posixPermissions] as? NSNumber)?.intValue, 0o700)
            let file = try FileManager.default.attributesOfItem(atPath: directory.appendingPathComponent("identity").path)
            XCTAssertEqual((file[.posixPermissions] as? NSNumber)?.intValue, 0o600)
        }
    }
    func testEnrollmentOmitsInviteAndMissingOrCorruptIdentityFailsClosed() throws {
        try isolated { directory in
            let store = DevelopmentHubStore(directory: directory)
            _ = try HubAccount(store: store).identity()
            let pairing = try HubPairing.parse(#"{"name":"Test","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":[],"invite":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}"#)
            try store.saveHub(pairing)
            XCTAssertNil(try store.loadHub()?.invite)
            XCTAssertFalse(try String(contentsOf: directory.appendingPathComponent("enrollment.json"), encoding: .utf8).contains("invite"))
            try store.deleteKey()
            XCTAssertThrowsError(try HubAccount(store: store).identity())
            try store.saveKey(Data([1]))
            XCTAssertThrowsError(try HubAccount(store: store).identity())
            try HubAccount(store: store).signOut()
            XCTAssertNil(try store.loadKey())
            XCTAssertNil(try store.loadHub())
        }
    }
    func testEnrollmentReplacementIsAtomicAndKeepsOldOpenFileIntact() throws {
        try isolated { directory in
            let store = DevelopmentHubStore(directory: directory)
            let pairing = try HubPairing.parse(#"{"name":"Old","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":[]}"#)
            try store.saveHub(pairing)
            let url = directory.appendingPathComponent("enrollment.json")
            let old = try FileHandle(forReadingFrom: url)
            defer { try? old.close() }
            let expected = try Data(contentsOf: url)
            let replacement = try HubPairing.parse(#"{"name":"New","hub":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","addresses":[]}"#)
            try store.saveHub(replacement)
            XCTAssertEqual(try old.readToEnd(), expected)
            XCTAssertEqual(try store.loadHub()?.name, "New")
            XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: directory.path), ["enrollment.json"])
        }
    }
    func testInsecureDirectoryFileAndSymlinksFailClosed() throws {
        try isolated { directory in
            let store = DevelopmentHubStore(directory: directory)
            _ = try HubAccount(store: store).identity()
            try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: directory.path)
            XCTAssertThrowsError(try store.loadKey())
            try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: directory.path)
            let identity = directory.appendingPathComponent("identity")
            try FileManager.default.setAttributes([.posixPermissions: 0o644], ofItemAtPath: identity.path)
            XCTAssertThrowsError(try store.loadKey())
            try FileManager.default.removeItem(at: identity)
            try FileManager.default.createSymbolicLink(atPath: identity.path, withDestinationPath: "/dev/null")
            XCTAssertThrowsError(try store.loadKey())
            XCTAssertThrowsError(try store.saveKey(Data(repeating: 1, count: 32)))
            let link = directory.appendingPathExtension("link")
            defer { try? FileManager.default.removeItem(at: link) }
            try FileManager.default.createSymbolicLink(at: link, withDestinationURL: directory)
            XCTAssertThrowsError(try DevelopmentHubStore(directory: link).loadKey())
        }
    }
}
#endif
