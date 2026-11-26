import Foundation
import XCTest
@testable import SoftmaxAppSurface

final class URLOpenCommandTests: XCTestCase {
    @MainActor func testUserWaitsForActualAcceptanceAndDiscardIsFailure() async throws {
        let url = try XCTUnwrap(URL(string: "https://example.invalid/support"))
        var completion: ((Bool) -> Void)?
        var calls = 0
        let command = URLOpenCommand(url: url) { actual, done in
            XCTAssertEqual(actual, url); calls += 1; completion = done
        }
        var returned = false
        let task = Task { try await command.run(context: nil); returned = true }
        for _ in 0..<10_000 { if completion != nil { break }; await Task.yield() }
        guard completion != nil else { task.cancel(); XCTFail("Native callback was not reached"); return }
        XCTAssertFalse(returned); XCTAssertEqual(calls, 1)
        completion?(true)
        try await task.value
        XCTAssertTrue(returned)
        let rejected = URLOpenCommand(url: url) { _, done in done(false) }
        do { try await rejected.run(context: nil); XCTFail("Discard is not native success") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
    }

    @MainActor func testAdapterUsesSameCommandAndRechecksAuthorityAtActualOpen() async throws {
        let url = try XCTUnwrap(URL(string: "https://example.invalid/source"))
        var allowed = true, calls = 0
        let command = URLOpenCommand(url: url) { _, done in calls += 1; done(true) }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "about" })
        try registry.mount(id: "about.source", window: "about", route: "about", role: "button", label: "Source",
            verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
            invokeAsync: { _, context in
                // Admission occurred, but native initiation must still recheck.
                allowed = false
                try await command.run(context: context)
            }, invoke: { _ in XCTFail("Never synchronous") })
        let args = CBORValue.map([.init("window", .text("about")), .init("control", .text("about.source")), .init("verb", .text("press"))])
        let request = try registry.prepare(args, sensitive: false)
        do { try await registry.performAsync(request, authorized: { allowed }); XCTFail("Revoked") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .permissionDenied) }
        XCTAssertEqual(calls, 0)
        try await command.run(context: nil)
        XCTAssertEqual(calls, 1)
    }

    @MainActor func testAdapterRetainsPendingUntilNativeCompletion() async throws {
        let url = try XCTUnwrap(URL(string: "https://example.invalid/license"))
        var completion: ((Bool) -> Void)?
        let command = URLOpenCommand(url: url) { _, done in completion = done }
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "about" })
        try registry.mount(id: "about.sparkle_license", window: "about", route: "about", role: "button", label: "Sparkle licence",
            verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
            invokeAsync: { _, context in try await command.run(context: context) }, invoke: { _ in XCTFail("Never synchronous") })
        let request = try registry.prepare(.map([.init("window", .text("about")), .init("control", .text("about.sparkle_license")), .init("verb", .text("press"))]), sensitive: false)
        var returned = false
        let task = Task { try await registry.performAsync(request, authorized: { true }); returned = true }
        for _ in 0..<10_000 { if completion != nil { break }; await Task.yield() }
        guard completion != nil else { task.cancel(); XCTFail("Native callback was not reached"); return }
        XCTAssertFalse(returned)
        completion?(true)
        try await task.value
        XCTAssertTrue(returned)
    }
}
