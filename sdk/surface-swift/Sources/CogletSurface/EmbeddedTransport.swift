import Foundation
import CogletTypes

/// Exact ABI-v1 signatures. The application supplies linked runtime symbols;
/// this package neither loads code nor supplies a fake runtime implementation.
public struct SurfaceRuntimeABI {
    public typealias Open = @convention(c) (UInt32, UnsafeMutablePointer<OpaquePointer?>?) -> Int32
    public typealias Send = @convention(c) (OpaquePointer?, UnsafePointer<UInt8>?, Int) -> Int32
    public typealias Recv = @convention(c) (OpaquePointer?, UnsafeMutablePointer<UInt8>?, Int, UnsafeMutablePointer<Int>?) -> Int32
    public typealias Close = @convention(c) (OpaquePointer?) -> Void
    public let open: Open
    public let send: Send
    public let recv: Recv
    public let close: Close
    public init(open: @escaping Open, send: @escaping Send, recv: @escaping Recv, close: @escaping Close) {
        self.open = open; self.send = send; self.recv = recv; self.close = close
    }
}

/// Caller serializes all operations on a non-audio control queue, including
/// close. The independent native off-switch must never wait on this queue.
/// Binding MUST be installed in the runtime before constructing this adapter.
public final class EmbeddedSurfaceTransport {
    private let abi: SurfaceRuntimeABI
    private var handle: OpaquePointer?
    public init(abi: SurfaceRuntimeABI) throws {
        self.abi = abi
        var link: OpaquePointer?
        let result = abi.open(1, &link)
        guard result == 0, let link else { throw SurfaceFailure.protocolViolation("ABI open failed: \(result)") }
        handle = link
    }
    deinit { close() }
    public func close() {
        if let link = handle { handle = nil; abi.close(link) }
    }
    /// false means would-block: the runtime copied nothing. Retry identical bytes.
    public func send(_ bytes: [UInt8]) throws -> Bool {
        guard let link = handle else { throw SurfaceFailure.closed }
        _ = try SurfaceWire.decode(bytes, from: .surface)
        let result = bytes.withUnsafeBufferPointer { abi.send(link, $0.baseAddress, $0.count) }
        switch result {
        case 0: return true
        case 1: return false
        default: close(); throw SurfaceFailure.protocolViolation("ABI send failed: \(result)")
        }
    }
    /// nil means would-block. Uses the size query without dequeue, followed by an
    /// exact-sized read; all returned bytes are copied to caller-owned storage.
    public func receive() throws -> [UInt8]? {
        guard let link = handle else { throw SurfaceFailure.closed }
        var count = 0
        let query = abi.recv(link, nil, 0, &count)
        if query == 1, count == 0 { return nil }
        guard query == 2, count > 0, count <= SurfaceWire.maximumFrame else {
            close(); throw SurfaceFailure.protocolViolation("invalid ABI size query")
        }
        var bytes = [UInt8](repeating: 0, count: count)
        let capacity = count
        let result = bytes.withUnsafeMutableBufferPointer { abi.recv(link, $0.baseAddress, capacity, &count) }
        guard result == 0, count == capacity else {
            close(); throw SurfaceFailure.protocolViolation("invalid ABI exact read")
        }
        do { _ = try SurfaceWire.decode(bytes, from: .runtime) }
        catch { close(); throw error }
        return bytes
    }
}
