public enum AppState: String, CaseIterable, Hashable, Sendable {
    case onboarding, idle, listening, thinking, speaking, offline, error

    public var symbolName: String {
        switch self {
        case .onboarding: "sparkles"
        case .idle: "circle"
        case .listening: "mic.fill"
        case .thinking: "ellipsis.circle"
        case .speaking: "speaker.wave.2.fill"
        case .offline: "network.slash"
        case .error: "exclamationmark.triangle.fill"
        }
    }
}

public enum AppEvent: CaseIterable, Hashable, Sendable {
    case finishOnboarding, hubConnected, hubDisconnected
    case beginListening, finishListening, beginSpeaking, finishSpeaking, cancel
    case fail, recover
}

/// Pure reducer: callers own side effects (audio, hub, persistence).
/// Rejected events leave state unchanged. Reconnect never resumes interrupted audio.
public struct AppStateMachine: Sendable {
    public private(set) var state: AppState

    public init(onboardingComplete: Bool) {
        state = onboardingComplete ? .offline : .onboarding
    }

    @discardableResult
    public mutating func send(_ event: AppEvent) -> Bool {
        let next: AppState
        switch (state, event) {
        case (.onboarding, .finishOnboarding): next = .offline
        case (.offline, .hubConnected): next = .idle
        case (.idle, .beginListening), (.speaking, .beginListening): next = .listening
        case (.listening, .finishListening): next = .thinking
        case (.thinking, .beginSpeaking): next = .speaking
        case (.speaking, .finishSpeaking): next = .idle
        case (.listening, .cancel), (.thinking, .cancel), (.speaking, .cancel): next = .idle
        case (.idle, .hubDisconnected), (.listening, .hubDisconnected),
             (.thinking, .hubDisconnected), (.speaking, .hubDisconnected): next = .offline
        case (.idle, .fail), (.listening, .fail), (.thinking, .fail),
             (.speaking, .fail), (.offline, .fail): next = .error
        case (.error, .recover): next = .offline
        default: return false
        }
        state = next
        return true
    }
}
