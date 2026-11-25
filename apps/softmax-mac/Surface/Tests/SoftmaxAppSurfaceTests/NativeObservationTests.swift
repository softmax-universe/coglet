import XCTest
@testable import SoftmaxAppSurface

final class NativeObservationTests: XCTestCase {
    func testOffActorPreparationRemainsBoundToCapturedMount() async throws {
        let (registry, directory, mount) = try await MainActor.run {
            let registry = ControlRegistry(revision: 0) { _ in "general" }
            let mount = try registry.mount(id: "button", window: "settings", route: "general", role: "button",
                label: "Button", verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
                invoke: { _ in XCTFail("old native command") })
            return (registry, registry.captureDirectory(), mount)
        }
        let args: CBORValue = .map([.init("window", .text("settings")), .init("control", .text("button")), .init("verb", .text("press"))])
        let request = try await Task.detached {
            XCTAssertFalse(Thread.isMainThread)
            return try directory.prepare(args, sensitive: false)
        }.value
        try await MainActor.run {
            try registry.unmount(mount)
            _ = try registry.mount(id: "button", window: "settings", route: "general", role: "button",
                label: "Replacement", verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
                invoke: { _ in XCTFail("replacement borrowed") })
            XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        }
    }

    func testAppliedSelfDismissDoesNotBorrowSuccessorValueOrFail() async throws {
        try await MainActor.run {
            let registry = ControlRegistry(revision: 0) { _ in "general" }
            var old: ControlRegistry.Mount!
            var effects = 0, successorReads = 0
            old = try registry.mount(id: "dismiss", window: "settings", route: "general", role: "button",
                label: "Dismiss", verbs: ["press"], value: { .map([.init("kind", .text("none"))]) }, invoke: { _ in
                    effects += 1
                    try registry.unmount(old)
                    _ = try registry.mount(id: "dismiss", window: "settings", route: "general", role: "button",
                        label: "Successor", verbs: ["press"], value: {
                            successorReads += 1
                            return .map([.init("kind", .text("none"))])
                        }, invoke: { _ in XCTFail("successor invoked") })
                })
            let args: CBORValue = .map([.init("window", .text("settings")), .init("control", .text("dismiss")), .init("verb", .text("press"))])
            let request = try registry.prepare(args, sensitive: false)
            try registry.perform(request, authorized: { true })
            XCTAssertEqual(effects, 1)
            XCTAssertEqual(successorReads, 1) // registration validation only
            XCTAssertThrowsError(try registry.perform(request, authorized: { true }))
        }
    }

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
