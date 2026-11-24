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
    func record(_ name: String) { lock.withLock { calls.append(name); if Thread.isMainThread { mainCalls += 1 } } }
}
private func fixture() -> WorkerFixture { WorkerFixture.lock.withLock { WorkerFixture.current! } }
private func configureWorker(_ bytes: UnsafePointer<UInt8>?, _ n: Int) -> Int32 { fixture().record("configure"); return 0 }
private func startWorker(_ bytes: UnsafePointer<UInt8>?, _ n: Int) -> Int32 { fixture().record("start"); return 0 }
private func stopWorker() -> Int32 { fixture().record("stop"); return 0 }
private func statusWorker() -> Int32 { fixture().record("status"); return 1 }
private func openWorker(_ v: UInt32, _ out: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 {
    fixture().record("open"); out?.pointee = OpaquePointer(bitPattern: 1); return 0
}
private func sendWorker(_ p: OpaquePointer?, _ raw: UnsafePointer<UInt8>?, _ n: Int) -> Int32 {
    let f = fixture(); f.record("send")
    let (gate, entered) = f.lock.withLock { (f.gate, f.entered) }
    entered?.signal(); gate?.wait()
    return f.lock.withLock {
        if f.blocked { return 1 }
        f.messages.append(Array(UnsafeBufferPointer(start: raw!, count: n))); return 0
    }
}
private func recvWorker(_ p: OpaquePointer?, _ raw: UnsafeMutablePointer<UInt8>?, _ n: Int, _ size: UnsafeMutablePointer<Int>?) -> Int32 {
    fixture().record("recv"); size?.pointee = 0; return 1
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
final class FaceRuntimeWorkerTests: XCTestCase {
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
        } catch {}
        gate.signal(); try await link.shutdown()
        f.lock.withLock { f.gate = nil; f.entered = nil }
        let successor = try await FaceEmbeddedRuntimeLink.start(abi: functions(), binding: [], configuration: [])
        await link.close(); try await link.shutdown()
        XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "stop" }.count }, 1)
        await successor.close(); try await successor.shutdown()
        XCTAssertEqual(f.lock.withLock { f.calls.filter { $0 == "stop" }.count }, 2)
    }
}
