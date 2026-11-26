import Foundation
import XCTest
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif
@testable import SoftmaxCore

@MainActor
final class HubNativeConnectionTests: XCTestCase {
    func testReaderDescriptorSurvivesClosingOriginalPipeHandle() throws {
        let pipe = Pipe()
        let original = pipe.fileHandleForReading.fileDescriptor
        let descriptor = try ProcessHubLink.readerDescriptor(for: pipe.fileHandleForReading)
        defer { if descriptor != original { close(descriptor) } }
        // Model stop() closing its handle before the detached reader resumes.
        // No scheduler race or descriptor-reuse lottery is needed.
        let frame = Data("{\"state\":\"offline\"}\n".utf8)
        try pipe.fileHandleForWriting.write(contentsOf: frame)
        try pipe.fileHandleForWriting.close()
        try pipe.fileHandleForReading.close()
        var buffer = [UInt8](repeating: 0, count: 4096)
        let count = buffer.withUnsafeMutableBytes { read(descriptor, $0.baseAddress!, $0.count) }
        XCTAssertEqual(count, frame.count, "Reader must own a descriptor independent of stop()'s handle")
        if count > 0 { XCTAssertEqual(Data(buffer.prefix(count)), frame) }
    }

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
        let enrollment = Task { try await hub.enrollOnce(pairing) }
        let enrollmentWatchdog = Task {
            try await Task.sleep(for: .seconds(30))
            enrollment.cancel()
        }
        defer { enrollmentWatchdog.cancel() }
        try await enrollment.value
        enrollmentWatchdog.cancel()
        XCTAssertEqual(hub.state, .connected, "Awaited command requires actual native connection and persisted completion")
        XCTAssertEqual(try store.loadHub()?.hub, pairing.hub)
        XCTAssertNil(try store.loadHub()?.invite)
        // Exercise the same explicit reconnect command used by the native UI,
        // not only its historical fire-and-forget retry wrapper.
        let reconnect = Task { try await hub.reconnectOnce() }
        let reconnectWatchdog = Task {
            try await Task.sleep(for: .seconds(30))
            reconnect.cancel()
        }
        defer { reconnectWatchdog.cancel() }
        try await reconnect.value
        reconnectWatchdog.cancel()
        XCTAssertEqual(hub.state, .connected)
        states.removeAll()
        try link.disconnect()
        try await eventually { states.contains(.offline) }
        try await eventually { hub.state == .connected }
        XCTAssertEqual(try store.loadKey(), identity)
        XCTAssertNil(try store.loadHub()?.invite)
        // Reuse this exact ProcessHubLink instance: an old detached reader must
        // not consume bytes or deliver failure into its replacement process.
        hub.stop()
        XCTAssertEqual(hub.state, .offline)
        hub.start()
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
