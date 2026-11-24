import Foundation
import XCTest
@testable import SoftmaxCore

@MainActor
final class HubNativeConnectionTests: XCTestCase {
    func testHubConnectionTestsEnrollDropOfflineAndRecoverAgainstCogletMock() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let helper = env["SOFTMAX_HUB_HELPER"], let launcher = env["SOFTMAX_MOCK_HUB"] else {
            if env["SOFTMAX_REQUIRE_HUB_TESTS"] == "1" { XCTFail("Native hub executables are required"); return }
            throw XCTSkip("Dedicated native hub lane supplies real Rust executables")
        }
        let store = MemoryHubStore()
        let link = ProcessHubLink(executable: URL(fileURLWithPath: helper))
        let hub = HubConnection(account: HubAccount(store: store), link: link)
        var states: [HubConnectionState] = []
        hub.changed = { states.append(hub.state) }
        hub.start()
        defer { hub.signOut() }
        try await eventually { hub.deviceID != nil }
        let identity = try store.loadKey()
        let mock = Process(), input = Pipe(), output = Pipe()
        mock.executableURL = URL(fileURLWithPath: launcher)
        mock.arguments = [try XCTUnwrap(hub.deviceID)] // public ID only
        mock.standardInput = input; mock.standardOutput = output
        mock.standardError = FileHandle.nullDevice
        try mock.run()
        defer {
            try? input.fileHandleForWriting.close()
            if mock.isRunning { mock.terminate() }
            try? output.fileHandleForReading.close()
        }
        // Bounded asynchronous read, never print the bootstrap's private invitation.
        let reader = Task.detached { () throws -> Data in
            var data = Data()
            while data.count < 16384 {
                guard let byte = try output.fileHandleForReading.read(upToCount: 1), !byte.isEmpty else { throw HubError.unavailable }
                if byte == Data([10]) { return data }
                data.append(byte)
            }
            throw HubError.pairing
        }
        let timeout = Task {
            try await Task.sleep(for: .seconds(20))
            if mock.isRunning { mock.terminate() }
        }
        defer { timeout.cancel(); reader.cancel() }
        let bootstrap = try await reader.value
        var envelope = try XCTUnwrap(JSONSerialization.jsonObject(with: bootstrap) as? [String: Any])
        envelope["name"] = "Native mock hub"
        let pairing = try HubPairing.parse(String(decoding: JSONSerialization.data(withJSONObject: envelope), as: UTF8.self))
        try hub.enroll(pairing)
        try await eventually { hub.state == .connected }
        XCTAssertNil(try store.loadHub()?.invite)
        states.removeAll()
        try link.disconnect()
        try await eventually { states.contains(.offline) }
        try await eventually { hub.state == .connected }
        XCTAssertEqual(try store.loadKey(), identity)
        XCTAssertNil(try store.loadHub()?.invite)
        hub.signOut()
        XCTAssertNil(try store.loadKey())
        XCTAssertNil(try store.loadHub())
        XCTAssertEqual(hub.state, .offline)
    }
    private func eventually(_ condition: () -> Bool) async throws {
        let deadline = ContinuousClock.now + .seconds(30)
        while !condition(), ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(condition(), "Timed out waiting for real mock-hub state")
        if !condition() { throw HubError.unavailable }
    }
}
