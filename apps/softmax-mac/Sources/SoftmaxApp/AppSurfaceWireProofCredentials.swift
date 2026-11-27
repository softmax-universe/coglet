#if DEBUG
import Foundation
import Darwin
import SoftmaxCore

private struct CredentialFixtureFailure: Swift.Error {}

/// Memory-only HubStore for the isolated wire proof. It is never a Keychain,
/// UserDefaults or file store: the synthetic identity key exists only here.
@MainActor
private final class CredentialFixtureStore: HubStore {
    private var key: Data?
    private var hub: HubPairing?
    init(key: Data) { self.key = key }
    func loadKey() throws -> Data? { key }
    func saveKey(_ value: Data) throws { key = value }
    func deleteKey() throws { key = nil }
    func loadHub() throws -> HubPairing? { hub }
    func saveHub(_ value: HubPairing?) throws { hub = value?.withoutInvite }
}

/// In-process HubLink: no helper process, socket or network. It records ONLY
/// Boolean/counter witnesses that the real HubConnection handed it the exact
/// synthetic key and invitation; the secret bytes are never copied elsewhere.
@MainActor
private final class CredentialFixtureLink: HubLink {
    private let expectedKey: Data
    private let expectedInvite: String
    private var handler: ((HubLinkEvent) -> Void)?
    private(set) var starts = 0, matchingStarts = 0
    private(set) var connects = 0, matchingConnects = 0
    private(set) var stops = 0
    init(key: Data, invite: String) { expectedKey = key; expectedInvite = invite }
    func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws {
        starts += 1
        if key == expectedKey { matchingStarts += 1 }
        handler = event
    }
    func connect(_ pairing: HubPairing) throws {
        connects += 1
        if pairing.invite == expectedInvite { matchingConnects += 1 }
    }
    func stop() { stops += 1; handler = nil }
    func deliverReady(device: String) { handler?(.ready(device: device)) }
}

/// DEBUG-only native credential fixture for the isolated app wire proof.
/// Activated only by `--ui-testing`, `--sdk-wire-proof-directory` AND the
/// distinct `--sdk-wire-proof-credentials` flag. The synthetic key/invite are
/// read from a bounded private file in the already-validated proof directory,
/// then driven through the PRODUCTION HubAccount and HubConnection with fake
/// in-memory store/link only (never KeychainHubStore, ProcessHubLink, network).
@MainActor
final class WireProofCredentialFixture {
    static let flag = "--sdk-wire-proof-credentials"
    static let fileName = "credentials.json"
    static let witnessName = "credential-witness"
    // Public, non-secret identifiers for the fake router/device.
    static let publicDevice = String(repeating: "d", count: 64)
    static let publicHub = String(repeating: "c", count: 64)
    private static let sizeLimit = 1024

    private let directory: URL
    private var attempted = false
    private var connection: HubConnection?
    private var link: CredentialFixtureLink?
    private var stopped = false

    init(directory: URL) { self.directory = directory }

    /// Call ONLY after the executor validated `directory`. One attempt; a
    /// failure is terminal (never retried and never partially installed).
    func install(into model: ShellModel) throws {
        guard !attempted else { return }
        attempted = true
        let (key, invite) = try Self.readCredentials(directory.appendingPathComponent(Self.fileName))
        let link = CredentialFixtureLink(key: key, invite: invite)
        let account = HubAccount(store: CredentialFixtureStore(key: key),
                                 generate: { throw HubError.identity })
        let connection = HubConnection(account: account, link: link)
        // Real startup path first; the fake link then reports a PUBLIC device.
        connection.start()
        guard link.starts == 1, link.matchingStarts == 1 else { connection.stop(); throw CredentialFixtureFailure() }
        link.deliverReady(device: Self.publicDevice)
        guard connection.deviceID == Self.publicDevice, connection.state == .offline else {
            connection.stop(); throw CredentialFixtureFailure()
        }
        // Real enrollment: leaves .connecting with an actual pending invite and
        // no retry (the fake link never reports another event).
        let object: [String: Any] = ["name": "Synthetic router", "hub": Self.publicHub,
                                     "addresses": [String](), "invite": invite]
        do {
            let text = String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
            try connection.enroll(try HubPairing.parse(text))
        } catch { connection.stop(); throw CredentialFixtureFailure() }
        guard connection.state == .connecting, connection.pairing == nil, connection.errorMessage == nil,
              link.connects == 1, link.matchingConnects == 1 else {
            connection.stop(); throw CredentialFixtureFailure()
        }
        self.link = link; self.connection = connection
        do { try model.installWireProofHubFixture(connection) }
        catch { connection.stop(); self.connection = nil; self.link = nil; throw error }
        try writeWitness()
    }

