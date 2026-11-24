import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac

private final class HostLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
}
@MainActor private final class HostClock { var now: UInt64 = 0 }

final class FaceHostTests: XCTestCase {
    private static let owner = "coglet:" + String(repeating: "a", count: 64)
    private static let identity = "surface:" + String(repeating: "b", count: 64)
    func testRestartRetainsAdmittedStreamTombstoneWithoutReplayingAudio() async throws {
        try await MainActor.run {
            let clock = HostClock(), store = HostLedger(), io = TestDuplex(), tts = TestSynthesis()
            @MainActor func make() throws -> FaceSurfaceHost {
                try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: store,
                    backend: FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }),
                    grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            }
            let host = try make(); try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            try host.receive(Self.speak(host: host, suffix: "1", chunk: 0, deadline: 1000))
            try host.close()
            let reopened = try make(); try Self.negotiate(reopened); try reopened.activateDuplex(localConsent: true)
            try reopened.receive(Self.speak(host: reopened, suffix: "2", chunk: 0, deadline: 1000))
            let reply = try SurfaceWire.decode(reopened.drain()[0], from: .surface)
            XCTAssertEqual(reply["body"]?["accepted"], .boolean(false))
            XCTAssertEqual(reply["body"]?["error"]?["code"], .text("conflict"))
            XCTAssertEqual(tts.callbacks.count, 1)
            XCTAssertTrue(io.enqueued.isEmpty)
            try reopened.close()
        }
    }
    func testActionExpiryCancelsLaterQueueWithoutStartingAudio() async throws {
        try await MainActor.run {
            let clock = HostClock(), io = TestDuplex(), tts = TestSynthesis()
            let backend = FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in })
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(), backend: backend,
                grants: [.init(coglet: Self.owner, operations: Set(FaceSurfaceHost.operations.map(\.name)), resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { clock.now })
            try Self.negotiate(host); try host.activateDuplex(localConsent: true)
            let first = try Self.speak(host: host, suffix: "1", chunk: 0, deadline: 1000)
            try host.receive(first)
            try host.receive(Self.speak(host: host, suffix: "2", chunk: 1, deadline: 2000))
            _ = host.drain()
            tts.callbacks[0](.success(.init(samples: [1, 2, 3])))
            clock.now = 1000; try host.pump()
            let values = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            let outcomes = values.compactMap { $0["body"]?["payload"] }.filter { $0["status"] != nil }
            XCTAssertEqual(Set(outcomes.compactMap { $0["status"]?.faceText }), ["expired", "cancelled"])
            XCTAssertTrue(io.enqueued.isEmpty)
            try host.receive(first)
            let retries = try host.drain().map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(retries.last?["body"]?["payload"]?["status"], .text("expired"))
            try host.close()
        }
    }
    @MainActor private static func negotiate(_ host: FaceSurfaceHost) throws {
        try host.start()
        let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
        try host.receive(SurfaceWire.encode(.message("hello_reply", .record([
            ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
        _ = host.drain()
    }
    @MainActor private static func speak(host: FaceSurfaceHost, suffix: String, chunk: UInt64, deadline: UInt64) throws -> [UInt8] {
        try SurfaceWire.encode(.message("act", .record([("grant_epoch", .unsigned(host.grantEpoch)), ("action", .record([
            ("id", .text(String(repeating: "0", count: 31) + suffix)), ("coglet", .text(owner)),
            ("capability", .text("speech.speak_chunk")), ("deadline", .unsigned(deadline)),
            ("correlation_id", .text(String(repeating: "f", count: 32))),
            ("args", .record([("stream", .text(String(repeating: "a", count: 32))), ("chunk", .unsigned(chunk)),
                              ("text", .text("hello")), ("final", .boolean(false))]))]))])), from: .runtime)
    }
    func testHostRequiresLocalConsentAndCommonDuplexLeaseBeforeStartingGraph() async throws {
        try await MainActor.run {
            let io = TestDuplex(), backend = FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in })
            let clock = HostClock()
            let operations = Set(FaceSurfaceHost.operations.map(\.name))
            let host = try FaceSurfaceHost(identity: Self.identity, owner: Self.owner, store: HostLedger(), backend: backend,
                grants: [.init(coglet: Self.owner, operations: operations, resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)],
                clock: { clock.now })
            try host.start()
            let hello = try SurfaceWire.decode(host.drain()[0], from: .surface)
            try host.receive(SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])), ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            _ = host.drain()
            XCTAssertThrowsError(try host.activateDuplex(localConsent: false))
            XCTAssertFalse(io.isRunning)
            try host.activateDuplex(localConsent: true)
            XCTAssertTrue(io.isRunning)
            try host.installGrants([.init(coglet: Self.owner, operations: operations, resources: ["audio.speaker", "display.face"], expires: 10_000_000)])
            XCTAssertFalse(io.isRunning)
            XCTAssertThrowsError(try host.activateDuplex(localConsent: true))
            try host.close()
        }
    }
}
