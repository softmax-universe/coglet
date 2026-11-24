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
        timeout.cancel()
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
    func testProcessPublishesReadyBeforeItsOutputPipeCloses() async throws {
        let env = ProcessInfo.processInfo.environment
        guard let helper = env["SOFTMAX_HUB_HELPER"] else {
            if env["SOFTMAX_REQUIRE_HUB_TESTS"] == "1" { XCTFail("Native helper is required"); return }
            throw XCTSkip("Dedicated native lane supplies the real helper")
        }
        let link = ProcessHubLink(executable: URL(fileURLWithPath: helper))
        var device: String?
        try link.start(key: Data(repeating: 7, count: 32)) {
            if case .ready(let value) = $0 { device = value }
        }
        defer { link.stop() }
        try await eventually { device != nil }
        XCTAssertTrue(HubPairing.isHex(try XCTUnwrap(device)))
    }

    /// A live subprocess may close its command channel before terminating. The
    /// app must report an unavailable ROUTER, not receive a fatal SIGPIPE.
    func testClosedHelperInputThrowsWithoutTerminatingClient() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let executable = directory.appendingPathComponent("closed-input-helper")
        let device = String(repeating: "a", count: 64)
        // Read the initial key without recording it, close stdin, then publish
        // readiness. This orders the closed-pipe write without a timing guess.
        let script = "#!/bin/sh\nIFS= read -r key\nunset key\nexec 0<&-\nprintf '%s\\n' '{\"state\":\"ready\",\"device\":\"\(device)\"}'\nexec /bin/sleep 20\n"
        try Data(script.utf8).write(to: executable)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: executable.path)
        let link = ProcessHubLink(executable: executable)
        var ready = false
        try link.start(key: Data(repeating: 7, count: 32)) {
            if case .ready = $0 { ready = true }
        }
        defer { link.stop() }
        try await eventually { ready }
        XCTAssertThrowsError(try link.disconnect()) { error in
            XCTAssertTrue(error is HubError)
        }
    }

    private func eventually(_ condition: () -> Bool, file: StaticString = #filePath, line: UInt = #line) async throws {
        let deadline = ContinuousClock.now + .seconds(30)
        while !condition(), ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(20)) }
        XCTAssertTrue(condition(), "Timed out waiting for real mock-ROUTER state", file: file, line: line)
        if !condition() { throw HubError.unavailable }
    }
}
