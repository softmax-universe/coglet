import CogletTypes

/// Transport validation follows the peer's actual immutable selection, never
/// the local provisioned ceiling. The session separately binds correlation and
/// lifecycle; transports don't infer grants or operation schemas from syntax.
struct SurfaceTransportVersion {
    let ceiling: SurfaceContractProfile
    private var wire: SurfaceContractProfile = .surface01
    private var selected = false
    init(_ ceiling: SurfaceContractProfile) { self.ceiling = ceiling }
    func validateOutgoing(_ bytes: [UInt8]) throws {
        _ = try SurfaceWire.decode(bytes, from: .surface, contractProfile: wire)
    }
    mutating func validateIncoming(_ bytes: [UInt8]) throws {
        let message = try SurfaceWire.decode(bytes, from: .runtime, contractProfile: wire)
        if message["type"] == .text("hello_reply") {
            guard !selected, let version = message["body"]?["selection"]?["version"]?.textValue,
                  version == "0.1" || (version == "0.2" && ceiling == .surface02) else {
                throw SurfaceFailure.protocolViolation("invalid transport version selection")
            }
            wire = version == "0.2" ? .surface02 : .surface01
            selected = true
        }
    }
}
