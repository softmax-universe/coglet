import XCTest
import CogletTypes
import CogletSurface
import SoftmaxCore
import FaceMac
@testable import SoftmaxRuntime

private struct NativeRuntimeBoundaryFailure: Swift.Error {}

final class RuntimeHostTests: XCTestCase {
    @MainActor func testNativeEchoGuestTrapCaughtPanicAndRestart() async throws {
        let env = ProcessInfo.processInfo.environment
        let source = try XCTUnwrap(env["SOFTMAX_ECHO_INSTALL"])
        let trap = try XCTUnwrap(env["SOFTMAX_TRAP_INSTALL"])
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("runtime-host-" + UUID().uuidString)
        var returnFromPreservationFailure = false
        defer {
            if let path = env["SOFTMAX_RUNTIME_DIAGNOSTICS_DIR"] {
                let target = URL(fileURLWithPath: path).appendingPathComponent(root.lastPathComponent)
                do {
                    try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                    try FileManager.default.copyItem(at: root, to: target)
                } catch {
                    XCTFail("Runtime diagnostics copy failed; original retained at \(root.path): \(error)")
                    // Never delete the sole copy of first-failure evidence.
                    returnFromPreservationFailure = true
                }
            }
            if !returnFromPreservationFailure { try? FileManager.default.removeItem(at: root) }
        }
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
        func requireRunning(_ checked: RuntimeHost, boundary: String, file: StaticString = #filePath, line: UInt = #line) async throws {
            guard checked.state == .running else {
                #if COGLET_RUNTIME_TEST_HOOKS
                let trace = await checked.diagnosticsForTesting()
                try Data(trace.joined(separator: "\n").utf8).write(to: checked.directory.appendingPathComponent("pump-first-state-failure.log"))
                #endif
                XCTFail("First state failure at \(boundary): \(checked.errorMessage ?? "no error"); no further lifecycle actions", file: file, line: line)
                throw NativeRuntimeBoundaryFailure()
            }
        }
        // XCTest assertions normally continue; stop at the first failed receipt
        // before any subsequent poll, lifecycle action or destructive fixture step.
        func stopOnAssertionFailure(_ checked: RuntimeHost) async throws {
            guard (self.testRun?.totalFailureCount ?? 0) > 0 else { return }
            #if COGLET_RUNTIME_TEST_HOOKS
            let trace = await checked.diagnosticsForTesting()
            try Data(trace.joined(separator: "\n").utf8).write(to: checked.directory.appendingPathComponent("pump-first-assertion-failure.log"))
            #endif
            throw NativeRuntimeBoundaryFailure()
        }
        await host.start()
        try await requireRunning(host, boundary: "initial start")
        XCTAssertEqual(host.coglets.count, 1)
        try await stopOnAssertionFailure(host)
        XCTAssertEqual(host.coglets.first?.version, "0.1")
        try await stopOnAssertionFailure(host)
        let text = "Native Swift SDK → Wasm → speak — 世界"
        await host.submitTranscript(text, generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        try await requireRunning(host, boundary: "expected running")
        XCTAssertEqual(host.actions.first?.capability, "speech.speak_chunk")
        try await stopOnAssertionFailure(host)
        XCTAssertEqual(host.actions.first?.text, text)
        try await stopOnAssertionFailure(host)
        let firstAction = try XCTUnwrap(host.actions.first)
        func outcome(_ action: RuntimeAction) throws -> CBORValue? {
            let ledger = try CBORCodec.decodeProfile(Array(Data(contentsOf: good.appendingPathComponent("surface/ledger.cbor"))))
            guard case .array(let values) = ledger["records"]?[action.owner + "/" + action.id]?["outcome"] else { return nil }
            return values.first
        }
        XCTAssertEqual(device.callbacks.count, 1, "real Wasm output reaches FacePlaybackSession and FaceSpeechSink")
        try await stopOnAssertionFailure(host)
        await host.poll()
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertNil(try outcome(firstAction), "synthesized/scheduled PCM cannot complete the durable action")
        try await stopOnAssertionFailure(host)
        device.renderedFrames = 2; device.callbacks[0]()
        await host.poll()
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(try outcome(firstAction)?["status"], .text("succeeded"))
        try await stopOnAssertionFailure(host)
        let beforeBarge = host.actions.count
        await host.submitTranscript("Barge this reply", generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, beforeBarge + 1)
        try await stopOnAssertionFailure(host)
        let interrupted = try XCTUnwrap(host.actions.last)
        let late = try XCTUnwrap(device.callbacks.last)
        host.cancelOutput()
        XCTAssertFalse(speech.active)
        try await stopOnAssertionFailure(host)
        late(); host.complete(interrupted)
        await host.poll()
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(try outcome(interrupted)?["error"]?["code"], .text("cancelled"))
        try await stopOnAssertionFailure(host)
        let cancelledActionCount = host.actions.count, cancelledCallbackCount = device.callbacks.count
        host.beforeTranscriptHandoff = { host.cancelOutput() }
        await host.submitTranscript("Submitted before local Stop", generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        host.beforeTranscriptHandoff = nil
        await host.poll()
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, cancelledActionCount, "older submission must not reopen a newer suspension")
        try await stopOnAssertionFailure(host)
        XCTAssertEqual(device.callbacks.count, cancelledCallbackCount)
        try await stopOnAssertionFailure(host)
        host.cancelOutput()
        await host.poll()
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        // A rendered receipt is terminal even if lifecycle teardown beats the
        // polling task. Exercise the real ledger, not only the output mailbox.
        let stopActionCount = host.actions.count, stopCallbackCount = device.callbacks.count
        await host.submitTranscript("Rendered immediately before stop", generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, stopActionCount + 1)
        try await stopOnAssertionFailure(host)
        XCTAssertEqual(device.callbacks.count, stopCallbackCount + 1)
        try await stopOnAssertionFailure(host)
        let beforeStop = try XCTUnwrap(host.actions.last)
        XCTAssertEqual(beforeStop.text, "Rendered immediately before stop")
        try await stopOnAssertionFailure(host)
        XCTAssertNil(try outcome(beforeStop))
        try await stopOnAssertionFailure(host)
        device.renderedFrames = 2
        try XCTUnwrap(device.callbacks.last)()
        XCTAssertNil(try outcome(beforeStop), "native receipt has not been reconciled")
        try await stopOnAssertionFailure(host)
        await host.stop() // Deliberately no poll between native callback and stop.
        XCTAssertEqual(try outcome(beforeStop)?["status"], .text("succeeded"))
        try await stopOnAssertionFailure(host)
        await host.start()
        try await requireRunning(host, boundary: "expected running")
        let restartActionCount = host.actions.count, restartCallbackCount = device.callbacks.count
        await host.submitTranscript("Rendered immediately before restart", generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, restartActionCount + 1)
        try await stopOnAssertionFailure(host)
        XCTAssertEqual(device.callbacks.count, restartCallbackCount + 1)
        try await stopOnAssertionFailure(host)
        let beforeRestartReceipt = try XCTUnwrap(host.actions.last)
        XCTAssertEqual(beforeRestartReceipt.text, "Rendered immediately before restart")
        try await stopOnAssertionFailure(host)
        XCTAssertNil(try outcome(beforeRestartReceipt))
        try await stopOnAssertionFailure(host)
        device.renderedFrames = 2
        try XCTUnwrap(device.callbacks.last)()
        XCTAssertNil(try outcome(beforeRestartReceipt), "native receipt has not been reconciled")
        try await stopOnAssertionFailure(host)
        await host.restart() // Deliberately no poll before restarting either.

        try await requireRunning(host, boundary: "expected running")
        XCTAssertEqual(try outcome(beforeRestartReceipt)?["status"], .text("succeeded"))
        try await stopOnAssertionFailure(host)
        // No synthetic UI-success path: actual C ABI panic returns an error.
        #if COGLET_RUNTIME_TEST_HOOKS
        await host.injectCaughtPanicForTesting()
        XCTAssertEqual(host.state, .error)
        try await stopOnAssertionFailure(host)
        await host.restart()
        try await requireRunning(host, boundary: "expected running")
        await host.submitTranscript("After caught panic", generation: host.generation)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.last?.text, "After caught panic")
        try await stopOnAssertionFailure(host)
        #else
        XCTFail("Native acceptance must enable COGLET_RUNTIME_TEST_HOOKS")
        #endif
        await host.stop()
        XCTAssertEqual(host.state, .stopped)
        try await stopOnAssertionFailure(host)
        await host.start()
        try await requireRunning(host, boundary: "expected running")
        await host.stop()
        let oldGeneration = host.generation
        await host.start()
        try await requireRunning(host, boundary: "expected running")
        let beforeStale = host.actions.count
        await host.submitTranscript("Old recognition callback", generation: oldGeneration)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, beforeStale)
        try await stopOnAssertionFailure(host)
        let beforeRestart = host.generation
        await host.restart()
        try await requireRunning(host, boundary: "expected running")
        await host.submitTranscript("Pre-restart callback", generation: beforeRestart)
        try await requireRunning(host, boundary: "after poll or transcript handoff")
        XCTAssertEqual(host.actions.count, beforeStale)
        try await stopOnAssertionFailure(host)
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
        try await stopOnAssertionFailure(host)
        await host.restart()
        try await requireRunning(host, boundary: "expected running")
        await host.stop()
        // Loss of the entire ledger directory cannot mean a NEW identity.
        try FileManager.default.removeItem(at: good.appendingPathComponent("surface"))
        await host.start()
        XCTAssertEqual(host.state, .error)
        try await stopOnAssertionFailure(host)
        XCTAssertFalse(FileManager.default.fileExists(atPath: ledger.path))
        try await stopOnAssertionFailure(host)
        await host.stop()

        let bad = root.appendingPathComponent("trap")
        try provision(bad, from: trap)
        let faultHost = RuntimeHost(directory: bad)
        await faultHost.start()
        try await requireRunning(faultHost, boundary: "trap host expected running")
        await faultHost.submitTranscript("This executes unreachable in the guest", generation: faultHost.generation)
        XCTAssertEqual(faultHost.state, .error)
        try await stopOnAssertionFailure(faultHost)
        XCTAssertTrue(faultHost.actions.isEmpty)
        try await stopOnAssertionFailure(faultHost)
        await faultHost.restart()
        try await requireRunning(faultHost, boundary: "trap host expected running")
        XCTAssertTrue(faultHost.actions.isEmpty, "Restart must not replay a failed batch")
        try await stopOnAssertionFailure(faultHost)
        await faultHost.stop()
        XCTAssertTrue(FileManager.default.fileExists(atPath: bad.appendingPathComponent("state.wal").path))
        try await stopOnAssertionFailure(faultHost)
        XCTAssertTrue(FileManager.default.fileExists(atPath: bad.appendingPathComponent("logs/runtime.log").path))
        try await stopOnAssertionFailure(faultHost)
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
