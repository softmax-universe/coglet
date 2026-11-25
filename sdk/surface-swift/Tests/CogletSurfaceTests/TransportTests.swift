import XCTest
import Foundation
import CogletTypes
@testable import CogletSurface
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

private let correlation = String(repeating: "1", count: 32)
private func health() -> CBORValue {
    .message("health", .record([("state", .text("ready")), ("clock", .unsigned(0)),
        ("pending", .unsigned(0)), ("correlation_id", .text(correlation))]))
}
private final class NativeProbe {
    var bytes = try! SurfaceWire.encode(health(), from: .runtime)
    var profile: SurfaceContractProfile = .surface01
    var sent: [[UInt8]] = []
    var blocked = true
    var closed = false
    var queries = 0
    var refusal: Int32?
}
private func probe(_ link: OpaquePointer?) -> NativeProbe {
    Unmanaged<NativeProbe>.fromOpaque(UnsafeRawPointer(link!)).takeUnretainedValue()
}
private func nativeOpen(_ version: UInt32, _ out: UnsafeMutablePointer<OpaquePointer?>?) -> Int32 {
    guard version == 1, let out,
          let state = Thread.current.threadDictionary["coglet-abi-test"] as? NativeProbe else { return -1 }
    out.pointee = OpaquePointer(Unmanaged.passRetained(state).toOpaque())
    return 0
}
private func nativeSend(_ link: OpaquePointer?, _ bytes: UnsafePointer<UInt8>?, _ n: Int) -> Int32 {
    guard let link, let bytes, n > 0 else { return -1 }
    let state = probe(link)
    if state.blocked { state.blocked = false; return 1 }
    let owned = Array(UnsafeBufferPointer(start: bytes, count: n))
    guard (try? SurfaceWire.decode(owned, from: .surface, contractProfile: state.profile)) != nil else { return -2 }
    state.sent.append(owned)
    return 0
}
private func nativeRecv(_ link: OpaquePointer?, _ out: UnsafeMutablePointer<UInt8>?, _ cap: Int, _ n: UnsafeMutablePointer<Int>?) -> Int32 {
    guard let link, let n, cap >= 0, out != nil || cap == 0 else { return -1 }
    let state = probe(link)
    n.pointee = state.bytes.count
    if let refusal = state.refusal { n.pointee = 0; state.queries += 1; return refusal }
    if state.bytes.isEmpty { return 1 }
    if cap < state.bytes.count { state.queries += 1; return 2 }
    out!.update(from: state.bytes, count: state.bytes.count)
    state.bytes = []
    return 0
}
private func nativeClose(_ link: OpaquePointer?) {
    guard let link else { return }
    let state = Unmanaged<NativeProbe>.fromOpaque(UnsafeRawPointer(link)).takeRetainedValue()
    state.closed = true
}

final class TransportTests: XCTestCase {
    func testEmbeddedProfileFollowsActualSelectionNotLocalCeiling() throws {
        let state = NativeProbe()
        Thread.current.threadDictionary["coglet-abi-test"] = state
        defer { Thread.current.threadDictionary.removeObject(forKey: "coglet-abi-test") }
        let abi = SurfaceRuntimeABI(open: nativeOpen, send: nativeSend, recv: nativeRecv, close: nativeClose)
        let owner = "coglet:" + String(repeating: "b", count: 64)
        let action = CBORValue.message("act", .record([("grant_epoch", .unsigned(1)), ("action", .record([
            ("id", .text(correlation)), ("coglet", .text(owner)), ("capability", .text("softmax.app.invoke")),
            ("args", .map([])), ("deadline", .unsigned(100)), ("correlation_id", .text(correlation))]))]))
        let incoming = try SurfaceWire.encode(action, from: .runtime, contractProfile: .surface02)
        let outgoing = try SurfaceWire.encode(.message("grants", .record([
            ("epoch", .unsigned(1)), ("entries", .array([.record([
                ("coglet", .text(owner)), ("operations", .array([.text("softmax.app.invoke")])),
                ("resources", .array([])), ("expires", .unsigned(100))
            ])])), ("correlation_id", .text(correlation))])), from: .surface, contractProfile: .surface02)
        func reply(_ version: String) throws -> [UInt8] {
            try SurfaceWire.encode(.message("hello_reply", .record([
                ("selection", .record([("version", .text(version))])), ("correlation_id", .text(correlation))])), from: .runtime)
        }
        let selected = try EmbeddedSurfaceTransport(abi: abi, contractProfile: .surface02)
        XCTAssertThrowsError(try selected.send(outgoing)) // local ceiling is not negotiation
        let selection = try reply("0.2")
        state.bytes = selection
        XCTAssertEqual(try selected.receive(), selection)
        state.profile = .surface02
        XCTAssertFalse(try selected.send(outgoing)) // identical whole-frame would-block
        XCTAssertTrue(try selected.send(outgoing))
        XCTAssertEqual(state.sent.last, outgoing)
        state.bytes = incoming
        XCTAssertEqual(try selected.receive(), incoming)
        state.bytes = try reply("0.1")
        XCTAssertThrowsError(try selected.receive()) // never switch after first selection
        XCTAssertTrue(state.closed)

        let downgraded = try EmbeddedSurfaceTransport(abi: abi, contractProfile: .surface02)
        state.bytes = try reply("0.1"); _ = try downgraded.receive()
        XCTAssertThrowsError(try downgraded.send(outgoing))
        state.bytes = incoming
        XCTAssertThrowsError(try downgraded.receive())
        let frozen = try EmbeddedSurfaceTransport(abi: abi)
        state.bytes = try reply("0.2")
        XCTAssertThrowsError(try frozen.receive())
    }

