import Foundation
import CogletSurface

/// Trusted installed configuration only. Never feed peer advertisements or a
/// guest request here. The embedded runtime receives these SAME original bytes.
/// Selecting a catalog/profile does not add manifest operations or resource leases.
public struct AppSurfaceProvisioning {
    public static let catalogPin = "3b2ad502dd3d111b9f6fcf832a51fccff2b8582348de8f9e354e75f2019354f9"
    public let profile: SurfaceContractProfile
    public let catalog: TrustedExtensionCatalog?
    public let operations: [SurfaceOperation]
    public let operationNames: Set<String>
    public let resources: Set<String>
    public var enablesApp: Bool { operations.contains { $0.name.hasPrefix("softmax.app.") } }

    public init(configuration: Data) throws {
        guard let root = try JSONSerialization.jsonObject(with: configuration) as? [String: Any],
              let setup = root["setup"] as? [String: Any], let manifest = setup["manifest"] as? [String: Any],
              let subscriptions = manifest["subscriptions"] as? [String],
              let required = manifest["required_capabilities"] as? [String],
              let exclusive = manifest["exclusive_outputs"] as? [String] else { throw AppSurfaceFailure.invalidArguments }
        if let selector = setup["contract_profile"] {
            guard let raw = selector as? String, let parsed = SurfaceContractProfile(rawValue: raw) else { throw AppSurfaceFailure.invalidArguments }
            profile = parsed
        } else { profile = .surface01 }
        if let provision = root["trusted_extension_catalog"] {
            guard let record = provision as? [String: Any], Set(record.keys) == ["json", "sha256"],
                  let json = record["json"] as? String, let pin = record["sha256"] as? String,
                  pin == Self.catalogPin else { throw AppSurfaceFailure.permissionDenied }
            catalog = try TrustedExtensionCatalog(json: Array(json.utf8), sha256: pin, contractProfile: profile)
        } else { catalog = nil }
        // Preserve the existing production speech subset, not every SDK builtin.
        let builtins = SurfaceOperation.builtins.filter {
            ["speech.start", "speech.end", "speech.speak_chunk", "expression.set_expression"].contains($0.name)
        }
        let available = builtins + (catalog?.operations ?? [])
        let requested = Set(subscriptions + required)
        guard requested.isSubset(of: Set(available.map(\.name))) else { throw AppSurfaceFailure.unsupported }
        let selected = requested.union(["speech.start", "speech.end"])
        operationNames = selected
        operations = available.filter { selected.contains($0.name) }
        // Existing audio permissions remain unchanged. App resources are NEVER
        // inferred from the catalog or operation request (especially sensitive).
        resources = Set(exclusive).union(["audio.mic", "audio.speaker"])
        guard !enablesApp || profile == .surface02 else { throw AppSurfaceFailure.permissionDenied }
    }
}
