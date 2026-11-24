#if DEBUG
import Foundation
import Darwin

/// Local development ONLY. Never reads or migrates the production Keychain.
/// Not encrypted by Keychain; use only disposable development enrollments.
@MainActor
public final class DevelopmentHubStore: HubStore {
    private let directory: URL
    public init(directory: URL = FileManager.default.homeDirectoryForCurrentUser
        .appendingPathComponent(".softmax-development-hub", isDirectory: true)) {
        self.directory = directory
    }
    private func directoryFD() throws -> Int32 {
        if mkdir(directory.path, 0o700) != 0 && errno != EEXIST { throw HubError.storage }
        let fd = open(directory.path, O_RDONLY | O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw HubError.storage }
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_uid == getuid(),
              info.st_mode & 0o777 == 0o700 else { close(fd); throw HubError.storage }
        return fd
    }
    private func read(_ name: String) throws -> Data? {
        let dir = try directoryFD(); defer { close(dir) }
        let fd = openat(dir, name, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        if fd < 0 && errno == ENOENT { return nil }
        guard fd >= 0 else { throw HubError.storage }
        defer { close(fd) }
        try validate(fd)
        var data = Data()
        var buffer = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = Darwin.read(fd, &buffer, buffer.count)
            guard count >= 0 else { throw HubError.storage }
            if count == 0 { return data }
            data.append(contentsOf: buffer.prefix(count))
            guard data.count <= 65536 else { throw HubError.storage }
        }
    }
    private func validate(_ fd: Int32) throws {
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_uid == getuid(),
              info.st_mode & S_IFMT == S_IFREG, info.st_mode & 0o777 == 0o600,
              info.st_nlink == 1 else { throw HubError.storage }
    }
    private func write(_ data: Data, _ name: String, addOnly: Bool) throws {
        let dir = try directoryFD(); defer { close(dir) }
        let flags = O_WRONLY | O_CREAT | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC | (addOnly ? O_EXCL : 0)
        let fd = openat(dir, name, flags, 0o600)
        guard fd >= 0 else { throw HubError.storage }
        defer { close(fd) }
        try validate(fd)
        guard ftruncate(fd, 0) == 0 else { throw HubError.storage }
        try data.withUnsafeBytes { bytes in
            var offset = 0
            while offset < bytes.count {
                let count = Darwin.write(fd, bytes.baseAddress!.advanced(by: offset), bytes.count - offset)
                guard count > 0 else { throw HubError.storage }
                offset += count
            }
        }
        guard fsync(fd) == 0 else { throw HubError.storage }
    }
    private func remove(_ name: String) throws {
        let dir = try directoryFD(); defer { close(dir) }
        if unlinkat(dir, name, 0) != 0 && errno != ENOENT { throw HubError.storage }
    }
    public func loadKey() throws -> Data? { try read("identity") }
    public func saveKey(_ value: Data) throws { try write(value, "identity", addOnly: true) }
    public func deleteKey() throws { try remove("identity") }
    public func loadHub() throws -> HubPairing? {
        guard let data = try read("enrollment.json") else { return nil }
        return try HubPairing.parse(String(decoding: data, as: UTF8.self)).withoutInvite
    }
    public func saveHub(_ value: HubPairing?) throws {
        if let value { try write(JSONEncoder().encode(value.withoutInvite), "enrollment.json", addOnly: false) }
        else { try remove("enrollment.json") }
    }
}
#endif
