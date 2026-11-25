import XCTest
import Foundation
import CogletTypes
@testable import CogletSurface
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

private let socketCorrelation = String(repeating: "1", count: 32)
private let socketOwner = "coglet:" + String(repeating: "b", count: 64)

// A credential-authenticated native peer, not a runtime implementation. Both
// descriptors remain nonblocking; poll and a whole-operation deadline bound I/O.
private final class ProfileSocketPeer {
    let transport: SurfaceSocketTransport
    let descriptor: Int32

    init(profile: SurfaceContractProfile? = nil) throws {
        var pair: [Int32] = [-1, -1]
#if canImport(Darwin)
        let result = socketpair(AF_UNIX, SOCK_STREAM, 0, &pair)
#else
        let result = socketpair(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0, &pair)
#endif
        guard result == 0 else { throw SurfaceFailure.system(errno) }
        do {
            guard fcntl(pair[1], F_SETFL, O_NONBLOCK) == 0 else { throw SurfaceFailure.system(errno) }
#if canImport(Darwin)
            var enabled: Int32 = 1
            guard setsockopt(pair[1], SOL_SOCKET, SO_NOSIGPIPE, &enabled,
                             socklen_t(MemoryLayout<Int32>.size)) == 0 else { throw SurfaceFailure.system(errno) }
#endif
            if let profile {
                transport = try SurfaceSocketTransport(authenticatedDescriptor: pair[0], runtimeUID: getuid(), contractProfile: profile)
            } else {
                // Deliberately exercise the public frozen default, not explicit 0.1.
                transport = try SurfaceSocketTransport(authenticatedDescriptor: pair[0], runtimeUID: getuid())
            }
            descriptor = pair[1]
        } catch {
            _ = close(pair[0]); _ = close(pair[1])
            throw error
        }
    }

    deinit { transport.close(); _ = close(descriptor) }

    private func awaitReady(_ events: Int16, until deadline: UInt64) throws {
        guard DispatchTime.now().uptimeNanoseconds < deadline else { throw SurfaceFailure.exhausted }
        var item = pollfd(fd: descriptor, events: events, revents: 0)
        let result = poll(&item, 1, 20)
        guard result >= 0 || errno == EINTR else { throw SurfaceFailure.system(errno) }
    }

    func sendCoalesced(_ payloads: [[UInt8]]) throws {
        let bytes = try payloads.flatMap { try SurfaceWire.frame($0) }
        // These fixtures fit in one native send and one transport read, making
        // selection plus subsequent frame validation exercise the coalesced path.
        XCTAssertLessThan(bytes.count, 4096)
        let count = bytes.withUnsafeBytes { buffer -> Int in
#if canImport(Darwin)
            Darwin.send(descriptor, buffer.baseAddress, buffer.count, 0)
#else
            Glibc.send(descriptor, buffer.baseAddress, buffer.count, Int32(MSG_NOSIGNAL))
#endif
        }
        guard count == bytes.count else { throw SurfaceFailure.system(errno) }
    }

    func readExactly(_ count: Int) throws -> [UInt8] {
        let deadline = DispatchTime.now().uptimeNanoseconds + 1_000_000_000
        var result: [UInt8] = []
        while result.count < count {
            try awaitReady(Int16(POLLIN), until: deadline)
            var bytes = [UInt8](repeating: 0, count: count - result.count)
            let n = bytes.withUnsafeMutableBytes { recv(descriptor, $0.baseAddress, $0.count, 0) }
            if n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) { continue }
            guard n > 0 else { throw SurfaceFailure.closed }
            result.append(contentsOf: bytes.prefix(n))
        }
        return result
    }

    func assertTerminalClose(file: StaticString = #filePath, line: UInt = #line) throws {
        XCTAssertThrowsError(try transport.receive(), file: file, line: line)
        XCTAssertThrowsError(try transport.flush(), file: file, line: line)
        let deadline = DispatchTime.now().uptimeNanoseconds + 1_000_000_000
        while true {
            try awaitReady(Int16(POLLIN), until: deadline)
            var byte: UInt8 = 0
            let n = recv(descriptor, &byte, 1, 0)
            if n < 0 && (errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR) { continue }
            XCTAssertEqual(n, 0, "native peer must observe EOF", file: file, line: line)
            return
        }
    }
}

