import XCTest
@testable import SoftmaxAppSurface

final class UpdateCheckCommandTests: XCTestCase {
    @MainActor private func started(_ flag: () -> Bool) async {
        for _ in 0..<10_000 { if flag() { return }; await Task.yield() }
        XCTFail("Native initiation was not reached")
    }

    @MainActor func testDecisionWaitsForNativeCallbackAndSlotWaitsForCycleEnd() async throws {
        let command = UpdateCheckCommand()
        var calls = 0, returned = false
        let task = Task {
            try await command.run(context: nil, available: { true }) { calls += 1; return true }
            returned = true
        }
        await started { calls == 1 }
        XCTAssertFalse(returned)
        command.foundUpdate()
        try await task.value
        XCTAssertTrue(returned)
        do {
            try await command.run(context: nil, available: { true }) { calls += 1; return true }
            XCTFail("Previous Sparkle cycle is still active")
        } catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
        XCTAssertEqual(calls, 1)
        command.aborted() // Duplicate/later callbacks must not resume twice.
        command.finishedCycle()
        let next = Task { try await command.run(context: nil, available: { true }) { calls += 1; return true } }
        await started { calls == 2 }
        command.noUpdateFound()
        try await next.value
        command.finishedCycle()
    }

    @MainActor func testUnavailableNeverStartsAndAbortOrUnknownFinishFails() async throws {
        let command = UpdateCheckCommand()
        var calls = 0
        do {
            try await command.run(context: nil, available: { false }) { calls += 1; return true }
            XCTFail("Unavailable")
        } catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
        XCTAssertEqual(calls, 0)
        for abort in [true, false] {
            let before = calls
            let task = Task { try await command.run(context: nil, available: { true }) { calls += 1; return true } }
            await started { calls == before + 1 }
            if abort { command.aborted() } else { command.finishedCycle() }
            do { try await task.value; XCTFail("No positive native decision") }
            catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
            command.finishedCycle()
        }
    }

    @MainActor func testSynchronousNativeDecisionAndIdleCallbacksAreSafe() async throws {
        let command = UpdateCheckCommand()
        command.foundUpdate(); command.finishedCycle()
        try await command.run(context: nil, available: { true }) { command.noUpdateFound(); return true }
        command.finishedCycle()
    }

    @MainActor func testSensitiveAdapterChecksOriginalAuthorityAtInitiation() async throws {
        let command = UpdateCheckCommand()
        var allowed = true, calls = 0, entered = false
        let registry = ControlRegistry(revision: 0, currentRoute: { _ in "updates" })
        try registry.mount(id: "settings.updates.check", window: "settings", route: "updates",
            role: "button", label: "Check for Updates", sensitive: true, verbs: ["press"],
            value: { .map([.init("kind", .text("none"))]) }, invokeAsync: { _, context in
                entered = true
                allowed = false
                try await command.run(context: context, available: { true }) { calls += 1; return true }
            }, invoke: { _ in XCTFail("Never synchronous") })
        let args = CBORValue.map([.init("window", .text("settings")), .init("control", .text("settings.updates.check")), .init("verb", .text("press"))])
        do { _ = try registry.prepare(args, sensitive: false); XCTFail("Sensitive control") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .permissionDenied) }
        let request = try registry.prepare(args, sensitive: true)
        do { try await registry.performAsync(request, authorized: { allowed }); XCTFail("Revoked") }
        catch { XCTAssertEqual(error as? ControlRegistryFailure, .permissionDenied) }
        XCTAssertTrue(entered, "Original admission passed; command must recheck after invocation revokes it")
        XCTAssertEqual(calls, 0)
        // Refused initiation must not occupy the command slot.
        try await command.run(context: nil, available: { true }) { command.foundUpdate(); calls += 1; return true }
        command.finishedCycle()
        XCTAssertEqual(calls, 1)
    }

    @MainActor func testNoOpInitiationAndPreStartCancellationDoNotOccupySlot() async throws {
        let command = UpdateCheckCommand()
        var calls = 0
        do {
            try await command.run(context: nil, available: { true }) { calls += 1; return false }
            XCTFail("No native session started")
        } catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
        let task = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            try await command.run(context: nil, available: { true }) { calls += 1; return true }
        }
        do { try await task.value; XCTFail("Already cancelled") }
        catch { XCTAssertTrue(error is CancellationError) }
        XCTAssertEqual(calls, 1)
        try await command.run(context: nil, available: { true }) { command.foundUpdate(); return true }
        command.finishedCycle()
    }

    @MainActor func testCancellationDoesNotReleaseNativeSessionOrClaimSuccess() async throws {
        let command = UpdateCheckCommand()
        var calls = 0
        let task = Task { try await command.run(context: nil, available: { true }) { calls += 1; return true } }
        await started { calls == 1 }
        task.cancel()
        do {
            try await command.run(context: nil, available: { true }) { calls += 1; return true }
            XCTFail("Cancelled native cycle is not retired yet")
        } catch { XCTAssertEqual(error as? ControlRegistryFailure, .unavailable) }
        command.foundUpdate()
        do { try await task.value; XCTFail("Cancelled task") } catch { XCTAssertTrue(error is CancellationError) }
        command.finishedCycle()
        XCTAssertEqual(calls, 1)
    }
}
