import Foundation
import Security

/// Compile with the unmodified production HubAccount.swift and HubPairing.swift.
/// Run as a disposable signed/provisioned app, never against the real service.
@main
struct KeychainProbe {
    @MainActor
    static func main() throws {
        let service = "com.softmax.app.keychain-proof." + UUID().uuidString
        let keychain = SecurityHubKeychain()
        let value = try HubAccount.randomKey()
        guard try keychain.read(service: service) == nil else { throw HubError.storage }
        do {
            try keychain.add(value, service: service, accessibility: .afterFirstUnlockThisDeviceOnly)
            guard try keychain.read(service: service) == value else { throw HubError.storage }
            // Read actual OS attributes, not merely the application's request.
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: service,
                kSecAttrAccount as String: "iroh-device",
                kSecUseDataProtectionKeychain as String: true,
                kSecAttrSynchronizable as String: false,
                kSecReturnAttributes as String: true
            ]
            var result: CFTypeRef?
            guard SecItemCopyMatching(query as CFDictionary, &result) == errSecSuccess,
                  let attributes = result as? [String: Any],
                  attributes[kSecAttrAccessible as String] as? String == kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String,
                  attributes[kSecAttrSynchronizable as String] as? Bool == false else { throw HubError.storage }
            var refusedDuplicate = false
            do { try keychain.add(Data(repeating: 0, count: 32), service: service, accessibility: .afterFirstUnlockThisDeviceOnly) }
            catch { refusedDuplicate = true }
            guard refusedDuplicate, try keychain.read(service: service) == value else { throw HubError.storage }
            try keychain.delete(service: service)
            guard try keychain.read(service: service) == nil else { throw HubError.storage }
        } catch {
            try keychain.delete(service: service)
            throw error
        }
        print("Data Protection Keychain: add/read, device-only accessibility, no sync, add-only, delete/read-not-found PASS")
    }
}
