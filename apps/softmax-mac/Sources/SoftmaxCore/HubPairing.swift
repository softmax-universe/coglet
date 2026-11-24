import Foundation
import Darwin

/// Local provisioning envelope, not a new hub wire method. Invite is memory-only.
public struct HubPairing: Codable, Equatable, Sendable {
    public var name: String
    public var hub: String
    public var addresses: [String]
    public var relay: String?
    public var invite: String?
    public var device: String?
    public var withoutInvite: HubPairing { var copy = self; copy.invite = nil; return copy }
    public static func isHex(_ value: String) -> Bool {
        value.utf8.count == 64 && value.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
    }
    public static func parse(_ text: String) throws -> HubPairing {
        guard text.utf8.count <= 16384, let data = text.data(using: .utf8),
              let p = try? JSONDecoder().decode(HubPairing.self, from: data),
              !p.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              p.name.count <= 128, !p.name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }),
              isHex(p.hub), p.invite.map(isHex) ?? true, p.device.map(isHex) ?? true,
              p.addresses.count <= 16, p.addresses.allSatisfy(validAddress) else { throw HubError.pairing }
        if let relay = p.relay {
            guard relay.utf8.count <= 2048, let u = URLComponents(string: relay),
                  u.scheme == "https", u.host != nil, u.user == nil, u.password == nil,
                  u.fragment == nil else { throw HubError.pairing }
        }
        return p
    }
    private static func validAddress(_ value: String) -> Bool {
        guard value.utf8.count <= 128, let colon = value.lastIndex(of: ":"),
              let port = UInt16(value[value.index(after: colon)...]), port > 0 else { return false }
        let host = String(value[..<colon])
        var v4 = in_addr(), v6 = in6_addr()
        if host.hasPrefix("["), host.hasSuffix("]") {
            return String(host.dropFirst().dropLast()).withCString { inet_pton(AF_INET6, $0, &v6) } == 1
        }
        return host.withCString { inet_pton(AF_INET, $0, &v4) } == 1
    }
}
