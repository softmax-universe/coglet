import CogletSurface

/// MainActor, nonblocking control transport. `send(false)` copies nothing; retry
/// the identical message. Socket framing and authenticated peer checks remain in
/// the SDK. No audio callback or guest execution runs through this interface.
@MainActor public protocol FaceControlLink: AnyObject {
    func send(_ bytes: [UInt8]) throws -> Bool
    func receive() throws -> [UInt8]?
    func flush() throws -> Bool
    func close()
}

@MainActor public final class FaceSocketLink: FaceControlLink {
    private let transport: SurfaceSocketTransport
    public init(authenticatedTransport: SurfaceSocketTransport) { transport = authenticatedTransport }
    public func send(_ bytes: [UInt8]) throws -> Bool { try transport.enqueue(bytes); return true }
    public func receive() throws -> [UInt8]? { try transport.receive() }
    public func flush() throws -> Bool { try transport.flush() }
    public func close() { transport.close() }
}

/// The containing app installs its binding in the real runtime before opening
/// the ABI link. This adapter never invents runtime symbols or a fallback peer.
@MainActor public final class FaceEmbeddedLink: FaceControlLink {
    private let transport: EmbeddedSurfaceTransport
    public init(abi: SurfaceRuntimeABI) throws { transport = try EmbeddedSurfaceTransport(abi: abi) }
    public func send(_ bytes: [UInt8]) throws -> Bool { try transport.send(bytes) }
    public func receive() throws -> [UInt8]? { try transport.receive() }
    public func flush() throws -> Bool { true }
    public func close() { transport.close() }
}
