import Foundation
import CogletTypes
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

private func streamSocket() -> Int32 {
#if canImport(Darwin)
    return socket(AF_UNIX, SOCK_STREAM, 0)
#else
    return socket(AF_UNIX, Int32(SOCK_STREAM.rawValue), 0)
#endif
}
private func closeFD(_ fd: Int32) {
#if canImport(Darwin)
    _ = Darwin.close(fd)
#else
    _ = Glibc.close(fd)
#endif
}
private func systemError() -> SurfaceFailure { .system(errno) }

/// A surface-owned, user-private Unix socket. Construct in a NEW directory;
/// refuses existing directories/sockets rather than unlinking another service.
/// Trust is established from OS credentials, never a hello identity string.
/// Calls are serialized by the application's non-audio control queue.
public final class SurfaceSocketListener {
    private var descriptor: Int32 = -1
    private let directory: String
    public let path: String
    private let runtimeUID: uid_t
    private let contractProfile: SurfaceContractProfile
    private var ownsDirectory = false
    public init(directory: String, runtimeUID: uid_t, contractProfile: SurfaceContractProfile = .surface01) throws {
        self.directory = directory
        self.path = directory + "/surface.sock"
        self.runtimeUID = runtimeUID
        self.contractProfile = contractProfile
        guard mkdir(directory, 0o700) == 0 else { throw systemError() }
        ownsDirectory = true
        do {
            var address = sockaddr_un()
            address.sun_family = sa_family_t(AF_UNIX)
            let bytes = Array(path.utf8) + [0]
            guard bytes.count <= MemoryLayout.size(ofValue: address.sun_path) else { throw SurfaceFailure.invalidFrame }
            withUnsafeMutableBytes(of: &address.sun_path) { $0.copyBytes(from: bytes) }
            let fd = streamSocket()
            guard fd >= 0 else { throw systemError() }
            descriptor = fd
            guard fcntl(fd, F_SETFL, O_NONBLOCK) == 0,
                  fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw systemError() }
            // The directory is private before bind, so no permissive access window.
            let result = withUnsafePointer(to: &address) { pointer in
                pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                    bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
                }
            }
            guard result == 0, chmod(path, 0o600) == 0, listen(fd, 1) == 0 else { throw systemError() }
        } catch { close(); throw error }
    }
    deinit { close() }
    public func close() {
        if descriptor >= 0 { closeFD(descriptor); descriptor = -1 }
        if ownsDirectory {
            _ = unlink(path); _ = rmdir(directory); ownsDirectory = false
        }
    }
    public func acceptConnection() throws -> SurfaceSocketTransport? {
        guard descriptor >= 0 else { throw SurfaceFailure.closed }
        let fd = accept(descriptor, nil, nil)
        if fd < 0 {
            if errno == EAGAIN || errno == EWOULDBLOCK || errno == EINTR { return nil }
            throw systemError()
        }
        do { return try SurfaceSocketTransport(authenticatedDescriptor: fd, runtimeUID: runtimeUID, contractProfile: contractProfile) }
        catch { closeFD(fd); throw error }
    }
}

/// Nonblocking, bounded transport. enqueue() copies the immutable CBOR; flush()
/// never restarts a partially written frame. Connection failure is terminal;
/// the owning surface must immediately apply its disconnect/native-stop path.
public final class SurfaceSocketTransport {
    private var descriptor: Int32
    private var outgoing: [[UInt8]] = []
    private var offset = 0
    private var incoming: [[UInt8]] = []
    private var decoder = SurfaceFrameDecoder()
    private var version: SurfaceTransportVersion
    public init(authenticatedDescriptor fd: Int32, runtimeUID: uid_t, contractProfile: SurfaceContractProfile = .surface01) throws {
        version = SurfaceTransportVersion(contractProfile)
#if canImport(Darwin)
        var uid: uid_t = 0, gid: gid_t = 0
        guard getpeereid(fd, &uid, &gid) == 0, uid == runtimeUID else { throw SurfaceFailure.unauthenticated }
        var enabled: Int32 = 1
        guard setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &enabled, socklen_t(MemoryLayout<Int32>.size)) == 0 else { throw systemError() }
#else
        struct Credentials { var pid: Int32 = 0; var uid: uid_t = 0; var gid: gid_t = 0 }
        var credentials = Credentials()
        var size = socklen_t(MemoryLayout<Credentials>.size)
        guard getsockopt(fd, SOL_SOCKET, SO_PEERCRED, &credentials, &size) == 0,
              size == MemoryLayout<Credentials>.size, credentials.uid == runtimeUID else { throw SurfaceFailure.unauthenticated }
#endif
        guard fcntl(fd, F_SETFL, O_NONBLOCK) == 0, fcntl(fd, F_SETFD, FD_CLOEXEC) == 0 else { throw systemError() }
        descriptor = fd
    }
    deinit { close() }
    public func close() {
        if descriptor >= 0 { closeFD(descriptor); descriptor = -1 }
        incoming = []; outgoing = []; offset = 0
    }
    public func enqueue(_ bytes: [UInt8]) throws {
        guard descriptor >= 0 else { throw SurfaceFailure.closed }
        try version.validateOutgoing(bytes)
        guard outgoing.count < SurfaceWire.maximumQueuedMessages else {
            close(); throw SurfaceFailure.exhausted
        }
        outgoing.append(try SurfaceWire.frame(bytes))
    }
    /// true when all queued bytes have been written; not an action acceptance.
    public func flush() throws -> Bool {
        guard descriptor >= 0 else { throw SurfaceFailure.closed }
        while let frame = outgoing.first {
            let sent = frame.withUnsafeBytes { buffer -> Int in
#if canImport(Darwin)
                Darwin.send(descriptor, buffer.baseAddress!.advanced(by: offset), frame.count - offset, 0)
#else
                Glibc.send(descriptor, buffer.baseAddress!.advanced(by: offset), frame.count - offset, Int32(MSG_NOSIGNAL))
#endif
            }
            if sent < 0 {
                if errno == EINTR { continue }
                if errno == EAGAIN || errno == EWOULDBLOCK { return false }
                let error = systemError(); close(); throw error
            }
            guard sent > 0 else { close(); throw SurfaceFailure.closed }
            offset += sent
            if offset == frame.count { outgoing.removeFirst(); offset = 0 }
        }
        return true
    }
    public func receive() throws -> [UInt8]? {
        guard descriptor >= 0 else { throw SurfaceFailure.closed }
        if !incoming.isEmpty { return incoming.removeFirst() }
        var bytes = [UInt8](repeating: 0, count: 4096)
        let count = bytes.withUnsafeMutableBytes { recv(descriptor, $0.baseAddress, $0.count, 0) }
        if count < 0 {
            if errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK { return nil }
            let error = systemError(); close(); throw error
        }
        do {
            guard count > 0 else {
                try decoder.finish()
                throw SurfaceFailure.closed
            }
            var decoded: [[UInt8]] = []
            try decoder.consume(bytes.prefix(count)) { frame in
                try version.validateIncoming(frame)
                guard decoded.count < SurfaceWire.maximumQueuedMessages else { throw SurfaceFailure.exhausted }
                decoded.append(frame)
            }
            incoming = decoded
            return incoming.isEmpty ? nil : incoming.removeFirst()
        } catch { close(); throw error }
    }
}
