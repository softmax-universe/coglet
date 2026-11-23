import AppKit
import SwiftUI
import SoftmaxCore

/// Test-only rendering entry point: real production views with explicit deterministic
/// fixtures, no TCC, capture, updater, persistence or network side effects.
@MainActor enum DesignSnapshots {
    static func run(to directory: String) throws {
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var manifest: [String] = []
        for scheme in [ColorScheme.light, .dark] {
            for contrast in [ColorSchemeContrast.standard, .increased] {
                let theme = "\(scheme == .dark ? "dark" : "light")\(contrast == .increased ? "-contrast" : "")"
                func capture<V: View>(_ name: String, _ size: NSSize, _ view: V) throws {
                    let key = "\(theme)-\(name)"
                    let hosting = NSHostingView(rootView: view
                        .environment(\.colorScheme, scheme)
                        .environment(\.colorSchemeContrast, contrast)
                        .environment(\.accessibilityReduceMotion, true))
                    let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless], backing: .buffered, defer: false)
                    window.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
                    window.isReleasedWhenClosed = false
                    window.contentView = hosting
                    hosting.frame = NSRect(origin: .zero, size: size)
                    hosting.layoutSubtreeIfNeeded()
                    window.display()
                    guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
                        throw CocoaError(.fileWriteUnknown)
                    }
                    hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
                    guard let bytes = bitmap.representation(using: .png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
                    try bytes.write(to: root.appendingPathComponent(key + ".png"))
                    manifest.append(key)
                    window.close()
                }
                let model = ShellModel()
                for step in 0..<3 {
                    try capture("onboarding-\(step)", NSSize(width: 620, height: 500), OnboardingView(model: model, openSettings: {}, finish: {}, step: step))
                }
                for permission in PermissionStatus.allCases {
                    model.permissions = PermissionSnapshot(microphone: permission, inputMonitoring: permission)
                    try capture("onboarding-voice-\(permission.rawValue)", NSSize(width: 620, height: 500), OnboardingView(model: model, openSettings: {}, finish: {}, step: 1))
                }
                for tab in SettingsTab.allCases {
                    model.selectedTab = tab
                    try capture("settings-\(tab.rawValue.lowercased())", NSSize(width: 760, height: 540), SettingsView(model: model))
                    try capture("settings-\(tab.rawValue.lowercased())-large", NSSize(width: 920, height: 720), SettingsView(model: model).environment(\.dynamicTypeSize, .accessibility5))
                }
                for state in HUDPresentation.allCases {
                    try capture("hud-\(state.rawValue)", NSSize(width: 320, height: 88), TalkHUDView(state: state, level: 0.4, elapsed: 8, hotkey: "Right Option"))
                }
                for state in MenuPresentation.allCases {
                    try capture("menu-\(state.rawValue)", NSSize(width: 322, height: 440), SoftmaxMenuView(model: model, showFace: {}, showChat: { _ in }, settings: {}, about: {}, quit: {}, presentation: state))
                    try capture("template-\(state.rawValue)", NSSize(width: 36, height: 36), MenuTemplate(state: state, animate: false).padding(9))
                }
                let host = FaceHost(shell: model)
                try capture("face-idle", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}))
                try capture("about", NSSize(width: 420, height: 420), AboutView())
            }
        }
        try JSONSerialization.data(withJSONObject: manifest.sorted(), options: [.prettyPrinted]).write(to: root.appendingPathComponent("manifest.json"))
    }
}
