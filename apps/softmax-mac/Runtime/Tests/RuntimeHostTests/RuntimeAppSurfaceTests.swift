import Foundation
import XCTest
import CogletSurface
import CogletTypes
import class SoftmaxAppSurface.ControlRegistry
import class SoftmaxAppSurface.AppSurfaceAdapter
import enum SoftmaxAppSurface.CBORValue
import struct SoftmaxAppSurface.CBORMapEntry
@testable import SoftmaxRuntime

/// Real C ABI/WAMR and production RuntimeHost with a MainActor command fixture.
/// NOT the app's live AppDelegate/SwiftUI tree or a coglet-mock acceptance proof.
final class RuntimeAppSurfaceTests: XCTestCase {
    @MainActor func testProductionHostCompletesPinnedFocusOnlyAfterNativeWitness() async throws {
        typealias N = SoftmaxAppSurface.CBORValue
        func object(_ fields: [String: N]) -> N { .map(fields.map { SoftmaxAppSurface.CBORMapEntry($0.key, $0.value) }) }
        let source = try XCTUnwrap(ProcessInfo.processInfo.environment["SOFTMAX_ECHO_INSTALL"])
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("app-host-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("surface"), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        // Teardown blocks run in reverse order: stop the global runtime before
        // removing its files, even when an assertion unwrap or ledger read throws.
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        try Data(CogletTypes.CBORCodec.encodeProfile(.map([]))).write(to: root.appendingPathComponent("surface/ledger.cbor"))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: root.appendingPathComponent("surface/ledger.cbor").path)
        let installed = URL(fileURLWithPath: source)
        try FileManager.default.copyItem(at: installed.appendingPathComponent("binding.json"), to: root.appendingPathComponent("binding.json"))
        var configuration = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: installed.appendingPathComponent("runtime.json"))) as? [String: Any])
        var setup = try XCTUnwrap(configuration["setup"] as? [String: Any])
        var manifest = try XCTUnwrap(setup["manifest"] as? [String: Any])
        let owner = try XCTUnwrap(manifest["id"] as? String)
        let actionID = String(repeating: "e", count: 32)
        manifest["required_capabilities"] = (manifest["required_capabilities"] as? [String] ?? []) + ["softmax.app.state", "softmax.app.set_focus"]
        manifest["exclusive_outputs"] = (manifest["exclusive_outputs"] as? [String] ?? []) + ["softmax.app_control"]
        setup["manifest"] = manifest; setup["contract_profile"] = "surface-0.2"
        setup["initial_effect_ids"] = ["actions": [actionID], "timers": [], "llm_requests": []]
        setup["initial_actions"] = [["id": actionID, "coglet": owner, "capability": "softmax.app.set_focus",
            "args": ["window": "settings", "control": "settings.general.search"],
            "deadline": 60_000_000, "correlation_id": actionID]]
        let product = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Surface")
        configuration["trusted_extension_catalog"] = ["json": try String(contentsOf: product.appendingPathComponent("catalog.v1.json"), encoding: .utf8),
            "sha256": try String(contentsOf: product.appendingPathComponent("catalog.v1.sha256"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)]
        configuration["setup"] = setup; configuration["wal"] = root.appendingPathComponent("state.wal").path
        try JSONSerialization.data(withJSONObject: configuration, options: [.sortedKeys]).write(to: root.appendingPathComponent("runtime.json"))
        var focusRequests = 0
        var focused = false
        let userFocus = { focusRequests += 1; return true }
        let entered = XCTestExpectation(description: "native focus settling entered")
        let finished = XCTestExpectation(description: "actual native focus witness settled")
        var resume: CheckedContinuation<Void, Never>?
        let registry = ControlRegistry(revision: 1, currentRoute: { $0 == "settings" ? "general" : nil })
        // The injected policy must obey the production two-second maximum.
        // Freeze only the local settling clock: the explicit continuation, not
        // runner scheduling, controls when the native witness becomes true.
        let instant = ContinuousClock.now
        registry.focusSettling = .init(timeout: .seconds(2), interval: .milliseconds(10),
            now: { instant }, sleep: { _ in
                await withCheckedContinuation { continuation in resume = continuation; entered.fulfill() }
                try Task.checkCancellation()
                focused = true
                finished.fulfill()
            })
        _ = try registry.mount(id: "settings.general.search", window: "settings", route: "general", role: "text_field", label: "Search",
            verbs: ["type_text"], value: { object(["kind": .text("text"), "value": .text("")]) },
            focus: userFocus, focused: { focused }, focusSettles: true,
            invoke: { _ in XCTFail("focus must not invoke a control") })
        let adapter = AppSurfaceAdapter(registry: registry, captureState: {
            let meter = object(["state": .text("idle"), "visible": .boolean(false), "elapsed_ms": .unsigned(0), "level": .unsigned(0)])
            return object(["revision": .unsigned(registry.revision),
                "windows": .array([object(["window": .text("settings"), "route": .text("general"), "open": .boolean(true)])]), "focus": .null,
                "settings": object(["appearance": .text("System"), "text_size": .text("standard"), "floating_orb": .boolean(false),
                    "talk_mode": .text("hold"), "sound_cues": .boolean(false), "launch_at_login": .text("disabled"),
                    "hotkey": object(["key_code": .unsigned(61), "modifiers": .array([])])]),
                "ptt": meter, "hud": meter, "hub": .text("offline"),
                "face": object(["visible": .boolean(false), "mode": .text("full"), "expression": .text("neutral"), "status": .text("idle")])])
        }, currentRoute: { $0 == "settings" ? "general" : nil }, navigate: { _, _ in }, open: { _ in }, close: { _ in })
        let host = RuntimeHost(directory: root)
        addTeardownBlock { @MainActor in
            await host.stop()
            // A checked continuation does not wake on task cancellation. Release
            // a suspended fixture after stop has fenced its native command.
            resume?.resume()
            resume = nil
            XCTAssertEqual(host.state, .stopped, host.errorMessage ?? "")
            XCTAssertNil(registry.onChange)
        }
        host.automaticallyPolls = false
        host.installAppSurface(adapter, refresh: { try registry.nativeStateChanged() }, events: { registry.onChange = $0 })
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(focusRequests, 0, "SDK acceptance cannot fake a native effect")
        await host.poll()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        await fulfillment(of: [entered], timeout: 2)
        XCTAssertEqual(focusRequests, 1, "request uses the shared native focus path once")
        XCTAssertFalse(focused, "Suspension must not manufacture terminal success")
        let pendingLedger = try CogletTypes.CBORCodec.decodeProfile(Array(Data(contentsOf: root.appendingPathComponent("surface/ledger.cbor"))))
        XCTAssertEqual(pendingLedger["records"]?[owner + "/" + actionID]?["outcome"], .array([]))
        let continuation = try XCTUnwrap(resume)
        resume = nil // teardown must not resume this continuation twice
        continuation.resume()
        await fulfillment(of: [finished], timeout: 2)
        await host.poll()
        XCTAssertEqual(focusRequests, 1)
        XCTAssertTrue(focused)
        XCTAssertTrue(host.actions.isEmpty, "app actions must never enter RuntimeAction speech decoding")
        let ledger = try CogletTypes.CBORCodec.decodeProfile(Array(Data(contentsOf: root.appendingPathComponent("surface/ledger.cbor"))))
        guard case .array(let outcome)? = ledger["records"]?[owner + "/" + actionID]?["outcome"] else {
            await host.stop(); return XCTFail("missing real durable app outcome")
        }
        XCTAssertEqual(outcome.first?["status"], .text("succeeded"))
        await host.stop()
        XCTAssertNil(registry.onChange)
    }
}
