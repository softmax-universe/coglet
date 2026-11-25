import XCTest
import Foundation
import CogletSurface
import CogletTypes
@testable import FaceMac

// Scheduling/ownership fixture, NOT real runtime qualification.
private final class WorkerFixture: @unchecked Sendable {
    static let lock = NSLock()
    nonisolated(unsafe) static var current: WorkerFixture?
    let lock = NSLock()
    var calls: [String] = []
    var mainCalls = 0
    var blocked = true
    var messages: [[UInt8]] = []
    var gate: DispatchSemaphore?
    var entered: DispatchSemaphore?
    var configureResult: Int32 = 0
    var startResult: Int32 = 0
    var stopResult: Int32 = 0
    var statusResult: Int32 = 1
    var openResult: Int32 = 0
    var sendResult: Int32?
    var recvQueryResult: Int32?
    var recvReadResult: Int32?
    var configureGate: DispatchSemaphore?
    var configureEntered: DispatchSemaphore?
    var inbound: [[UInt8]] = []
    var received = 0
    var attempts: [[UInt8]] = []
    var sendSteps: [WorkerSendStep] = []
    var receiveStep: WorkerSendStep?
    var eventDelayUS: UInt64 = 0
    var clockUS: UInt64 = 0
    var replyTimesUS: [UInt64] = []
    func record(_ name: String) { lock.withLock { calls.append(name); if Thread.isMainThread { mainCalls += 1 } } }
}
private func fixture() -> WorkerFixture { WorkerFixture.lock.withLock { WorkerFixture.current! } }
private func configureWorker(_ bytes: UnsafePointer<UInt8>?, _ n: Int) -> Int32 {
    let f = fixture(); f.record("configure")
    let (gate, entered) = f.lock.withLock { (f.configureGate, f.configureEntered) }
    entered?.signal(); gate?.wait(); return f.lock.withLock { f.configureResult }
}
private func startWorker(_ bytes: UnsafePointer<UInt8>?, _ n: Int) -> Int32 {
    let f = fixture(); f.record("start"); return f.lock.withLock { f.startResult }
}
private func stopWorker() -> Int32 { let f = fixture(); f.record("stop"); return f.lock.withLock { f.stopResult } }
private func statusWorker() -> Int32 { let f = fixture(); f.record("status"); return f.lock.withLock { f.statusResult } }
private func openWorker(_ v: UInt32, _ out: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 {
    let f = fixture(); f.record("open")
    return f.lock.withLock {
        if f.openResult == 0 { out?.pointee = OpaquePointer(bitPattern: 1) }
        return f.openResult
    }
}
private final class WorkerSendStep: @unchecked Sendable {
    let entered = DispatchSemaphore(value: 0)
    let release = DispatchSemaphore(value: 0)
    let result: Int32
    init(_ result: Int32 = 0) { self.result = result }
}
private func sendWorker(_ p: OpaquePointer?, _ raw: UnsafePointer<UInt8>?, _ n: Int) -> Int32 {
    let f = fixture(); f.record("send")
    let bytes = Array(UnsafeBufferPointer(start: raw!, count: n))
    let (gate, entered, step) = f.lock.withLock {
        f.attempts.append(bytes)
        return (f.gate, f.entered, f.sendSteps.isEmpty ? nil : f.sendSteps.removeFirst())
    }
    entered?.signal(); gate?.wait()
    step?.entered.signal(); step?.release.wait()
    return f.lock.withLock {
        let result = step?.result ?? f.sendResult ?? (f.blocked ? 1 : 0)
        guard result == 0 else { return result }
        let type = try? SurfaceWire.decode(bytes, from: .surface)["type"]
        if type == .text("event") { f.clockUS += f.eventDelayUS }
        if type == .text("read_reply") { f.replyTimesUS.append(f.clockUS) }
        f.messages.append(bytes); return 0
    }
}
private func recvWorker(_ p: OpaquePointer?, _ raw: UnsafeMutablePointer<UInt8>?, _ n: Int, _ size: UnsafeMutablePointer<Int>?) -> Int32 {
    let f = fixture(); f.record("recv")
    let step = f.lock.withLock { let step = f.receiveStep; f.receiveStep = nil; return step }
    step?.entered.signal(); step?.release.wait()
    return f.lock.withLock {
        if raw == nil, let result = f.recvQueryResult { size?.pointee = 0; return result }
        if raw != nil, let result = f.recvReadResult { return result }
        guard let first = f.inbound.first else { size?.pointee = 0; return 1 }
        size?.pointee = first.count
        guard let raw, n >= first.count else { return 2 }
        for (index, byte) in first.enumerated() { raw[index] = byte }
        f.inbound.removeFirst(); f.received += 1; return 0
    }
}
private func closeWorker(_ p: OpaquePointer?) { fixture().record("close") }
private func functions() -> FaceRuntimeABI {
    .init(surface: .init(open: openWorker, send: sendWorker, recv: recvWorker, close: closeWorker),
          configure: configureWorker, start: startWorker, stop: stopWorker, status: statusWorker)
}
private func hello() throws -> [UInt8] {
    try SurfaceWire.encode(.message("hello", .record([
        ("surface", .text("surface:" + String(repeating: "b", count: 64))),
        ("clock", .unsigned(0)), ("offer", .record([("versions", .array([.text("0.1")]))])),
        ("correlation_id", .text(String(repeating: "a", count: 32)))])), from: .surface)
}
private final class WorkerLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
}
private func workerEvent(_ sequence: UInt64, payload: CBORValue = .record([])) throws -> [UInt8] {
    try SurfaceWire.encode(.message("event", .record([
        ("source", .text("surface:" + String(repeating: "b", count: 64))),
        ("kind", .text("speech.partial")), ("seq", .unsigned(sequence)),
        ("logical_time", .unsigned(0)), ("payload", payload),
        ("correlation_id", .text(String(format: "%032x", sequence)))])), from: .surface)
}
private func workerReply(_ id: UInt64) throws -> [UInt8] {
    try SurfaceWire.encode(.message("read_reply", .record([
        ("correlation_id", .text(String(format: "%032x", id))),
        ("ok", .boolean(true)), ("value", .record([])), ("snapshot", .unsigned(0))])), from: .surface)
}
final class FaceRuntimeWorkerTests: XCTestCase {
    @MainActor private static func waitForDrain(_ link: FaceEmbeddedRuntimeLink) async throws {
        let deadline = ContinuousClock.now + .seconds(3)
        while !(try link.flush()) && ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertTrue(try link.flush(), "worker did not drain within watchdog")
    }
    func testReadReplyDelayPairAvoidsTwoEarlierEventCommits() async throws {
        try await Self.exerciseReadReplyDelayPair()
    }
    @MainActor private static func exerciseReadReplyDelayPair() async throws {
        for delay: UInt64 in [0, 600_000] {
            let f = WorkerFixture(), head = WorkerSendStep()
            f.blocked = false; f.eventDelayUS = delay; f.sendSteps = [head]
            WorkerFixture.lock.withLock { WorkerFixture.current = f }
            let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            defer { head.release.signal(); link.close() }
            let initial = try hello(), e0 = try workerEvent(0), e1 = try workerEvent(1), reply = try workerReply(2)
            XCTAssertTrue(try link.send(initial))
            XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
            // The ABI call is held, so both events and the reply are queued
            // before pumping resumes. Only successful event sends advance time.
            for frame in [e0, e1, reply] { XCTAssertTrue(try link.send(frame)) }
            head.release.signal()
            try await waitForDrain(link)
            XCTAssertEqual(f.lock.withLock { f.replyTimesUS }, [0], "delay=\(delay)")
            XCTAssertLessThan(f.lock.withLock { f.replyTimesUS.first ?? UInt64.max }, 1_000_000)
            XCTAssertEqual(f.lock.withLock { f.clockUS }, delay * 2)
            XCTAssertEqual(f.lock.withLock { f.messages }, [initial, reply, e0, e1])
            try await link.shutdown()
        }
    }
    func testReadReplyCannotDisplaceConcurrentInFlightEvent() async throws {
        try await Self.exercisePinnedEvent(wouldBlock: false)
    }
    func testReadReplyCannotDisplaceWouldBlockedEventBetweenAttempts() async throws {
        try await Self.exercisePinnedEvent(wouldBlock: true)
    }
    @MainActor private static func exercisePinnedEvent(wouldBlock: Bool) async throws {
        let f = WorkerFixture(), head = WorkerSendStep(wouldBlock ? 1 : 0), between = WorkerSendStep()
        f.blocked = false; f.sendSteps = [head]
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        defer { head.release.signal(); between.release.signal(); link.close() }
        let e0 = try workerEvent(0), e1 = try workerEvent(1), e2 = try workerEvent(2), reply = try workerReply(3)
        XCTAssertTrue(try link.send(e0))
        XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
        if wouldBlock {
            // Pause after send returned would-block, before the next head
            // selection. This catches pinning only during an active ABI call.
            f.lock.withLock { f.receiveStep = between }
            head.release.signal()
            XCTAssertEqual(between.entered.wait(timeout: .now() + 2), .success)
        }
        for frame in [e1, e2, reply] { XCTAssertTrue(try link.send(frame)) }
        if wouldBlock { between.release.signal() } else { head.release.signal() }
        try await waitForDrain(link)
        XCTAssertEqual(f.lock.withLock { f.messages }, [e0, reply, e1, e2])
        XCTAssertEqual(f.lock.withLock { f.attempts }, wouldBlock ? [e0, e0, reply, e1, e2] : [e0, reply, e1, e2])
        try await link.shutdown()
    }
    func testReadReplyStopsAtNonEventBarriersAndPreviousReplies() async throws {
        try await Self.exerciseOrderingFences()
    }
    @MainActor private static func exerciseOrderingFences() async throws {
        let correlation = CBORValue.text(String(repeating: "c", count: 32))
        let barriers: [CBORValue] = [
            .message("health", .record([("state", .text("ready")), ("clock", .unsigned(0)),
                ("pending", .unsigned(0)), ("correlation_id", correlation)])),
            .message("grants", .record([("epoch", .unsigned(1)), ("entries", .array([])), ("correlation_id", correlation)])),
            .message("kill", .record([("reason", .text("stop")), ("correlation_id", correlation)]))]
        for value in barriers {
            let f = WorkerFixture(), head = WorkerSendStep()
            f.blocked = false; f.sendSteps = [head]
            WorkerFixture.lock.withLock { WorkerFixture.current = f }
            let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            defer { head.release.signal(); link.close() }
            let initial = try hello(), before = try workerEvent(0), after = try workerEvent(1),
                last = try workerEvent(2), r0 = try workerReply(3), r1 = try workerReply(4),
                barrier = try SurfaceWire.encode(value, from: .surface)
            XCTAssertTrue(try link.send(initial))
            XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
            for frame in [before, barrier, after, r0, last, r1] { XCTAssertTrue(try link.send(frame)) }
            head.release.signal()
            try await waitForDrain(link)
            XCTAssertEqual(f.lock.withLock { f.messages }, [initial, before, barrier, r0, r1, after, last])
            try await link.shutdown()
        }
    }
    func testCloseDropsPromotedReplyAndQueuedEventsWithoutRetryingHead() async throws {
        try await Self.exerciseCloseWithReply()
    }
    @MainActor private static func exerciseCloseWithReply() async throws {
        for result: Int32 in [0, 1] {
            let f = WorkerFixture(), head = WorkerSendStep(result)
            f.blocked = false; f.sendSteps = [head]
            WorkerFixture.lock.withLock { WorkerFixture.current = f }
            let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            defer { head.release.signal(); link.close() }
            let e0 = try workerEvent(0), e1 = try workerEvent(1), reply = try workerReply(2)
            XCTAssertTrue(try link.send(e0))
            XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
            for frame in [e1, reply] { XCTAssertTrue(try link.send(frame)) }
            link.close()
            XCTAssertThrowsError(try link.send(reply))
            XCTAssertThrowsError(try link.receive())
            XCTAssertThrowsError(try link.flush())
            head.release.signal(); try await link.shutdown()
            XCTAssertEqual(f.lock.withLock { f.attempts }, [e0])
            // An already-entered successful ABI call cannot be retracted.
            XCTAssertEqual(f.lock.withLock { f.messages }, result == 0 ? [e0] : [])
            XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "close" }.count }, 1)
        }
    }
    func testReadReplyPromotionRespectsMailboxCountLimitAndReclaimsCapacity() async throws {
        try await Self.exerciseReplyCapacity()
    }
    @MainActor private static func exerciseReplyCapacity() async throws {
        let f = WorkerFixture(), head = WorkerSendStep()
        f.blocked = false; f.sendSteps = [head]
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        defer { head.release.signal(); link.close() }
        let initial = try hello(), event = try workerEvent(0), reply = try workerReply(1)
        XCTAssertTrue(try link.send(initial))
        XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
        for _ in 0..<126 { XCTAssertTrue(try link.send(event)) }
        XCTAssertTrue(try link.send(reply))
        XCTAssertFalse(try link.send(reply))
        head.release.signal(); try await waitForDrain(link)
        XCTAssertEqual(f.lock.withLock { f.messages }, [initial, reply] + Array(repeating: event, count: 126))
        XCTAssertTrue(try link.send(reply)); try await waitForDrain(link)
        XCTAssertEqual(f.lock.withLock { f.messages.last }, reply)
        try await link.shutdown()
    }
    func testReadReplyPromotionRespectsByteLimitAndReclaimsExactBytes() async throws {
        try await Self.exerciseReplyByteCapacity()
    }
    @MainActor private static func exerciseReplyByteCapacity() async throws {
        let f = WorkerFixture(); f.blocked = false
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        let initial = try hello(), reply = try workerReply(1)
        defer { link.close() }
        let event = try workerEvent(0, payload: .record([("padding", .text(String(repeating: "x", count: 950_000)))]))
        let count = (8 * 1024 * 1024 - initial.count - reply.count) / event.count
        XCTAssertLessThan(count + 2, 128, "must exercise bytes, not the message-count limit")
        for _ in 0..<2 {
            let head = WorkerSendStep()
            defer { head.release.signal() }
            f.lock.withLock { f.sendSteps = [head] }
            XCTAssertTrue(try link.send(initial))
            XCTAssertEqual(head.entered.wait(timeout: .now() + 2), .success)
            for _ in 0..<count { XCTAssertTrue(try link.send(event)) }
            XCTAssertTrue(try link.send(reply))
            XCTAssertFalse(try link.send(event))
            head.release.signal(); try await waitForDrain(link)
            XCTAssertEqual(f.lock.withLock { Array(f.messages.suffix(count + 2)) },
                [initial, reply] + Array(repeating: event, count: count))
        }
        try await link.shutdown()
    }
    func testPumpFailureRetainsSanitizedPhaseAndAvailableStatus() async throws {
        try await Self.exercisePumpFailureDiagnostics()
    }
    @MainActor private static func exercisePumpFailureDiagnostics() async throws {
        for mode in ["send", "query", "read", "decode"] {
            let f = WorkerFixture()
            WorkerFixture.lock.withLock { WorkerFixture.current = f }
            let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            let expected: FaceRuntimeDiagnostic
            switch mode {
            case "send":
                expected = .init(phase: .send, status: -7)
                f.lock.withLock { f.sendResult = -7 }
                XCTAssertTrue(try link.send(hello()))
            case "query":
                // SDK intentionally exposes no numeric status for receive errors.
                expected = .init(phase: .receive, cause: .invalidSizeQuery)
                f.lock.withLock { f.recvQueryResult = -2 }
            case "read":
                expected = .init(phase: .receive, cause: .invalidExactRead)
                f.lock.withLock { f.inbound = [[0xa0]]; f.recvReadResult = -9 }
            default:
                // The schema validator's error is not a SurfaceFailure; retain
                // the phase without retaining arbitrary validator text.
                expected = .init(phase: .receive, cause: .unknown)
                f.lock.withLock { f.inbound = [[0xa0]] }
            }
            var observed: FaceRuntimeDiagnostic?
            let deadline = ContinuousClock.now + .seconds(3)
            while observed == nil && ContinuousClock.now < deadline {
                do { _ = try link.receive() }
                catch { observed = error as? FaceRuntimeDiagnostic; XCTAssertEqual(observed, expected) }
                if observed == nil { try await Task.sleep(for: .milliseconds(5)) }
            }
            XCTAssertEqual(observed, expected, mode)
            link.close()
            XCTAssertThrowsError(try link.send(hello())) { XCTAssertEqual($0 as? FaceRuntimeDiagnostic, expected) }
            XCTAssertThrowsError(try link.receive()) { XCTAssertEqual($0 as? FaceRuntimeDiagnostic, expected) }
            XCTAssertThrowsError(try link.flush()) { XCTAssertEqual($0 as? FaceRuntimeDiagnostic, expected) }
            try await link.shutdown()
            XCTAssertEqual(f.lock.withLock { Array(f.calls.suffix(2)) }, ["stop", "status"])
            XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "close" }.count }, 1)
        }
    }
    func testDiagnosticNeverCopiesArbitraryErrorText() {
        let secret = "private binding/configuration/frame data"
        for message in [secret, "ABI send failed: " + secret, "ABI open failed: " + secret] {
            let diagnostic = FaceRuntimeDiagnostic(phase: .send, error: SurfaceFailure.protocolViolation(message))
            XCTAssertEqual(diagnostic, .init(phase: .send, cause: .protocolViolation))
            XCTAssertFalse(String(describing: diagnostic).contains(secret))
        }
        let unknown = FaceRuntimeDiagnostic(phase: .receive, error: NSError(domain: secret, code: 42))
        XCTAssertEqual(unknown, .init(phase: .receive, cause: .unknown))
        XCTAssertFalse(String(describing: unknown).contains(secret))
    }
    func testInboundSaturationBackpressuresWithoutDroppingFrames() async throws {
        let f = WorkerFixture()
        let frames = try (0..<129).map { index in
            try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])),
                ("correlation_id", .text(String(format: "%032x", index)))])), from: .runtime)
        }
        f.inbound = frames
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        let deadline = ContinuousClock.now + .seconds(3)
        while f.lock.withLock({ f.received }) < 128 && ContinuousClock.now < deadline {
            try await Task.sleep(for: .milliseconds(5))
        }
        XCTAssertEqual(f.lock.withLock { f.received }, 128)
        try await Task.sleep(for: .milliseconds(30))
        XCTAssertEqual(f.lock.withLock { f.inbound }, [frames[128]])
        var received: [[UInt8]] = []
        while received.count < 129 && ContinuousClock.now < deadline {
            if let next = try await link.receive() { received.append(next) }
            else { try await Task.sleep(for: .milliseconds(5)) }
        }
        XCTAssertEqual(received, frames)
        try await link.shutdown()
    }
    func testOpenFailureStopsRuntimeBeforeSuccessor() async throws {
        let f = WorkerFixture(); f.openResult = -1
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        do {
            _ = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            XCTFail("failed open succeeded")
        } catch { XCTAssertEqual(error as? FaceRuntimeDiagnostic, .init(phase: .open, status: -1)) }
        XCTAssertEqual(f.lock.withLock { f.calls }, ["configure", "start", "open", "stop", "status"])
        f.lock.withLock { f.openResult = 0 }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        try await successor.shutdown()
    }
    func testFailedStopRetainsCustody() async throws {
        // A failed stop intentionally poisons process-wide custody. Isolate each
        // failure in a child rather than resetting the production supervisor.
        if let mode = ProcessInfo.processInfo.environment["FACE_WORKER_STOP_FAILURE"] {
            let f = WorkerFixture()
            f.stopResult = mode == "stop" ? -1 : 0
            f.statusResult = mode == "status" ? 0 : 1
            WorkerFixture.lock.withLock { WorkerFixture.current = f }
            let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            do { try await link.shutdown(); XCTFail("failed stop attested clean") }
            catch {
                XCTAssertEqual(error as? FaceRuntimeDiagnostic,
                    .init(phase: mode == "stop" ? .stop : .status, status: mode == "stop" ? -1 : 0))
            }
            let calls = f.lock.withLock { f.calls }
            do {
                _ = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
                XCTFail("successor acquired failed runtime")
            } catch FaceRuntimeFailure.busy {} catch { XCTFail("lost busy refusal: \(error)") }
            XCTAssertEqual(f.lock.withLock { f.calls }, calls)
            return
        }
        for mode in ["stop", "status"] {
            let child = Process()
            #if os(macOS)
            child.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
            child.arguments = ["xctest", "-XCTest", "FaceMacTests.FaceRuntimeWorkerTests/testFailedStopRetainsCustody", Bundle(for: Self.self).bundlePath]
            #else
            child.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
            child.arguments = ["FaceMacTests.FaceRuntimeWorkerTests/testFailedStopRetainsCustody"]
            #endif
            child.environment = ProcessInfo.processInfo.environment.merging(["FACE_WORKER_STOP_FAILURE": mode]) { _, value in value }
            try child.run()
            let deadline = ContinuousClock.now + .seconds(10)
            while child.isRunning && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(10)) }
            if child.isRunning { child.terminate(); XCTFail("stop-failure child timed out") }
            child.waitUntilExit()
            XCTAssertEqual(child.terminationStatus, 0, "failure mode: \(mode)")
        }
    }
    func testNativeOffSwitchReturnsWhileGuestBlocked() async throws {
        try await Self.exerciseNativeOffSwitchWhileGuestBlocked()
    }
    @MainActor private static func exerciseNativeOffSwitchWhileGuestBlocked() async throws {
        let f = WorkerFixture(); WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        let entered = DispatchSemaphore(value: 0), gate = DispatchSemaphore(value: 0)
        let (host, io) = try await MainActor.run {
            let io = TestDuplex(), owner = "coglet:" + String(repeating: "a", count: 64)
            let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "b", count: 64), owner: owner,
                store: WorkerLedger(), backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [], clock: { 0 })
            try host.attach(link: link)
            try io.start()
            f.lock.withLock { f.entered = entered; f.gate = gate }
            try host.start()
            return (host, io)
        }
        XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
        // The real host reflex must stop native I/O without joining ABI teardown.
        try await MainActor.run {
            try host.localOffSwitch()
            XCTAssertFalse(io.isRunning)
            XCTAssertThrowsError(try link.receive())
        }
        XCTAssertFalse(f.lock.withLock { f.calls.contains("stop") })
        gate.signal()
        try await link.shutdown()
        try host.close()
    }
    func testFailedConfigureRequiresStopAttestation() async throws {
        let f = WorkerFixture(); f.configureResult = -2
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        do {
            _ = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            XCTFail("failed configure succeeded")
        } catch {
            XCTAssertEqual(error as? FaceRuntimeDiagnostic, .init(phase: .configure, status: -2))
        }
        XCTAssertEqual(f.lock.withLock { f.calls }, ["configure", "stop", "status"])
        f.lock.withLock { f.configureResult = 0 }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        try await successor.shutdown()
    }
    func testFailedStartStillRequiresStopAttestation() async throws {
        let f = WorkerFixture(); f.startResult = -2
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        do {
            _ = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            XCTFail("failed ABI start succeeded")
        } catch { XCTAssertEqual(error as? FaceRuntimeDiagnostic, .init(phase: .start, status: -2)) }
        XCTAssertEqual(f.lock.withLock { f.calls }, ["configure", "start", "stop", "status"])
        f.lock.withLock { f.startResult = 0 }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        try await successor.shutdown()
    }
    func testCancellationDuringConfigureDoesNotStartGuest() async throws {
        let f = WorkerFixture(), entered = DispatchSemaphore(value: 0), gate = DispatchSemaphore(value: 0)
        f.configureEntered = entered; f.configureGate = gate
        WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let task = Task { @MainActor in
            try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        }
        XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
        task.cancel(); gate.signal()
        do { _ = try await task.value; XCTFail("cancelled start succeeded") } catch {}
        XCTAssertEqual(f.lock.withLock { f.calls }, ["configure", "stop", "status"])
        f.lock.withLock { f.configureEntered = nil; f.configureGate = nil }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        try await successor.shutdown()
    }
    func testCancelledStartupReleasesCustodyBeforeSuccessor() async throws {
        let f = WorkerFixture(); WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let task = Task { @MainActor in
            try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        }
        task.cancel()
        do { let link = try await task.value; await link.close(); try await link.shutdown(); XCTFail("cancelled start succeeded") } catch {}
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        await successor.close(); try await successor.shutdown()
    }
    func testBoundedMailboxRetriesIdenticalBytesAndConfinesWholeLifecycleOffMain() async throws {
        let f = WorkerFixture(); WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [123,125], configuration: [123,125])
        let message = try hello()
        try await MainActor.run {
            for _ in 0..<128 { XCTAssertTrue(try link.send(message)) }
            XCTAssertFalse(try link.send(message))
        }
        f.lock.withLock { f.blocked = false }
        let deadline = ContinuousClock.now + .seconds(3)
        while f.lock.withLock({ f.messages.count }) < 128 && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(5)) }
        XCTAssertEqual(f.lock.withLock { f.messages }, Array(repeating: message, count: 128))
        await link.close()
        try await link.shutdown()
        XCTAssertEqual(f.lock.withLock { f.mainCalls }, 0)
        XCTAssertEqual(f.lock.withLock { Array(f.calls.prefix(3)) }, ["configure", "start", "open"])
        XCTAssertEqual(f.lock.withLock { Array(f.calls.suffix(3)) }, ["close", "stop", "status"])
        XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "close" }.count }, 1)
    }
    func testCloseNeverWaitsForGuestAndOldCloseCannotStopSuccessor() async throws {
        let f = WorkerFixture(); WorkerFixture.lock.withLock { WorkerFixture.current = f }
        let link = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        let entered = DispatchSemaphore(value: 0), gate = DispatchSemaphore(value: 0)
        f.lock.withLock { f.entered = entered; f.gate = gate }
        _ = try await link.send(hello())
        XCTAssertEqual(entered.wait(timeout: .now() + 2), .success)
        // The worker is blocked in simulated scheduled guest execution. MainActor
        // close must return BEFORE releasing it, without taking the ABI queue lock.
        await link.close()
        do {
            _ = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
            XCTFail("successor opened before old runtime stopped")
        } catch FaceRuntimeFailure.busy {} catch { XCTFail("lost busy refusal: \(error)") }
        gate.signal(); try await link.shutdown()
        f.lock.withLock { f.gate = nil; f.entered = nil }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        await link.close(); try await link.shutdown()
        XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "stop" }.count }, 1)
        await successor.close(); try await successor.shutdown()
        XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "stop" }.count }, 2)
    }
}
