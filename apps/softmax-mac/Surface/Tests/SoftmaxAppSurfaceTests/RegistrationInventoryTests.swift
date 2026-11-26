import XCTest
@testable import SoftmaxAppSurface

final class RegistrationInventoryTests: XCTestCase {
    @MainActor func testInventoryCapturesActualMountsWithoutReadingValuesOrInvokingEffects() throws {
        var reads = 0, effects = 0, visible = true, enabled = false
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "general" })
        let mount = try registry.mount(id: "settings.secret", window: "settings", route: "general",
            role: "secure_field", label: "Secret", secure: true, verbs: ["type_text"],
            value: { reads += 1; return .map([]) }, enabled: { enabled }, visible: { visible },
            invoke: { _ in effects += 1 })
        let first = registry.registrationInventory()
        XCTAssertEqual(first.count, 1)
        XCTAssertEqual(first[0].id, "settings.secret")
        XCTAssertEqual(first[0].window, "settings")
        XCTAssertEqual(first[0].route, "general")
        XCTAssertEqual(first[0].role, "secure_field")
        XCTAssertEqual(first[0].verbs, ["type_text"])
        XCTAssertFalse(first[0].enabled)
        XCTAssertTrue(first[0].visible)
        enabled = true; visible = false
        let second = registry.registrationInventory()
        XCTAssertTrue(second[0].enabled)
        XCTAssertFalse(second[0].visible)
        XCTAssertFalse(first[0].enabled, "prior snapshot is immutable")
        XCTAssertTrue(first[0].visible)
        try registry.unmount(mount)
        XCTAssertTrue(registry.registrationInventory().isEmpty)
        XCTAssertEqual(reads, 0)
        XCTAssertEqual(effects, 0)
    }

    @MainActor func testInventoryIncludesDisabledOffRouteMountsAndExcludesRefusedDuplicate() throws {
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "audio" })
        func mount(_ id: String) throws {
            try registry.mount(id: id, window: "settings", route: "general", role: "button",
                label: "Button", sensitive: true, verbs: ["press"], value: { XCTFail("secret getter"); return .null },
                enabled: { false }, invoke: { _ in XCTFail("effect") })
        }
        try mount("settings.z"); try mount("settings.a")
        XCTAssertThrowsError(try mount("settings.a"))
        let inventory = registry.registrationInventory()
        XCTAssertEqual(inventory.map(\.id), ["settings.a", "settings.z"])
        XCTAssertTrue(inventory.allSatisfy { $0.sensitive && !$0.enabled })
        let data = try JSONEncoder().encode(inventory)
        let decoded = try JSONDecoder().decode([ControlRegistry.Registration].self, from: data)
        XCTAssertEqual(decoded, inventory)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [[String: Any]])
        XCTAssertTrue(object.allSatisfy { $0["value"] == nil && $0["label"] == nil })
    }
}
