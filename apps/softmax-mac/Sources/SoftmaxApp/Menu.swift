import SwiftUI
import AppKit
import SoftmaxCore

/// Activity wins over orthogonal availability. No connection is inferred from a UI setting.
enum MenuPresentation: String, CaseIterable {
    case idle, listening, thinking, speaking, offline, update, error
    static func resolve(activity: AppState, hasError: Bool, hubConnected: Bool, updateAvailable: Bool) -> Self {
        switch activity {
        case .listening: return .listening
        case .speaking: return .speaking
        case .thinking: return .thinking
        case .error: return .error
        default: break
        }
        if hasError { return .error }
        if !hubConnected { return .offline }
        return updateAvailable ? .update : .idle
    }
    var label: String {
        switch self {
        case .idle: "Ready"
        case .listening: "Listening"
        case .thinking: "Thinking"
        case .speaking: "Speaking"
        case .offline: "Hub disconnected"
        case .update: "Update available"
        case .error: "Needs attention"
        }
    }
}

struct MenuTemplate: View {
    var state: MenuPresentation
    var animate = true
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.softmaxFreezeMotion) private var freezeMotion
    var body: some View {
        TimelineView(.animation(minimumInterval: Double(SoftmaxDesignTokens.motionMenuFrameMs) / 1000,
                                paused: reduceMotion || freezeMotion || !animate || (state != .listening && state != .speaking))) { context in
            let frame = Int(context.date.timeIntervalSinceReferenceDate / (Double(SoftmaxDesignTokens.motionMenuFrameMs) / 1000)) % 4
            let suffix = !reduceMotion && !freezeMotion && animate && (state == .listening || state == .speaking) ? "-\(frame)" : ""
            Image("menu-\(state.rawValue)\(suffix)").renderingMode(.template)
                .resizable().frame(width: 18, height: 18)
        }.accessibilityHidden(true)
    }
}

struct SoftmaxMenuView: View {
    @ObservedObject var model: ShellModel
    @Environment(\.dismiss) private var dismiss
    var showFace: () -> Void
    var showChat: (Bool) -> Void
    var settings: () -> Void
    var about: () -> Void
    var quit: () -> Void
    var presentation: MenuPresentation? = nil
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
            HStack { MenuTemplate(state: presentation ?? model.menuPresentation); Text((presentation ?? model.menuPresentation).label).fontWeight(.semibold) }
            Text("Push-to-talk: \(model.hotkeyName)")
            Text("Hub disconnected · no hub configured")
            Text("Local runtime: not connected")
            if model.updates.updateAvailable { Text("Update available · install from Updates") }
            Divider()
            Button("Open Face") { dismiss(); showFace() }
            Button("Open Chat") { dismiss(); showChat(false) }
            Button("Open Voice") { dismiss(); showChat(true) }
            Divider()
            Button("Settings…") { dismiss(); settings() }.keyboardShortcut(",")
            Button("Check for Updates…") { model.updates.checkForUpdates() }.disabled(!model.updates.enabled)
            Button("About Softmax") { dismiss(); about() }
            Divider()
            Button("Quit Softmax", action: quit).keyboardShortcut("q")
        }.buttonStyle(.borderless).padding(SoftmaxDesignTokens.spacing[3])
            .frame(width: 322, alignment: .leading).modifier(SoftmaxTheme())
    }
}

extension ShellModel {
    var menuPresentation: MenuPresentation {
        MenuPresentation.resolve(activity: state, hasError: message != nil, hubConnected: false, updateAvailable: updates.updateAvailable)
    }
}