    /// Idempotent. Stops the real connection (-> link.stop) and records it.
    func stop() {
        guard let connection, !stopped else { return }
        stopped = true
        connection.stop()
        try? writeWitness()
    }

    // Closed evidence: Booleans, counters and enum raw values only.
    private func writeWitness() throws {
        guard let connection, let link else { throw CredentialFixtureFailure() }
        let value: [String: Any] = [
            "seeded": true,
            "key_started": link.starts == 1 && link.matchingStarts == 1,
            "token_connected": link.connects == 1 && link.matchingConnects == 1,
            "installs": 1,
            "state": connection.state.rawValue,
            "stopped": stopped,
            "link_stops": link.stops,
        ]
        try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
            .write(to: directory.appendingPathComponent(Self.witnessName), options: .atomic)
    }

    private static func readCredentials(_ url: URL) throws -> (Data, String) {
        // lstat rejects a symlink; O_NOFOLLOW + fstat identity rejects a swap.
        var info = stat()
        guard lstat(url.path, &info) == 0, info.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG),
              info.st_mode & 0o777 == 0o600, info.st_uid == getuid(),
              info.st_size > 0, info.st_size <= off_t(sizeLimit) else { throw CredentialFixtureFailure() }
        let descriptor = Darwin.open(url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard descriptor >= 0 else { throw CredentialFixtureFailure() }
        defer { Darwin.close(descriptor) }
        var opened = stat()
        guard fstat(descriptor, &opened) == 0, opened.st_dev == info.st_dev, opened.st_ino == info.st_ino,
              opened.st_mode & mode_t(S_IFMT) == mode_t(S_IFREG), opened.st_size == info.st_size else {
            throw CredentialFixtureFailure()
        }
        var buffer = [UInt8](repeating: 0, count: sizeLimit + 1)
        let capacity = buffer.count
        var total = 0
        while total < capacity {
            let count = buffer.withUnsafeMutableBytes { Darwin.read(descriptor, $0.baseAddress! + total, capacity - total) }
            guard count >= 0 else { throw CredentialFixtureFailure() }
            if count == 0 { break }
            total += count
        }
        guard total == Int(info.st_size), total <= sizeLimit else { throw CredentialFixtureFailure() }
        guard let parsed = try? JSONSerialization.jsonObject(with: Data(buffer[0..<total])),
              let fields = parsed as? [String: Any], Set(fields.keys) == ["identity", "invite"],
              let identity = fields["identity"] as? String, let invite = fields["invite"] as? String,
              let key = bytes(hex: identity), HubPairing.isHex(invite) else { throw CredentialFixtureFailure() }
        return (key, invite)
    }

    /// Exactly 64 lowercase hex characters -> a 32-byte identity key.
    private static func bytes(hex: String) -> Data? {
        guard HubPairing.isHex(hex) else { return nil }
        var result = [UInt8]()
        result.reserveCapacity(32)
        var iterator = hex.utf8.makeIterator()
        while let high = iterator.next(), let low = iterator.next() {
            let upper: UInt8 = high <= 57 ? high - 48 : high - 87
            let lower: UInt8 = low <= 57 ? low - 48 : low - 87
            result.append(upper << 4 | lower)
        }
        return result.count == 32 ? Data(result) : nil
    }
}
#endif
