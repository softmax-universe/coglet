import XCTest
import CogletSurface
import CogletTypes
@testable import FaceMac
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

private final class LinkLedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
}

final class FaceSocketTests: XCTestCase {
    func testRealAuthenticatedSocketDrivesFaceCaptureAndFragmentedFramesThenFatalDisconnect() async throws {
        try await MainActor.run {
            var pair: [Int32] = [-1, -1]
#if canImport(Darwin)
            XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
#else
            XCTAssertEqual(socketpair(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0, &pair), 0)
#endif
            let peer = pair[1]
            defer { _ = close(peer) }
            XCTAssertEqual(fcntl(peer, F_SETFL, O_NONBLOCK), 0)
            let transport = try SurfaceSocketTransport(authenticatedDescriptor: pair[0], runtimeUID: getuid())
            let io = TestDuplex(), recognition = TestRecognition(), tts = TestSynthesis()
            let owner = "coglet:" + String(repeating: "a", count: 64)
            let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "b", count: 64), owner: owner,
                store: LinkLedger(), backend: FaceSurfaceBackend(audio: io, synthesis: tts, renderer: .init { _ in }),
                grants: [.init(coglet: owner, operations: Set(FaceSurfaceHost.operations.map(\.name)),
                    resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: FaceSocketLink(authenticatedTransport: transport))
            try host.start()
            var decoder = SurfaceFrameDecoder()
            func receive() throws -> [CBORValue] {
                var buffer = [UInt8](repeating: 0, count: 65_536)
                let count = buffer.withUnsafeMutableBytes { recv(peer, $0.baseAddress, $0.count, 0) }
                guard count > 0 else { return [] }
                var messages: [CBORValue] = []
                try decoder.consume(buffer.prefix(count)) { messages.append(try SurfaceWire.decode($0, from: .surface)) }
                return messages
            }
            func sendBytes(_ bytes: [UInt8]) {
                let written = bytes.withUnsafeBytes { send(peer, $0.baseAddress, $0.count, 0) }
                XCTAssertEqual(written, bytes.count)
            }
            let hello = try XCTUnwrap(receive().first)
            let reply = try SurfaceWire.frame(SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])),
                ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            sendBytes(Array(reply.prefix(2))); try host.pump()
            XCTAssertTrue(try receive().isEmpty, "partial frame must not negotiate")
            sendBytes(Array(reply.dropFirst(2))); try host.pump()
            XCTAssertFalse(try receive().isEmpty)
            try host.activateDuplex(localConsent: true)
            let item = String(repeating: "1", count: 32), stream = String(repeating: "2", count: 32)
            let action = try SurfaceWire.encode(.message("act", .record([
                ("grant_epoch", .unsigned(host.grantEpoch)), ("action", .record([
                    ("id", .text(item)), ("coglet", .text(owner)), ("capability", .text("speech.speak_chunk")),
                    ("deadline", .unsigned(1_000_000)), ("correlation_id", .text(item)),
                    ("args", .record([("stream", .text(stream)), ("chunk", .unsigned(0)),
                        ("text", .text("hello")), ("final", .boolean(true))]))]))])), from: .runtime)
            sendBytes(try SurfaceWire.frame(action)); try host.pump()
            XCTAssertEqual(try receive().first?["body"]?["accepted"], .boolean(true))
            tts.callbacks[0](.success(.init(samples: [Int16](repeating: 1, count: 2400), spans: [.init(endMS: 100, text: "hello")])))
            try host.pump(); _ = try receive()
            io.renderedFrames = 1200
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try recognition.callbacks[0](.init(revision: 1, text: "socket final", final: true, positionMS: 100))
            try host.pump()
            let events = try receive()
            XCTAssertEqual(events.map { $0["body"]?["kind"] }, [.text("speech.start"), .text("speech.barge_in"),
                .text("speech.position"), .text("action.outcome"), .text("speech.end")])
            XCTAssertEqual(events[1]["body"]?["payload"]?["item"], .text(item))
            XCTAssertEqual(events[1]["body"]?["payload"]?["played_ms"], .unsigned(50))
            XCTAssertEqual(events[3]["body"]?["payload"]?["status"], .text("cancelled"))
            XCTAssertEqual(events.last?["body"]?["payload"]?["text"], .text("socket final"))
            // Oversized length is a fatal framing error, not a dropped message.
            sendBytes([0xff, 0xff, 0xff, 0xff])
            XCTAssertThrowsError(try host.pump())
            XCTAssertFalse(io.isRunning)
            XCTAssertTrue(recognition.inputs[0].cancelled)
            try host.close()
        }
    }
}
