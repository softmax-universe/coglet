import XCTest
@testable import SoftmaxRuntime

final class RuntimeHostTests: XCTestCase {
    @MainActor func testNativeEchoGuestTrapCaughtPanicAndRestart() async throws {
        let env = ProcessInfo.processInfo.environment
        let source = try XCTUnwrap(env["SOFTMAX_ECHO_INSTALL"])
        let trap = try XCTUnwrap(env["SOFTMAX_TRAP_INSTALL"])
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("runtime-host-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        func provision(_ directory: URL, from source: String) throws {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
            try FileManager.default.copyItem(at: URL(fileURLWithPath: source).appendingPathComponent("binding.json"), to: directory.appendingPathComponent("binding.json"))
            let data = try Data(contentsOf: URL(fileURLWithPath: source).appendingPathComponent("runtime.json"))
            var config = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            config["wal"] = directory.appendingPathComponent("state.wal").path
            try JSONSerialization.data(withJSONObject: config, options: [.sortedKeys]).write(to: directory.appendingPathComponent("runtime.json"))
        }
        let good = root.appendingPathComponent("good")
        try provision(good, from: source)
        let host = RuntimeHost(directory: good)
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(host.coglets.count, 1)
        XCTAssertEqual(host.coglets.first?.version, "0.1")
        let text = "Native Swift SDK → Wasm → speak — 世界"
        await host.submitTranscript(text)
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(host.actions.first?.capability, "speech.speak_chunk")
        XCTAssertEqual(host.actions.first?.text, text)
        // No synthetic UI-success path: actual C ABI panic returns an error.
        #if COGLET_RUNTIME_TEST_HOOKS
        await host.injectCaughtPanicForTesting()
        XCTAssertEqual(host.state, .error)
        await host.restart()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.submitTranscript("After caught panic")
        XCTAssertEqual(host.actions.last?.text, "After caught panic")
        #else
        XCTFail("Native acceptance must enable COGLET_RUNTIME_TEST_HOOKS")
        #endif
        await host.stop()
        XCTAssertEqual(host.state, .stopped)
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.stop()

        let bad = root.appendingPathComponent("trap")
        try provision(bad, from: trap)
        let faultHost = RuntimeHost(directory: bad)
        await faultHost.start()
        XCTAssertEqual(faultHost.state, .running, faultHost.errorMessage ?? "")
        await faultHost.submitTranscript("This executes unreachable in the guest")
        XCTAssertEqual(faultHost.state, .error)
        XCTAssertTrue(faultHost.actions.isEmpty)
        await faultHost.restart()
        XCTAssertEqual(faultHost.state, .running, faultHost.errorMessage ?? "")
        XCTAssertTrue(faultHost.actions.isEmpty, "Restart must not replay a failed batch")
        await faultHost.stop()
        XCTAssertTrue(FileManager.default.fileExists(atPath: bad.appendingPathComponent("state.wal").path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: bad.appendingPathComponent("logs/runtime.log").path))
    }

    @MainActor func testMissingInstallIsErrorAndStopIsSafe() async {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("runtime-missing-" + UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let host = RuntimeHost(directory: root)
        await host.start()
        XCTAssertEqual(host.state, .error)
        XCTAssertNotNil(host.errorMessage)
        await host.stop()
        XCTAssertEqual(host.state, .stopped)
    }
}
