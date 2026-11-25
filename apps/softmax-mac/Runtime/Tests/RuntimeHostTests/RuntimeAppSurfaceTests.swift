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
    @MainActor func testProductionHostDispatchesPinnedAppActionWithoutSpeechDecoding() async throws {
        typealias N = SoftmaxAppSurface.CBORValue
        func object(_ fields: [String: N]) -> N { .map(fields.map { SoftmaxAppSurface.CBORMapEntry($0.key, $0.value) }) }
        let source = try XCTUnwrap(ProcessInfo.processInfo.environment["SOFTMAX_ECHO_INSTALL"])
        let root = FileManager.default.temporaryDirectory.appendingPathComponent("app-host-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: root.appendingPathComponent("surface"), withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: root) }
        try Data(CogletTypes.CBORCodec.encodeProfile(.map([]))).write(to: root.appendingPathComponent("surface/ledger.cbor"))
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: root.appendingPathComponent("surface/ledger.cbor").path)
        let installed = URL(fileURLWithPath: source)
        try FileManager.default.copyItem(at: installed.appendingPathComponent("binding.json"), to: root.appendingPathComponent("binding.json"))
        var configuration = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: installed.appendingPathComponent("runtime.json"))) as? [String: Any])
        var setup = try XCTUnwrap(configuration["setup"] as? [String: Any])
        var manifest = try XCTUnwrap(setup["manifest"] as? [String: Any])
        let owner = try XCTUnwrap(manifest["id"] as? String)
        let actionID = String(repeating: "e", count: 32)
        manifest["required_capabilities"] = (manifest["required_capabilities"] as? [String] ?? []) + ["softmax.app.state", "softmax.app.invoke"]
        manifest["exclusive_outputs"] = (manifest["exclusive_outputs"] as? [String] ?? []) + ["softmax.app_control"]
        setup["manifest"] = manifest; setup["contract_profile"] = "surface-0.2"
        setup["initial_effect_ids"] = ["actions": [actionID], "timers": [], "llm_requests": []]
        setup["initial_actions"] = [["id": actionID, "coglet": owner, "capability": "softmax.app.invoke",
            "args": ["verb": "press", "window": "settings", "control": "settings.general.test_button"],
            "deadline": 60_000_000, "correlation_id": actionID]]
        let product = URL(fileURLWithPath: #filePath).deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().deletingLastPathComponent().appendingPathComponent("Surface")
        configuration["trusted_extension_catalog"] = ["json": try String(contentsOf: product.appendingPathComponent("catalog.v1.json"), encoding: .utf8),
            "sha256": try String(contentsOf: product.appendingPathComponent("catalog.v1.sha256"), encoding: .utf8).trimmingCharacters(in: .whitespacesAndNewlines)]
        configuration["setup"] = setup; configuration["wal"] = root.appendingPathComponent("state.wal").path
        try JSONSerialization.data(withJSONObject: configuration, options: [.sortedKeys]).write(to: root.appendingPathComponent("runtime.json"))
        var presses = 0
        let userPress = { presses += 1 }
        let registry = ControlRegistry(revision: 1, currentRoute: { $0 == "settings" ? "general" : nil })
        _ = try registry.mount(id: "settings.general.test_button", window: "settings", route: "general", role: "button", label: "Test button",
            verbs: ["press"], value: { object(["kind": .text("none")]) }, invoke: { _ in userPress() })
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
        host.automaticallyPolls = false
        host.installAppSurface(adapter, refresh: { try registry.nativeStateChanged() }, events: { registry.onChange = $0 })
        await host.start()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(presses, 0, "SDK acceptance cannot fake a native effect")
        await host.poll()
        XCTAssertEqual(host.state, .running, host.errorMessage ?? "")
        XCTAssertEqual(presses, 1)
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
