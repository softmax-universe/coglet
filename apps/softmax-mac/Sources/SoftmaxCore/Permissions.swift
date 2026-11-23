public enum PermissionStatus: String, CaseIterable, Sendable {
    case notDetermined, restricted, denied, granted, notGranted, unknown

    public static func microphone(rawValue: Int) -> Self {
        switch rawValue {
        case 0: .notDetermined
        case 1: .restricted
        case 2: .denied
        case 3: .granted
        default: .unknown
        }
    }

    // CoreGraphics does not distinguish never-requested from denied.
    public static func inputMonitoring(granted: Bool) -> Self {
        granted ? .granted : .notGranted
    }
}

public struct PermissionSnapshot: Equatable, Sendable {
    public var microphone: PermissionStatus
    public var inputMonitoring: PermissionStatus
    public init(microphone: PermissionStatus, inputMonitoring: PermissionStatus) {
        self.microphone = microphone
        self.inputMonitoring = inputMonitoring
    }
    public var canUseGlobalPushToTalk: Bool {
        microphone == .granted && inputMonitoring == .granted
    }
}

public enum LoginItemStatus: String, Sendable {
    case disabled, enabled, requiresApproval, unavailable, unknown
    public init(serviceRawValue: Int) {
        self = switch serviceRawValue {
        case 0: .disabled
        case 1: .enabled
        case 2: .requiresApproval
        case 3: .unavailable
        default: .unknown
        }
    }
}