private func socketSelection(_ version: String) throws -> [UInt8] {
    try SurfaceWire.encode(.message("hello_reply", .record([
        ("selection", .record([("version", .text(version))])),
        ("correlation_id", .text(socketCorrelation))])), from: .runtime)
}

private func socketAppAction() throws -> [UInt8] {
    try SurfaceWire.encode(.message("act", .record([
        ("grant_epoch", .unsigned(1)), ("action", .record([
            ("id", .text(socketCorrelation)), ("coglet", .text(socketOwner)),
            ("capability", .text("softmax.app.invoke")), ("args", .map([])),
            ("deadline", .unsigned(100)), ("correlation_id", .text(socketCorrelation))]))])),
        from: .runtime, contractProfile: .surface02)
}

private func socketAppGrants() throws -> [UInt8] {
    try SurfaceWire.encode(.message("grants", .record([
        ("epoch", .unsigned(1)), ("entries", .array([.record([
            ("coglet", .text(socketOwner)), ("operations", .array([.text("softmax.app.invoke")])),
            ("resources", .array([])), ("expires", .unsigned(100))])])),
        ("correlation_id", .text(socketCorrelation))])), from: .surface, contractProfile: .surface02)
}

final class SocketProfileTests: XCTestCase {
    func testSelected02AcceptsCoalescedReplyAndAppActionAndWritesAppGrants() throws {
        let peer = try ProfileSocketPeer(profile: .surface02)
        let selection = try socketSelection("0.2")
        let action = try socketAppAction()
        try peer.sendCoalesced([selection, action])
        XCTAssertEqual(try peer.transport.receive(), selection)
        XCTAssertEqual(try peer.transport.receive(), action)
        XCTAssertNil(try peer.transport.receive())
        let grants = try socketAppGrants()
        try peer.transport.enqueue(grants)
        XCTAssertTrue(try peer.transport.flush())
        let frame = try SurfaceWire.frame(grants)
        let received = try peer.readExactly(frame.count)
        XCTAssertEqual(received, frame)
        XCTAssertEqual(try SurfaceWire.decode(Array(received.dropFirst(4)), from: .surface,
                                              contractProfile: .surface02),
                       try SurfaceWire.decode(grants, from: .surface, contractProfile: .surface02))
    }

    func testLocal02RejectsOutgoingAppNamesBeforeNegotiation() throws {
        let peer = try ProfileSocketPeer(profile: .surface02)
        XCTAssertThrowsError(try peer.transport.enqueue(socketAppGrants()))
        XCTAssertTrue(try peer.transport.flush())
        XCTAssertNil(try peer.transport.receive())
    }

    func testLocal02RejectsIncomingAppNamesBeforeNegotiationAndCloses() throws {
        let peer = try ProfileSocketPeer(profile: .surface02)
        try peer.sendCoalesced([socketAppAction()])
        XCTAssertThrowsError(try peer.transport.receive())
        try peer.assertTerminalClose()
    }

    func testNegotiated01RejectsAppNamesDespiteLocal02Ceiling() throws {
        let peer = try ProfileSocketPeer(profile: .surface02)
        let selection = try socketSelection("0.1")
        try peer.sendCoalesced([selection])
        XCTAssertEqual(try peer.transport.receive(), selection)
        XCTAssertThrowsError(try peer.transport.enqueue(socketAppGrants()))
        try peer.sendCoalesced([socketAppAction()])
        XCTAssertThrowsError(try peer.transport.receive())
        try peer.assertTerminalClose()
    }

    func testFrozenDefaultRefuses02SelectionAndCloses() throws {
        let peer = try ProfileSocketPeer()
        try peer.sendCoalesced([socketSelection("0.2")])
        XCTAssertThrowsError(try peer.transport.receive())
        try peer.assertTerminalClose()
    }

    func testCoalescedChangingSelectionFailsClosedWithoutDeliveringQueuedFrames() throws {
        let peer = try ProfileSocketPeer(profile: .surface02)
        try peer.sendCoalesced([socketSelection("0.2"), socketSelection("0.1"), socketAppAction()])
        // The whole coalesced read fails: neither the first selection nor later
        // app bytes may leak out of the failed receive's temporary frame queue.
        XCTAssertThrowsError(try peer.transport.receive())
        try peer.assertTerminalClose()
    }
}
