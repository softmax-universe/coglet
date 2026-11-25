import XCTest
import CogletTypes
import CogletSurface
import SoftmaxCore
import FaceMac
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
            try FileManager.default.createDirectory(at: directory.appendingPathComponent("surface"), withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
            try Data(CBORCodec.encodeProfile(.map([]))).write(to: directory.appendingPathComponent("surface/ledger.cbor"))
            try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: directory.appendingPathComponent("surface/ledger.cbor").path)
            try FileManager.default.copyItem(at: URL(fileURLWithPath: source).appendingPathComponent("binding.json"), to: directory.appendingPathComponent("binding.json"))
            let data = try Data(contentsOf: URL(fileURLWithPath: source).appendingPathComponent("runtime.json"))
            var config = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
            config["wal"] = directory.appendingPathComponent("state.wal").path
            try JSONSerialization.data(withJSONObject: config, options: [.sortedKeys]).write(to: directory.appendingPathComponent("runtime.json"))
        }
        let good = root.appendingPathComponent("good")
        try provision(good, from: source)
        let host = RuntimeHost(directory: good)
        host.automaticallyPolls = false
        let device = RuntimeNativePlaybackDevice()
        let speech = FaceSpeechSink(device: device)
        let ptt = PushToTalk(keys: RuntimeNativeKeys(), audio: RuntimeNativeCapture())
        let player = RuntimeSpeechPlayback(session: FacePlaybackSession(ptt: ptt, speech: speech), synthesis: RuntimeNativeSynthesis())
        host.onStopOutput = { player.stop() }
        host.onAction = { action in
            guard let text = action.text, let stream = action.stream, let chunk = action.chunk else {
                host.complete(action, error: "unavailable"); return
            }
            player.enqueue(text: text, stream: stream, chunk: chunk, final: action.final, deadline: action.deadline) { host.complete(action, error: $0) }
        }
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(host.coglets.count, 1)
        XCTAssertEqual(host.coglets.first?.version, "0.1")
        let text = "Native Swift SDK → Wasm → speak — 世界"
        await host.submitTranscript(text, generation: host.generation)
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(host.actions.first?.capability, "speech.speak_chunk")
        XCTAssertEqual(host.actions.first?.text, text)
        let firstAction = try XCTUnwrap(host.actions.first)
        func outcome(_ action: RuntimeAction) throws -> CBORValue? {
            let ledger = try CBORCodec.decodeProfile(Array(Data(contentsOf: good.appendingPathComponent("surface/ledger.cbor"))))
            guard case .array(let values) = ledger["records"]?[action.owner + "/" + action.id]?["outcome"] else { return nil }
            return values.first
        }
        XCTAssertEqual(device.callbacks.count, 1, "real Wasm output reaches FacePlaybackSession and FaceSpeechSink")
        await host.poll()
        XCTAssertNil(try outcome(firstAction), "synthesized/scheduled PCM cannot complete the durable action")
        device.renderedFrames = 2; device.callbacks[0]()
        await host.poll()
        XCTAssertEqual(try outcome(firstAction)?["status"], .text("succeeded"))
        await host.submitTranscript("Barge this reply", generation: host.generation)
        let interrupted = try XCTUnwrap(host.actions.last)
        let late = try XCTUnwrap(device.callbacks.last)
        host.cancelOutput()
        XCTAssertFalse(speech.active)
        late(); host.complete(interrupted)
        await host.poll()
        XCTAssertEqual(try outcome(interrupted)?["error"]?["code"], .text("cancelled"))
        // A rendered receipt is terminal even if lifecycle teardown beats the
        // polling task. Exercise the real ledger, not only the output mailbox.
        let stopActionCount = host.actions.count, stopCallbackCount = device.callbacks.count
        await host.submitTranscript("Rendered immediately before stop", generation: host.generation)
        XCTAssertEqual(host.actions.count, stopActionCount + 1)
        XCTAssertEqual(device.callbacks.count, stopCallbackCount + 1)
        let beforeStop = try XCTUnwrap(host.actions.last)
        XCTAssertEqual(beforeStop.text, "Rendered immediately before stop")
        XCTAssertNil(try outcome(beforeStop))
        device.renderedFrames = 2
        try XCTUnwrap(device.callbacks.last)()
        XCTAssertNil(try outcome(beforeStop), "native receipt has not been reconciled")
        await host.stop() // Deliberately no poll between native callback and stop.
        XCTAssertEqual(try outcome(beforeStop)?["status"], .text("succeeded"))
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        let restartActionCount = host.actions.count, restartCallbackCount = device.callbacks.count
        await host.submitTranscript("Rendered immediately before restart", generation: host.generation)
        XCTAssertEqual(host.actions.count, restartActionCount + 1)
        XCTAssertEqual(device.callbacks.count, restartCallbackCount + 1)
        let beforeRestartReceipt = try XCTUnwrap(host.actions.last)
        XCTAssertEqual(beforeRestartReceipt.text, "Rendered immediately before restart")
        XCTAssertNil(try outcome(beforeRestartReceipt))
        device.renderedFrames = 2
        try XCTUnwrap(device.callbacks.last)()
        XCTAssertNil(try outcome(beforeRestartReceipt), "native receipt has not been reconciled")
        await host.restart() // Deliberately no poll before restarting either.
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(try outcome(beforeRestartReceipt)?["status"], .text("succeeded"))
        // No synthetic UI-success path: actual C ABI panic returns an error.
        #if COGLET_RUNTIME_TEST_HOOKS
        await host.injectCaughtPanicForTesting()
        XCTAssertEqual(host.state, .error)
        await host.restart()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.submitTranscript("After caught panic", generation: host.generation)
        XCTAssertEqual(host.actions.last?.text, "After caught panic")
        #else
        XCTFail("Native acceptance must enable COGLET_RUNTIME_TEST_HOOKS")
        #endif
        await host.stop()
        XCTAssertEqual(host.state, .stopped)
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.stop()
        let oldGeneration = host.generation
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        let beforeStale = host.actions.count
        await host.submitTranscript("Old recognition callback", generation: oldGeneration)
        XCTAssertEqual(host.actions.count, beforeStale)
        let beforeRestart = host.generation
        await host.restart()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.submitTranscript("Pre-restart callback", generation: beforeRestart)
        XCTAssertEqual(host.actions.count, beforeStale)
        await host.stop()

        // Simulate the exact durable running marker left by a process crash.
        let ledger = good.appendingPathComponent("surface/ledger.cbor")
        let saved = try Data(contentsOf: ledger)
        guard case .map(var entries) = try CBORCodec.decodeProfile(Array(saved)),
              let running = entries.firstIndex(where: { $0.key == "running" }) else {
            return XCTFail("Missing real SDK running marker")
        }
        entries[running].value = .boolean(true)
        try Data(CBORCodec.encodeProfile(.map(entries))).write(to: ledger)
        await host.start()
        XCTAssertEqual(host.state, .error, "Unclean restart must fail closed")
        await host.restart()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await host.stop()
        // Loss of the entire ledger directory cannot mean a NEW identity.
        try FileManager.default.removeItem(at: good.appendingPathComponent("surface"))
        await host.start()
        XCTAssertEqual(host.state, .error)
        XCTAssertFalse(FileManager.default.fileExists(atPath: ledger.path))
        await host.stop()

        let bad = root.appendingPathComponent("trap")
        try provision(bad, from: trap)
        let faultHost = RuntimeHost(directory: bad)
        await faultHost.start()
        XCTAssertEqual(faultHost.state, .running, faultHost.errorMessage ?? "")
        await faultHost.submitTranscript("This executes unreachable in the guest", generation: faultHost.generation)
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
