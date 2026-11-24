import Foundation
import Security

public enum HubError: Error { case storage, identity, pairing, unavailable }

public enum HubKeyAccessibility: Equatable, Sendable {
    case afterFirstUnlockThisDeviceOnly
    public var secAttr: CFString {
        switch self {
        case .afterFirstUnlockThisDeviceOnly: return kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        }
    }
}

@MainActor
public protocol HubKeychain: AnyObject {
    func read(service: String) throws -> Data?
    func add(_ data: Data, service: String, accessibility: HubKeyAccessibility) throws
    func delete(service: String) throws
}

@MainActor
public final class SecurityHubKeychain: HubKeychain {
    public init() {}
    private func query(_ service: String) -> [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service, kSecAttrAccount as String: "iroh-device",
         kSecAttrSynchronizable as String: false]
    }
    public func read(service: String) throws -> Data? {
        var q = query(service)
        q[kSecReturnData as String] = true
        q[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(q as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess, let data = result as? Data else { throw HubError.storage }
        return data
    }
    public func add(_ data: Data, service: String, accessibility: HubKeyAccessibility) throws {
        var q = query(service)
        q[kSecValueData as String] = data
        q[kSecAttrAccessible as String] = accessibility.secAttr
        // Add-only: a race cannot overwrite another identity.
        guard SecItemAdd(q as CFDictionary, nil) == errSecSuccess else { throw HubError.storage }
    }
    public func delete(service: String) throws {
        let status = SecItemDelete(query(service) as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw HubError.storage }
    }
}

@MainActor
public protocol HubStore {
    func loadKey() throws -> Data?
    func saveKey(_ value: Data) throws
    func deleteKey() throws
    func loadHub() throws -> HubPairing?
    func saveHub(_ value: HubPairing?) throws
}

@MainActor
public final class HubAccount {
    public let store: any HubStore
    private let generate: () throws -> Data
    public init(store: any HubStore, generate: @escaping () throws -> Data = HubAccount.randomKey) {
        self.store = store; self.generate = generate
    }
    public nonisolated static func randomKey() throws -> Data {
        var bytes = [UInt8](repeating: 0, count: 32)
        guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { throw HubError.identity }
        return Data(bytes)
    }
    public func identity() throws -> Data {
        if let key = try store.loadKey() {
            guard key.count == 32 else { throw HubError.identity }
            return key
        }
        // Never silently replace an enrolled device's missing identity.
        guard try store.loadHub() == nil else { throw HubError.identity }
        let key = try generate()
        guard key.count == 32 else { throw HubError.identity }
        try store.saveKey(key)
        return key
    }
    public func signOut() throws {
        // Delete identity first: if local state removal fails, startup fails closed.
        try store.deleteKey()
        try store.saveHub(nil)
    }
}

/// Identity is device-only, not iCloud-synchronized. Configuration contains no invite.
@MainActor
public final class KeychainHubStore: HubStore {
    private let service: String
    private let defaults: UserDefaults
    private let keychain: any HubKeychain
    private let configKey: String
    public init(service: String = "com.softmax.mac.hub", defaults: UserDefaults = .standard, keychain: any HubKeychain = SecurityHubKeychain()) {
        self.service = service; self.defaults = defaults; self.keychain = keychain
        self.configKey = service + ".configuration"
    }
    public func loadKey() throws -> Data? { try keychain.read(service: service) }
    public func saveKey(_ value: Data) throws {
        try keychain.add(value, service: service, accessibility: .afterFirstUnlockThisDeviceOnly)
    }
    public func deleteKey() throws { try keychain.delete(service: service) }
    public func loadHub() throws -> HubPairing? {
        guard let data = defaults.data(forKey: configKey) else { return nil }
        do { return try HubPairing.parse(String(decoding: data, as: UTF8.self)).withoutInvite }
        catch { throw HubError.storage }
    }
    public func saveHub(_ value: HubPairing?) throws {
        if let value { defaults.set(try JSONEncoder().encode(value.withoutInvite), forKey: configKey) }
        else { defaults.removeObject(forKey: configKey) }
    }
}
