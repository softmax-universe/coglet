import CogletTypes

/// Trusted local validation ceiling, never inferred from a peer or catalog.
public enum SurfaceContractProfile: String, Sendable {
    case surface01 = "surface-0.1"
    case surface02 = "surface-0.2"

    public func validateName(_ name: String) throws {
        try CBORCodec.validate(rule("common::capability-name"), .text(name))
    }

    func rule(_ frozen: String) -> String {
        guard self == .surface02 else { return frozen }
        switch frozen {
        case "common::capability-name": return "surface02::s02-capability-name"
        case "common::event": return "surface02::s02-event"
        case "common::action": return "surface02::s02-action"
        case "surface::surface-message": return "surface02::s02-surface-message"
        case "surface::capabilities": return "surface02::s02-capabilities"
        case "surface::cap-entry": return "surface02::s02-cap-entry"
        case "surface::schema-refs": return "surface02::s02-schema-refs"
        case "surface::grant": return "surface02::s02-grant"
        case "surface::grants": return "surface02::s02-grants"
        case "surface::read": return "surface02::s02-read"
        case "surface::act": return "surface02::s02-act"
        default: return frozen
        }
    }
}
