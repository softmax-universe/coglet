import SwiftUI
import AppKit
import SoftmaxCore
import SoftmaxAppSurface

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
    var searchCommands: () -> Void = {}
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
            HStack { MenuTemplate(state: presentation ?? model.menuPresentation); Text((presentation ?? model.menuPresentation).label).fontWeight(.semibold) }
            Text("Push-to-talk: \(model.hotkeyName)")
            Text(model.hubStatus)
            if let name = model.hubConnection?.pairing?.name { Text(name) }
            Text(model.testing ? "Local runtime: not connected" : "Local runtime: \(model.runtime.state.rawValue)")
            if model.updates.updateAvailable || presentation == .update { Text("Update available · install from Updates") }
            Divider()
            CogletButton("Open Face", id: "menu.open_face", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); showFace() }
            CogletButton("Open Chat", id: "menu.open_chat", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); showChat(false) }
            CogletButton("Open Voice", id: "menu.open_voice", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); showChat(true) }
            Divider()
            CogletButton("Commands…  ⌘K", id: "menu.commands", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); searchCommands() }
            CogletButton("Settings…", id: "menu.settings", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); settings() }.keyboardShortcut(",")
            CogletButton("Check for Updates…", id: "menu.check_updates", registry: model.appControls, window: "menu_bar", route: "menu", sensitive: true, adapterSynchronous: false, failure: model.appControlFailure) { model.updates.checkForUpdates() }.disabled(!model.updates.enabled)
            CogletButton("About Softmax", id: "menu.about", registry: model.appControls, window: "menu_bar", route: "menu", failure: model.appControlFailure) { dismiss(); about() }
            Divider()
            CogletButton("Quit Softmax", id: "menu.quit", registry: model.appControls, window: "menu_bar", route: "menu", sensitive: true, adapterSynchronous: false, failure: model.appControlFailure, action: quit).keyboardShortcut("q")
        }.buttonStyle(.borderless).padding(SoftmaxDesignTokens.spacing[3])
            .frame(width: 322, alignment: .leading).modifier(SoftmaxTheme())
    }
}

extension ShellModel {
    var menuPresentation: MenuPresentation {
        MenuPresentation.resolve(activity: state, hasError: message != nil || runtime.state == .error, hubConnected: hubConnection?.state == .connected, updateAvailable: updates.updateAvailable)
    }
}
