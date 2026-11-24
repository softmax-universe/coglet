import XCTest
import Foundation
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class LifetimeLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
}

private final class HostBox: @unchecked Sendable {
    var host: FaceSurfaceHost?
}

@MainActor private final class LifetimeDuplex: FaceDuplexIO {
    var renderedFrames: UInt64 = 0
    var isRunning = false
    var generation: UInt64 = 0
    var captureFailed = false
    var onVoiceActivity: ((Bool) -> Void)?
    var stops = 0
    var stoppedOnMain = false
    func start() throws { isRunning = true }
    func stop() { isRunning = false; generation += 1; stops += 1; stoppedOnMain = Thread.isMainThread }
    func enqueue(_ samples: [Int16]) throws {}
    func flush() {}
    func readCapture() -> [Int16] { [] }
}

@MainActor private final class LifetimeLink: FaceControlLink {
    var sent: [[UInt8]] = []
    var incoming: [[UInt8]] = []
    var blocked = false
    var closed = false
    var failReceive = false
    var closedOnMain = false
    func send(_ bytes: [UInt8]) throws -> Bool { sent.append(bytes); return true }
    func receive() throws -> [UInt8]? {
        if failReceive { throw SurfaceFailure.closed }
        return incoming.isEmpty ? nil : incoming.removeFirst()
    }
    func flush() throws -> Bool { true }
    func close() { closed = true; closedOnMain = Thread.isMainThread }
}

final class FaceHostLifetimeTests: XCTestCase {
    func testRetiredHostDestructionCannotStopSuccessorNativeCustody() async throws {
        try await Self.exerciseRetiredSuccessor()
    }
    @MainActor private static func exerciseRetiredSuccessor() async throws {
        let io = LifetimeDuplex()
        var host: FaceSurfaceHost? = try FaceSurfaceHost(
            identity: "surface:" + String(repeating: "b", count: 64),
            owner: "coglet:" + String(repeating: "a", count: 64), store: LifetimeLedger(),
            backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
            grants: [], clock: { 0 })
        try host!.close()
        try io.start() // trusted successor takes the same physical graph
        let stops = io.stops
        host = nil
        try await Task.sleep(for: .milliseconds(50))
        XCTAssertTrue(io.isRunning)
        XCTAssertEqual(io.stops, stops, "retired cleanup must not repeat native stop")
        io.stop()
    }
    func testOffMainActorHostReleaseStopsNativeAndClosesTransport() async throws {
        // Swift 6 deinit is nonisolated; releasing the last host ref off
        // MainActor must not trap in SurfaceSession.deinit → backend.stop.
        // Isolate traps in a child so a RED crash cannot kill the suite.
        if let mode = ProcessInfo.processInfo.environment["FACE_HOST_LIFETIME_CHILD"] {
            try await Self.exercise(mode: mode)
            return
        }
        for mode in ["forgotten", "closed", "failclose"] {
            let child = Process()
            #if os(macOS)
            child.executableURL = URL(fileURLWithPath: "/usr/bin/xcrun")
            child.arguments = ["xctest", "-XCTest", "FaceMacTests.FaceHostLifetimeTests/testOffMainActorHostReleaseStopsNativeAndClosesTransport", Bundle(for: Self.self).bundlePath]
            #else
            child.executableURL = URL(fileURLWithPath: CommandLine.arguments[0])
            child.arguments = ["FaceMacTests.FaceHostLifetimeTests/testOffMainActorHostReleaseStopsNativeAndClosesTransport"]
            #endif
            child.environment = ProcessInfo.processInfo.environment.merging(["FACE_HOST_LIFETIME_CHILD": mode]) { _, value in value }
            try child.run()
            let deadline = ContinuousClock.now + .seconds(15)
            while child.isRunning && ContinuousClock.now < deadline { try await Task.sleep(for: .milliseconds(10)) }
            if child.isRunning { child.terminate(); XCTFail("lifetime child timed out: \(mode)") }
            child.waitUntilExit()
            XCTAssertEqual(child.terminationStatus, 0, "lifetime child mode: \(mode)")
        }
    }

    @MainActor private static func negotiate(_ host: FaceSurfaceHost, link: LifetimeLink) throws {
        try host.attach(link: link)
        try host.start()
        let hello = try SurfaceWire.decode(link.sent[0], from: .surface)
        link.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
            ("selection", .record([("version", .text("0.1"))])),
            ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
        try host.pump()
    }

    private static func exercise(mode: String) async throws {
        let owner = "coglet:" + String(repeating: "a", count: 64)
        let identity = "surface:" + String(repeating: "b", count: 64)
        let box = HostBox()
        let (io, link) = try await MainActor.run { () -> (LifetimeDuplex, LifetimeLink) in
            let io = LifetimeDuplex(), link = LifetimeLink()
            let host = try FaceSurfaceHost(identity: identity, owner: owner, store: LifetimeLedger(),
                backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: owner, operations: Set(FaceSurfaceHost.operations.map(\.name)),
                    resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try negotiate(host, link: link)
            try host.activateDuplex(localConsent: true)
            XCTAssertTrue(io.isRunning)
            if mode == "closed" {
                try host.close()
                XCTAssertFalse(io.isRunning)
                XCTAssertTrue(link.closed)
            } else if mode == "failclose" {
                link.failReceive = true
                XCTAssertThrowsError(try host.pump())
                XCTAssertFalse(io.isRunning)
                XCTAssertTrue(link.closed)
            }
            box.host = host
            return (io, link)
        }
        await Task.detached {
            box.host = nil
        }.value
        let deadline = ContinuousClock.now + .seconds(2)
        while ContinuousClock.now < deadline {
            let done = await MainActor.run { !io.isRunning && link.closed && io.stops > 0 }
            if done { break }
            try await Task.sleep(for: .milliseconds(5))
        }
        await MainActor.run {
            XCTAssertFalse(io.isRunning, "native graph must stop after \(mode) release")
            XCTAssertGreaterThan(io.stops, 0, "stop reflex must run after \(mode) release")
            XCTAssertTrue(io.stoppedOnMain, "backend stop must run on MainActor after \(mode) release")
            XCTAssertTrue(link.closed, "transport must close after \(mode) release")
            XCTAssertTrue(link.closedOnMain, "transport close must run on MainActor after \(mode) release")
        }
        try await Task.sleep(for: .milliseconds(40))
        await MainActor.run {
            XCTAssertFalse(io.isRunning, "cancelled timer must not restart native I/O")
        }
    }
}