    func testSchemaDirectionAndProfile() throws {
        let bytes = try SurfaceWire.encode(health(), from: .runtime)
        XCTAssertEqual(try SurfaceWire.decode(bytes, from: .runtime), try CBORCodec.decodeProfile(bytes))
        XCTAssertThrowsError(try SurfaceWire.decode(bytes + [0], from: .runtime))
        XCTAssertThrowsError(try SurfaceWire.decode([0x40], from: .runtime))
        let kill = CBORValue.message("kill", .record([("reason", .text("stop")), ("correlation_id", .text(correlation))]))
        let killed = try SurfaceWire.encode(kill, from: .surface)
        XCTAssertThrowsError(try SurfaceWire.decode(killed, from: .runtime))
        let unknown = CBORValue.message("health", .record([("state", .text("ready")), ("clock", .unsigned(0)),
            ("pending", .unsigned(0)), ("correlation_id", .text(correlation)), ("extra", .null)]))
        XCTAssertThrowsError(try SurfaceWire.encode(unknown, from: .runtime))
    }
    func testFragmentationCoalescingAndEOF() throws {
        let payload = try SurfaceWire.encode(health(), from: .runtime)
        let framed = try SurfaceWire.frame(payload)
        var decoder = SurfaceFrameDecoder()
        var frames: [[UInt8]] = []
        for byte in framed { try decoder.consume([byte][...]) { frames.append($0) } }
        try decoder.consume((framed + framed)[...]) { frames.append($0) }
        XCTAssertEqual(frames, [payload, payload, payload])
        try decoder.finish()
        XCTAssertThrowsError(try decoder.consume(framed[...]) { _ in })
        for prefix in [Array(framed.prefix(1)), Array(framed.prefix(4)), Array(framed.dropLast())] {
            var truncated = SurfaceFrameDecoder()
            try truncated.consume(prefix[...]) { _ in }
            XCTAssertThrowsError(try truncated.finish())
        }
        for header: [UInt8] in [[0,0,0,0], [0,16,0,1], [255,255,255,255]] {
            var malformed = SurfaceFrameDecoder()
            XCTAssertThrowsError(try malformed.consume(header[...]) { _ in XCTFail("must not dispatch") })
            XCTAssertThrowsError(try malformed.consume(framed[...]) { _ in })
        }
    }
    func testEmbeddedBufferContractAndCopiedWouldBlock() throws {
        let state = NativeProbe()
        Thread.current.threadDictionary["coglet-abi-test"] = state
        defer { Thread.current.threadDictionary.removeObject(forKey: "coglet-abi-test") }
        var unsupported: OpaquePointer?
        XCTAssertEqual(nativeOpen(2, &unsupported), -1)
        XCTAssertNil(unsupported)
        let abi = SurfaceRuntimeABI(open: nativeOpen, send: nativeSend, recv: nativeRecv, close: nativeClose)
        let transport = try EmbeddedSurfaceTransport(abi: abi)
        var bytes = try SurfaceWire.encode(health(), from: .surface)
        let original = bytes
        XCTAssertFalse(try transport.send(bytes))
        XCTAssertTrue(state.sent.isEmpty)
        XCTAssertTrue(try transport.send(bytes))
        bytes[0] = 0
        XCTAssertEqual(state.sent, [original])
        XCTAssertEqual(try transport.receive(), original)
        XCTAssertEqual(state.queries, 1)
        XCTAssertNil(try transport.receive())
        transport.close(); transport.close()
        XCTAssertTrue(state.closed)
        XCTAssertThrowsError(try transport.send(original))
        XCTAssertThrowsError(try transport.receive())
    }
    func testABISizeQueryAndShortRetryDoNotDequeue() throws {
        // Existing size-query/exact-read contract is qualified separately from
        // the refusal diagnostics below.
        let state = NativeProbe()
        Thread.current.threadDictionary["coglet-abi-test"] = state
        defer { Thread.current.threadDictionary.removeObject(forKey: "coglet-abi-test") }
        var link: OpaquePointer?
        XCTAssertEqual(nativeOpen(1, &link), 0)
        defer { nativeClose(link) }
        let original = state.bytes
        var size = 0
        XCTAssertEqual(nativeRecv(link, nil, 0, &size), 2)
        XCTAssertEqual(size, original.count)
        var short = [UInt8](repeating: 0, count: size - 1)
        XCTAssertEqual(short.withUnsafeMutableBufferPointer { nativeRecv(link, $0.baseAddress, $0.count, &size) }, 2)
        XCTAssertEqual(state.bytes, original)
        var exact = [UInt8](repeating: 0, count: size)
        XCTAssertEqual(exact.withUnsafeMutableBufferPointer { nativeRecv(link, $0.baseAddress, $0.count, &size) }, 0)
        XCTAssertEqual(exact, original)
        XCTAssertEqual(nativeRecv(link, nil, 0, &size), 1)
        XCTAssertEqual(size, 0)
    }
    func testEmbeddedQueryRefusalRetainsStatusAndCountWithoutRetry() throws {
        let state = NativeProbe()
        state.refusal = -2
        Thread.current.threadDictionary["coglet-abi-test"] = state
        defer { Thread.current.threadDictionary.removeObject(forKey: "coglet-abi-test") }
        let transport = try EmbeddedSurfaceTransport(abi: .init(open: nativeOpen, send: nativeSend, recv: nativeRecv, close: nativeClose))
        XCTAssertThrowsError(try transport.receive()) { error in
            XCTAssertEqual(error as? SurfaceFailure, .protocolViolation("invalid ABI size query: status=-2 count=0 (before close)"))
        }
        XCTAssertEqual(state.queries, 1)
        XCTAssertTrue(state.closed)
        XCTAssertThrowsError(try transport.receive())
        XCTAssertEqual(state.queries, 1, "failure must close without an additional ABI query")
    }
    func testRealSocketPeerCredentialsDirectionQueueAndClose() throws {
        var pair: [Int32] = [-1, -1]
#if canImport(Darwin)
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
#else
        XCTAssertEqual(socketpair(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0, &pair), 0)
#endif
        defer { _ = close(pair[1]) }
        let transport = try SurfaceSocketTransport(authenticatedDescriptor: pair[0], runtimeUID: getuid())
        defer { transport.close() }
        XCTAssertNil(try transport.receive())
        let bytes = try SurfaceWire.encode(health(), from: .runtime)
        let frame = try SurfaceWire.frame(bytes)
        for byte in frame {
            var value = byte
            XCTAssertEqual(write(pair[1], &value, 1), 1)
        }
        XCTAssertEqual(try transport.receive(), bytes)
        try transport.enqueue(bytes)
        XCTAssertTrue(try transport.flush())
        var buffer = [UInt8](repeating: 0, count: frame.count)
        XCTAssertEqual(read(pair[1], &buffer, buffer.count), frame.count)
        XCTAssertEqual(buffer, frame)
        for _ in 0..<SurfaceWire.maximumQueuedMessages { try transport.enqueue(bytes) }
        XCTAssertThrowsError(try transport.enqueue(bytes))
        XCTAssertThrowsError(try transport.flush())
    }
    func testWrongUIDIsNotHelloAuthentication() throws {
        var pair: [Int32] = [-1, -1]
#if canImport(Darwin)
        XCTAssertEqual(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair), 0)
#else
        XCTAssertEqual(socketpair(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0, &pair), 0)
#endif
        defer { _ = close(pair[0]); _ = close(pair[1]) }
        XCTAssertThrowsError(try SurfaceSocketTransport(authenticatedDescriptor: pair[0], runtimeUID: getuid() &+ 1))
    }
    func testPrivateListenerOwnershipAndNoUnlinkOfExistingDirectory() throws {
        let base = ProcessInfo.processInfo.environment["TMPDIR"] ?? FileManager.default.temporaryDirectory.path
        let dir = base + "/coglet-swift-" + UUID().uuidString.prefix(8)
        let listener = try SurfaceSocketListener(directory: dir, runtimeUID: getuid())
        let directoryMode = try FileManager.default.attributesOfItem(atPath: dir)[.posixPermissions] as! NSNumber
        let socketMode = try FileManager.default.attributesOfItem(atPath: listener.path)[.posixPermissions] as! NSNumber
        XCTAssertEqual(directoryMode.intValue & 0o777, 0o700)
        XCTAssertEqual(socketMode.intValue & 0o777, 0o600)
        XCTAssertThrowsError(try SurfaceSocketListener(directory: dir, runtimeUID: getuid()))
        XCTAssertTrue(FileManager.default.fileExists(atPath: listener.path))
        XCTAssertNil(try listener.acceptConnection())
        listener.close()
        XCTAssertFalse(FileManager.default.fileExists(atPath: dir))
    }
}
