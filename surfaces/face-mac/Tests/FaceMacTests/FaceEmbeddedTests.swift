import XCTest
import Foundation
import CogletSurface
import CogletTypes
@testable import FaceMac

// ABI wrapper fixture only: deliberately NOT the coglet-runtime implementation.
private final class ABIState {
    var sent: [[UInt8]] = []
    var incoming: [[UInt8]] = []
    var blocked = true
    var closed = false
}
private func state(_ pointer: OpaquePointer) -> ABIState {
    Unmanaged<ABIState>.fromOpaque(UnsafeRawPointer(pointer)).takeUnretainedValue()
}
private func openABI(_ version: UInt32, _ out: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 {
    guard version == 1, let out, let state = Thread.current.threadDictionary["face-abi-fixture"] as? ABIState else { return -1 }
    out.pointee = OpaquePointer(Unmanaged.passRetained(state).toOpaque()); return 0
}
private func sendABI(_ pointer: OpaquePointer?, _ bytes: UnsafePointer<UInt8>?, _ count: Int) -> Int32 {
    guard let pointer, let bytes, count > 0 else { return -1 }
    let state = state(pointer)
    guard !state.blocked else { return 1 }
    state.sent.append(Array(UnsafeBufferPointer(start: bytes, count: count))); return 0
}
private func recvABI(_ pointer: OpaquePointer?, _ bytes: UnsafeMutablePointer<UInt8>?, _ capacity: Int,
                     _ size: UnsafeMutablePointer<Int>?) -> Int32 {
    guard let pointer, let size else { return -1 }
    let state = state(pointer)
    guard let message = state.incoming.first else { size.pointee = 0; return 1 }
    size.pointee = message.count
    guard capacity >= message.count, let bytes else { return 2 }
    bytes.update(from: message, count: message.count); state.incoming.removeFirst(); return 0
}
private func closeABI(_ pointer: OpaquePointer?) {
    guard let pointer else { return }
    Unmanaged<ABIState>.fromOpaque(UnsafeRawPointer(pointer)).takeRetainedValue().closed = true
}
private final class ABILedger: SurfaceLedgerStore {
    var bytes: [UInt8] = [0xa0]
    func load() throws -> [UInt8] { bytes }
    func save(_ bytes: [UInt8]) throws { self.bytes = bytes }
}

final class FaceEmbeddedTests: XCTestCase {
    func testEmbeddedABIWrapperDrivesRealFaceHostWithWouldBlockAndNativeOffSwitch() async throws {
        try await MainActor.run {
            let fixture = ABIState()
            Thread.current.threadDictionary["face-abi-fixture"] = fixture
            defer { Thread.current.threadDictionary.removeObject(forKey: "face-abi-fixture") }
            let link = try FaceEmbeddedLink(abi: .init(open: openABI, send: sendABI, recv: recvABI, close: closeABI))
            let owner = "coglet:" + String(repeating: "a", count: 64), io = TestDuplex(), recognition = TestRecognition()
            let host = try FaceSurfaceHost(identity: "surface:" + String(repeating: "b", count: 64), owner: owner,
                store: ABILedger(), backend: FaceSurfaceBackend(audio: io, synthesis: TestSynthesis(), renderer: .init { _ in }),
                grants: [.init(coglet: owner, operations: Set(FaceSurfaceHost.operations.map(\.name)),
                    resources: ["audio.mic", "audio.speaker", "display.face"], expires: 10_000_000)], clock: { 0 })
            try host.attach(link: link); try host.start()
            XCTAssertTrue(fixture.sent.isEmpty)
            fixture.blocked = false; try host.pump()
            XCTAssertEqual(fixture.sent.count, 1)
            let hello = try SurfaceWire.decode(fixture.sent[0], from: .surface)
            fixture.incoming.append(try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text("0.1"))])),
                ("correlation_id", hello["body"]!["correlation_id"]!)])), from: .runtime))
            try host.pump(); fixture.sent = []
            try host.activateDuplex(localConsent: true)
            _ = try host.beginCapture(recognition: recognition, positionMS: 0)
            try recognition.callbacks[0](.init(revision: 1, text: "embedded final", final: true, positionMS: 10))
            try host.pump()
            let events = try fixture.sent.map { try SurfaceWire.decode($0, from: .surface) }
            XCTAssertEqual(events.map { $0["body"]?["kind"] }, [.text("speech.start"), .text("speech.end")])
            XCTAssertEqual(events.last?["body"]?["payload"]?["text"], .text("embedded final"))
            try host.localOffSwitch()
            XCTAssertTrue(fixture.closed); XCTAssertFalse(io.isRunning)
            XCTAssertTrue(recognition.inputs[0].cancelled)
            try host.close()
        }
    }
}
