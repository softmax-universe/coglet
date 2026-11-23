import Foundation
import CogletTypes
#if canImport(Darwin)
import Darwin
#else
import Glibc
#endif

/// A snapshot is committed before receipts or native effects are released. Store
/// implementations must make save durable atomically, or throw. Never fall back
/// to an empty store on corruption/loss. Records are retained indefinitely (a
/// safe strengthening of the 24h minimum); a full ledger refuses new IDs.
public protocol SurfaceLedgerStore: AnyObject {
    func load() throws -> [UInt8]
    func save(_ bytes: [UInt8]) throws
}

/// The supervisor provisions a new private directory once per NEW identity.
/// Reopening an identity requires the existing file. Lost files are fatal.
public final class FileSurfaceLedger: SurfaceLedgerStore {
    private let url: URL
    public init(directory: URL, newIdentity: Bool = false) throws {
        url = directory.appendingPathComponent("ledger.cbor")
        if newIdentity {
            guard mkdir(directory.path, 0o700) == 0 else { throw SurfaceFailure.system(errno) }
            do { try save(try CBORCodec.encodeProfile(.map([]))) }
            catch { try? FileManager.default.removeItem(at: directory); throw error }
        }
        var info = stat()
        guard lstat(directory.path, &info) == 0,
              info.st_mode & mode_t(S_IFMT) == mode_t(S_IFDIR),
              info.st_uid == getuid(), info.st_mode & 0o777 == 0o700 else { throw SurfaceFailure.unauthenticated }
        guard FileManager.default.fileExists(atPath: url.path) else {
            throw SurfaceFailure.protocolViolation("lost ledger: reconcile and rotate surface identity")
        }
    }
    public func load() throws -> [UInt8] {
        var info = stat()
        guard lstat(url.path, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
              info.st_uid == getuid(), info.st_mode & 0o777 == 0o600,
              info.st_size > 0, info.st_size <= 268_435_456 else { throw SurfaceFailure.unauthenticated }
        return Array(try Data(contentsOf: url))
    }
    public func save(_ bytes: [UInt8]) throws {
        // Data.atomic alone does not promise crash durability. fsync the temporary
        // file BEFORE rename and the directory AFTER rename.
        let temporary = url.deletingLastPathComponent().appendingPathComponent(".ledger-" + UUID().uuidString)
        let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL | O_CLOEXEC, 0o600)
        guard fd >= 0 else { throw SurfaceFailure.system(errno) }
        defer {
#if canImport(Darwin)
            _ = Darwin.close(fd)
#else
            _ = Glibc.close(fd)
#endif
            _ = unlink(temporary.path)
        }
        try bytes.withUnsafeBytes { buffer in
            var offset = 0
            while offset < bytes.count {
                let n = write(fd, buffer.baseAddress!.advanced(by: offset), bytes.count - offset)
                if n < 0, errno == EINTR { continue }
                guard n > 0 else { throw SurfaceFailure.system(errno) }
                offset += n
            }
        }
        guard fsync(fd) == 0, rename(temporary.path, url.path) == 0 else { throw SurfaceFailure.system(errno) }
        let parent = open(url.deletingLastPathComponent().path, O_RDONLY | O_CLOEXEC)
        guard parent >= 0 else { throw SurfaceFailure.system(errno) }
        defer {
#if canImport(Darwin)
            _ = Darwin.close(parent)
#else
            _ = Glibc.close(parent)
#endif
        }
        guard fsync(parent) == 0 else { throw SurfaceFailure.system(errno) }
    }
}

public struct SurfaceActionRecord {
    public let envelope: [UInt8]
    public let accept: CBORValue
    public var outcome: CBORValue?
    init(envelope: [UInt8], accept: CBORValue, outcome: CBORValue? = nil) {
        self.envelope = envelope; self.accept = accept; self.outcome = outcome
    }
}

/// Internal durable state; not a wire schema and not a second authority catalog.
final class SurfaceLedger {
    let store: any SurfaceLedgerStore
    let identity: String
    var epoch: UInt64 = 0
    var sequence: UInt64 = 0
    var killed = false
    // Write-ahead authority marker. A missing marker in an old snapshot is
    // uncertain, not proof of an orderly shutdown.
    var running = false
    var records: [String: SurfaceActionRecord] = [:]
    init(store: any SurfaceLedgerStore, identity: String) throws {
        self.store = store; self.identity = identity
        let root = try CBORCodec.decodeProfile(store.load())
        if root == .map([]) { try save(); return }
        guard root["identity"] == .text(identity), case .unsigned(let epoch) = root["epoch"],
              case .unsigned(let sequence) = root["sequence"], case .boolean(let killed) = root["killed"],
              case .map(let rows) = root["records"] else { throw SurfaceFailure.protocolViolation("invalid/lost identity ledger") }
        self.epoch = epoch; self.sequence = sequence; self.killed = killed
        running = root["running"] != .boolean(false)
        if running { self.killed = true }
        for row in rows {
            guard case .text(let encoded) = row.value["envelope"], let raw = Data(base64Encoded: encoded),
                  let accept = row.value["accept"],
                  case .array(let outcomes) = row.value["outcome"], outcomes.count <= 1 else {
                throw SurfaceFailure.protocolViolation("corrupt action ledger")
            }
            let bytes = Array(raw)
            let envelope = try SurfaceWire.decode(bytes, from: .runtime)
            guard envelope["type"] == .text("act"), let action = envelope["body"]?["action"],
                  let owner = action["coglet"]?.textValue, let id = action["id"]?.textValue,
                  row.key == owner + "/" + id, accept["id"] == action["id"],
                  accept["correlation_id"] == action["correlation_id"] else { throw SurfaceFailure.protocolViolation("corrupt ledger key") }
            _ = try SurfaceWire.encode(.message("accept", accept), from: .surface)
            if let outcome = outcomes.first {
                try CBORCodec.validate("common::action-outcome", outcome)
                guard accept["accepted"] == .boolean(true), outcome["id"] == action["id"],
                      outcome["correlation_id"] == action["correlation_id"] else { throw SurfaceFailure.protocolViolation("corrupt ledger outcome") }
            }
            records[row.key] = .init(envelope: bytes, accept: accept, outcome: outcomes.first)
        }
    }
    func save() throws {
        let rows = records.sorted { $0.key < $1.key }.map { key, record in
            CBORMapEntry(key, .record([
                // Base64 is internal storage, not a new wire type. Keeping the
                // exact envelope opaque preserves max-depth legal messages.
                ("envelope", .text(Data(record.envelope).base64EncodedString())),
                ("accept", record.accept), ("outcome", .array(record.outcome.map { [$0] } ?? []))]))
        }
        let bytes = try CBORCodec.encodeProfile(.record([
            ("identity", .text(identity)), ("epoch", .unsigned(epoch)), ("sequence", .unsigned(sequence)),
            ("killed", .boolean(killed)), ("running", .boolean(running)), ("records", .map(rows))]))
        guard bytes.count <= 268_435_456 else { throw SurfaceFailure.exhausted }
        try store.save(bytes)
    }
}

extension CBORValue {
    var textValue: String? { if case .text(let value) = self { value } else { nil } }
    var uintValue: UInt64? { if case .unsigned(let value) = self { value } else { nil } }
    var arrayValue: [CBORValue]? { if case .array(let value) = self { value } else { nil } }
}
