import XCTest
@testable import SoftmaxAppSurface

final class NativeObservationTests: XCTestCase {
    func testCommittedNativeRoutesValuesAndFocus() async throws {
        try await MainActor.run {
            var route: String? = nil
            var value = false
            var focused = false
            let registry = ControlRegistry(revision: 1) { _ in route }
            var events: [(String, CBORValue)] = []
            registry.onChange = { events.append(($0, $1)) }
            _ = try registry.mount(id: "toggle", window: "settings", route: "general", role: "toggle",
                label: "Test", verbs: ["toggle"], value: {
                    .map([.init("kind", .text("boolean")), .init("value", .boolean(value))])
                }, focused: { focused }, invoke: { _ in })
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertTrue(events.isEmpty)
            route = "general"
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(events.map(\.0), ["softmax.app.window_changed"])
            events.removeAll()
            value = true; focused = true
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(events.map(\.0), ["softmax.app.control_changed", "softmax.app.focus_changed"])
            events.removeAll()
            let revision = registry.revision
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertTrue(events.isEmpty)
            XCTAssertEqual(registry.revision, revision)
            route = "audio"
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(events.map(\.0), ["softmax.app.view_changed", "softmax.app.focus_changed"])
            events.removeAll()
            route = nil
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(events.map(\.0), ["softmax.app.window_changed"])
        }
    }

    func testProtectedValueGetterNeverRunsDuringNativeObservation() async throws {
        try await MainActor.run {
            let registry = ControlRegistry(revision: 1) { _ in "hub" }
            var reads = 0
            var events: [CBORValue] = []
            registry.onChange = { _, value in events.append(value) }
            _ = try registry.mount(id: "token", window: "settings", route: "hub", role: "secure_field",
                label: "Invitation", sensitive: true, secure: true, verbs: ["type_text"], value: {
                    reads += 1
                    return .map([.init("kind", .text("text")), .init("value", .text("CANARY"))])
                }, invoke: { _ in })
            try registry.synchronizeNativeState(windows: ["settings"])
            try registry.synchronizeNativeState(windows: ["settings"])
            XCTAssertEqual(reads, 0)
            for event in events {
                XCTAssertFalse(String(decoding: try CBORCodec.encodeProfile(event), as: UTF8.self).contains("CANARY"))
            }
            XCTAssertThrowsError(try registry.synchronizeNativeState(windows: ["settings", "settings"]))
            XCTAssertThrowsError(try registry.synchronizeNativeState(windows: ["unknown"]))
        }
    }
}
