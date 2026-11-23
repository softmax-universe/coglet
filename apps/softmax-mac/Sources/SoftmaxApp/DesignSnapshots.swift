import AppKit
import SwiftUI
import SoftmaxCore

/// Test-only rendering entry point: real production views with explicit deterministic
/// fixtures, no TCC, capture, updater, persistence or network side effects.
@MainActor enum DesignSnapshots {
    static func run(to directory: String) throws {
        // Asset compilation and precedence are acceptance assertions, not visual guesses.
        for state in MenuPresentation.allCases {
            let names = ["menu-\(state.rawValue)"] + ([MenuPresentation.listening, .speaking].contains(state) ? (0..<4).map { "menu-\(state.rawValue)-\($0)" } : [])
            for name in names {
                guard let image = NSImage(named: name), image.isTemplate, image.size.width == 18, image.size.height == 18 else {
                    throw CocoaError(.fileReadCorruptFile, userInfo: [NSLocalizedDescriptionKey: "Missing or invalid native template: \(name); loaded=\(String(describing: NSImage(named: name)))"])
                }
            }
        }
        for activity in [AppState.listening, .speaking, .thinking] {
            guard MenuPresentation.resolve(activity: activity, hasError: true, hubConnected: false, updateAvailable: true).rawValue == activity.rawValue else { throw CocoaError(.validationMultipleErrors) }
        }
        guard MenuPresentation.resolve(activity: .idle, hasError: true, hubConnected: false, updateAvailable: true) == .error,
              MenuPresentation.resolve(activity: .idle, hasError: false, hubConnected: false, updateAvailable: true) == .offline,
              MenuPresentation.resolve(activity: .idle, hasError: false, hubConnected: true, updateAvailable: true) == .update,
              MenuPresentation.resolve(activity: .idle, hasError: false, hubConnected: true, updateAvailable: false) == .idle else { throw CocoaError(.validationMultipleErrors) }
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        var manifest: [String] = []
        for scheme in [ColorScheme.light, .dark] {
            for contrast in [ColorSchemeContrast.standard, .increased] {
                let theme = "\(scheme == .dark ? "dark" : "light")\(contrast == .increased ? "-contrast" : "")"
                func capture<V: View>(_ name: String, _ size: NSSize, _ view: V) throws {
                    let key = "\(theme)-\(name)"
                    let hosting = NSHostingView(rootView: view
                        .frame(width: size.width, height: size.height, alignment: .topLeading)
                        .background(scheme == .dark ? SoftmaxDesignTokens.Dark.background : SoftmaxDesignTokens.Light.background)
                        .environment(\.colorScheme, scheme)
                        .environment(\.softmaxHighContrast, contrast == .increased)
                        .environment(\.softmaxFreezeMotion, true))
                    let window = NSWindow(contentRect: NSRect(origin: .zero, size: size), styleMask: [.borderless], backing: .buffered, defer: false)
                    window.appearance = NSAppearance(named: contrast == .increased ? (scheme == .dark ? .accessibilityHighContrastDarkAqua : .accessibilityHighContrastAqua) : (scheme == .dark ? .darkAqua : .aqua))
                    window.isReleasedWhenClosed = false
                    window.contentView = hosting
                    hosting.frame = NSRect(origin: .zero, size: size)
                    window.orderBack(nil)
                    // SwiftUI/AppKit may defer intrinsic TextField sizing and autofocus
                    // beyond one run-loop turn. Require three identical consecutive rasters
                    // after a minimum settling interval, rather than capturing a 40 ms race.
                    // This affects fixtures only; production keyboard focus stays unchanged.
                    let started = Date()
                    var previous: Data?
                    var identicalFrames = 0
                    var stable: Data?
                    while Date().timeIntervalSince(started) < 2 {
                        RunLoop.current.run(until: Date().addingTimeInterval(0.05))
                        // AppKit can initially reserve a legacy scroller gutter before
                        // adopting the runner's overlay preference. Pin fixture scroll
                        // geometry explicitly; leave production/user preferences alone.
                        func normalizeScrollers(_ view: NSView) {
                            if let scroll = view as? NSScrollView, scroll.scrollerStyle != .overlay {
                                print("Normalizing legacy scroller in native snapshot: \(key)")
                                scroll.scrollerStyle = .overlay
                            }
                            for child in view.subviews { normalizeScrollers(child) }
                        }
                        normalizeScrollers(hosting)
                        hosting.layoutSubtreeIfNeeded()
                        if let editor = window.firstResponder as? NSTextView {
                            editor.setSelectedRange(NSRange(location: 0, length: 0))
                        }
                        window.display()
                        guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
                            throw CocoaError(.fileWriteUnknown)
                        }
                        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
                        guard let bytes = bitmap.representation(using: NSBitmapImageRep.FileType.png, properties: [:]) else { throw CocoaError(.fileWriteUnknown) }
                        identicalFrames = bytes == previous ? identicalFrames + 1 : 1
                        previous = bytes
                        if identicalFrames >= 3 && Date().timeIntervalSince(started) >= 0.2 {
                            stable = bytes
                            break
                        }
                    }
                    guard let bytes = stable else {
                        throw CocoaError(.fileWriteUnknown, userInfo: [NSLocalizedDescriptionKey: "Native snapshot did not settle: \(key)"])
                    }
                    try bytes.write(to: root.appendingPathComponent(key + ".png"))
                    manifest.append(key)
                    window.close()
                }
                let model = ShellModel()
                let commands = SoftmaxCommand.catalog(settings: { _ in }, face: {}, chat: { _ in }, about: {})
                for query in ["", "microphone", "no-such-command"] {
                    let name = query.isEmpty ? "commands" : query == "microphone" ? "commands-filtered" : "commands-empty"
                    try capture(name, NSSize(width: 460, height: 480), CommandPaletteView(commands: commands, dismiss: {}, query: query))
                    try capture(name + "-large", NSSize(width: 460, height: 480), CommandPaletteView(commands: commands, dismiss: {}, query: query).environment(\.softmaxTextScale, 1.6))
                }
                for step in 0..<3 {
                    try capture("onboarding-\(step)", NSSize(width: 620, height: 600), OnboardingView(model: model, openSettings: {}, finish: {}, step: step))
                    try capture("onboarding-\(step)-large", NSSize(width: 620, height: 600), OnboardingView(model: model, openSettings: {}, finish: {}, step: step).environment(\.softmaxTextScale, 1.6))
                }
                for permission in PermissionStatus.allCases {
                    model.permissions = PermissionSnapshot(microphone: permission, inputMonitoring: permission)
                    try capture("onboarding-voice-\(permission.rawValue)", NSSize(width: 620, height: 600), OnboardingView(model: model, openSettings: {}, finish: {}, step: 1))
                }
                for status in [LoginItemStatus.disabled, .enabled, .requiresApproval, .unavailable, .unknown] {
                    model.loginItem = status
                    try capture("onboarding-login-\(status.rawValue)", NSSize(width: 620, height: 600), OnboardingView(model: model, openSettings: {}, finish: {}, step: 2))
                }
                for tab in SettingsTab.allCases {
                    model.selectedTab = tab
                    try capture("settings-\(tab.rawValue.lowercased())", NSSize(width: 760, height: 540), SettingsView(model: model))
                    try capture("settings-\(tab.rawValue.lowercased())-large", NSSize(width: 760, height: 540), SettingsView(model: model).environment(\.softmaxTextScale, 1.6))
                }
                for state in HUDPresentation.allCases {
                    try capture("hud-\(state.rawValue)", NSSize(width: 320, height: 88), TalkHUDView(state: state, level: 0.4, elapsed: 8, hotkey: "Right Option"))
                    try capture("hud-\(state.rawValue)-large", NSSize(width: 320, height: 141), TalkHUDView(state: state, level: 0.4, elapsed: 8, hotkey: "Right Option").environment(\.softmaxTextScale, 1.6))
                }
                for state in MenuPresentation.allCases {
                    try capture("menu-\(state.rawValue)", NSSize(width: 322, height: 440), SoftmaxMenuView(model: model, showFace: {}, showChat: { _ in }, settings: {}, about: {}, quit: {}, presentation: state))
                    try capture("menu-\(state.rawValue)-large", NSSize(width: 322, height: 704), SoftmaxMenuView(model: model, showFace: {}, showChat: { _ in }, settings: {}, about: {}, quit: {}, presentation: state).environment(\.softmaxTextScale, 1.6))
                    try capture("template-\(state.rawValue)", NSSize(width: 36, height: 36), MenuTemplate(state: state, animate: false).padding(9).modifier(SoftmaxTheme()))
                }
                let host = FaceHost(shell: model)
                try capture("face-idle", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}))
                try capture("face-idle-large", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}).environment(\.softmaxTextScale, 1.6))
                for (name, state) in [("listening", FaceState.listening(level: 0.4)), ("thinking", .thinking), ("speaking", .speaking(level: 0.4))] {
                    host.driver.setState(state)
                    try capture("face-\(name)", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}))
                    try capture("face-\(name)-large", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}).environment(\.softmaxTextScale, 1.6))
                }
                for expression in ["happy", "concerned"] {
                    host.driver.setState(.idle)
                    try host.driver.setExpression(expression, intensity: 1000, durationMS: 60000)
                    try capture("face-\(expression)", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}))
                    try capture("face-\(expression)-large", NSSize(width: 280, height: 330), FacePanelView(host: host, hide: {}, resize: {}).environment(\.softmaxTextScale, 1.6))
                }
                try capture("about", NSSize(width: 420, height: 420), AboutView())
                try capture("about-licenses", NSSize(width: 420, height: 480), AboutView(licenses: true))
                try capture("about-large", NSSize(width: 420, height: 420), AboutView().environment(\.softmaxTextScale, 1.6))
                try capture("about-licenses-large", NSSize(width: 420, height: 480), AboutView(licenses: true).environment(\.softmaxTextScale, 1.6))
            }
        }
        try JSONSerialization.data(withJSONObject: manifest.sorted(), options: [.prettyPrinted]).write(to: root.appendingPathComponent("manifest.json"))
    }
}
