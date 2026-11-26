import SwiftUI
import ServiceManagement
import SoftmaxAppSurface

enum SettingsTab: String, CaseIterable, Identifiable {
    case general = "General", hotkey = "Hotkey", audio = "Audio", hub = "Hub"
    case coglets = "Coglets", updates = "Updates", privacy = "Privacy"
    var id: Self { self }
    var keywords: String {
        switch self {
        case .general: "login startup appearance theme dock orb face"
        case .hotkey: "shortcut keyboard hold toggle push talk"
        case .audio: "microphone input output sound volume"
        case .hub: "router account connect enrollment offline"
        case .coglets: "runtime grants capabilities"
        case .updates: "version download install release"
        case .privacy: "permissions security diagnostics"
        }
    }
}

struct SettingsView: View {
    @ObservedObject var model: ShellModel
    @AppStorage("appearance") private var appearance = "System"
    @AppStorage("textScale") private var textScale = 1.0
    @State private var query = ""
    @FocusState private var searchFocused: Bool
    @Environment(\.colorScheme) private var scheme
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.softmaxHighContrast) private var forceContrast
    private var accent: Color { scheme == .dark ? SoftmaxDesignTokens.Dark.accent : SoftmaxDesignTokens.Light.accent }
    private var tabs: [SettingsTab] {
        SettingsTab.allCases.filter { query.isEmpty || ($0.rawValue + " " + $0.keywords).localizedCaseInsensitiveContains(query) }
    }
    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
                TextField("Filter settings  /", text: $query).focused($searchFocused)
                    .modifier(SoftmaxSearchFieldFrame())
                    .coglet(id: "settings.search", registry: model.appControls, window: "settings",
                        route: model.selectedTab.rawValue.lowercased(), role: "text_field", label: "Search settings",
                        verbs: ["type_text"], value: {
                            .map([.init("kind", .text("text")), .init("value", .text(query))])
                        }, focus: { searchFocused = true; return true }, focused: { searchFocused },
                        failure: model.appControlFailure, invoke: { args in
                            guard case .map(let fields) = args,
                                  let value = fields.first(where: { $0.key == "value" }), case .text(let text) = value.value else {
                                throw ControlRegistryFailure.unsupported
                            }
                            query = text
                        })
                    .accessibilityLabel("Search settings").onSubmit {
                        if !tabs.contains(model.selectedTab), let tab = tabs.first { model.selectedTab = tab }
                    }
                    .onKeyPress(.downArrow) {
                        guard !tabs.isEmpty else { return .handled }
                        let index = tabs.firstIndex(of: model.selectedTab) ?? 0
                        model.selectedTab = tabs[min(index + 1, tabs.count - 1)]
                        return .handled
                    }
                    .onKeyPress(.upArrow) {
                        guard !tabs.isEmpty else { return .handled }
                        let index = tabs.firstIndex(of: model.selectedTab) ?? 0
                        model.selectedTab = tabs[max(index - 1, 0)]
                        return .handled
                    }
                Text("\(tabs.count) sections").softmaxFont(SoftmaxDesignTokens.fontCaption)
                ForEach(tabs) { tab in
                    let select = { model.selectedTab = tab }
                    Button(action: select) {
                        highlightedMatch(tab.rawValue, query: query).frame(maxWidth: .infinity, alignment: .leading).padding(6)
                            .background(model.selectedTab == tab ? accent.opacity(0.14) : .clear, in: RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusControl))
                            .overlay(RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusControl)
                                .strokeBorder(model.selectedTab == tab && (contrast == .increased || forceContrast) ? accent : .clear, lineWidth: 2))
                    }.buttonStyle(.borderless)
                        .fontWeight(model.selectedTab == tab ? .semibold : .regular)
                        .accessibilityAddTraits(model.selectedTab == tab ? .isSelected : [])
                        // The sidebar is visible on every route, so it registers under the actual open route.
                        .coglet(id: "settings.tab.\(tab.rawValue.lowercased())", registry: model.appControls,
                            window: "settings", route: model.selectedTab.rawValue.lowercased(), role: "tab",
                            label: tab.rawValue, verbs: ["press"], value: { .map([.init("kind", .text("none"))]) },
                            failure: model.appControlFailure, invoke: { _ in select() })
                }
                if tabs.isEmpty { Text("No matching settings") }
                Spacer()
                CogletButton("Keyboard shortcuts  ?", id: "settings.shortcuts.open", registry: model.appControls,
                    window: "settings", route: model.selectedTab.rawValue.lowercased(),
                    failure: model.appControlFailure) {
                        #if DEBUG
                        model.wireProofShortcutPresses += 1
                        #endif
                        model.keyboardShortcutsOpen = true
                    }
                    .keyboardShortcut("?", modifiers: [])
            }.padding(16).frame(width: 180)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[4]) {
                    Text(model.selectedTab.rawValue).softmaxFont(SoftmaxDesignTokens.fontTitle, weight: .semibold)
                    DesignGroup(label: "\(model.selectedTab.rawValue) settings") {
                        switch model.selectedTab {
                        case .general:
                            Text("Softmax lives in your menu bar. Close windows without quitting.")
                            // Native radio-group titles use the 13 pt control font, which the OS contrast
                            // audit rejects on this surface; draw the title with the 14 pt body token.
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Appearance").softmaxFont(SoftmaxDesignTokens.fontBody).accessibilityHidden(true)
                                Picker("Appearance", selection: $appearance) {
                                    ForEach(["System", "Light", "Dark"], id: \.self) { choice in
                                        Text(choice).cogletPickerChoice(id: "settings.general.appearance." + choice.lowercased(),
                                            registry: model.appControls, window: "settings", route: "general", label: choice,
                                            selection: $appearance, choice: choice, failure: model.appControlFailure).tag(choice)
                                    }
                                }.pickerStyle(.radioGroup).labelsHidden()
                                    .cogletPickerSelection(id: "settings.general.appearance", registry: model.appControls,
                                        window: "settings", route: "general", label: "Appearance", selection: $appearance,
                                        choices: [("System", "System"), ("Light", "Light"), ("Dark", "Dark")],
                                        failure: model.appControlFailure)
                            }
                            VStack(alignment: .leading, spacing: 4) {
                                Text("Text size").softmaxFont(SoftmaxDesignTokens.fontBody).accessibilityHidden(true)
                                Picker("Text size", selection: $textScale) {
                                    Text("Standard").cogletPickerChoice(id: "settings.general.text_size.standard",
                                        registry: model.appControls, window: "settings", route: "general", label: "Standard",
                                        selection: $textScale, choice: 1.0, failure: model.appControlFailure).tag(1.0)
                                    Text("Large").cogletPickerChoice(id: "settings.general.text_size.large",
                                        registry: model.appControls, window: "settings", route: "general", label: "Large",
                                        selection: $textScale, choice: 1.3, failure: model.appControlFailure).tag(1.3)
                                    Text("Largest").cogletPickerChoice(id: "settings.general.text_size.largest",
                                        registry: model.appControls, window: "settings", route: "general", label: "Largest",
                                        selection: $textScale, choice: 1.6, failure: model.appControlFailure).tag(1.6)
                                }.pickerStyle(.radioGroup).labelsHidden()
                                    .cogletPickerSelection(id: "settings.general.text_size", registry: model.appControls,
                                        window: "settings", route: "general", label: "Text size", selection: $textScale,
                                        choices: [(1.0, "standard"), (1.3, "large"), (1.6, "largest")],
                                        failure: model.appControlFailure)
                            }
                            CogletToggle("Show floating orb", id: "floatingOrbToggle", registry: model.appControls,
                                window: "settings", route: "general", isOn: $model.orbEnabled, failure: model.appControlFailure)
                            Text("Click the orb for chat, right-click for voice, or drag it to a screen edge.")
                            LoginItemControls(model: model, window: "settings", route: "general", idPrefix: "settings.general")
                        case .hotkey:
                            Text("Push-to-talk: \(model.hotkeyName)")
                            CogletButton(model.recordingKey ? "Cancel Recording" : "Record Hotkey…",
                                id: "settings.hotkey.record", registry: model.appControls, window: "settings", route: "hotkey",
                                failure: model.appControlFailure, action: model.recordHotkey)
                            CogletButton("Reset to Right Option", id: "settings.hotkey.reset", registry: model.appControls,
                                window: "settings", route: "hotkey", failure: model.appControlFailure, action: model.resetHotkey)
                            if model.recordingKey { Text("Press a key or chord. Modifier-only keys are supported.") }
                            CogletToggle("Toggle mode (press to start, press again to stop)", id: "settings.hotkey.toggle_mode",
                                registry: model.appControls, window: "settings", route: "hotkey", isOn: $model.toggleMode,
                                failure: model.appControlFailure)
                            Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                            CogletButton("Input Monitoring Settings", id: "settings.hotkey.input_monitoring",
                                registry: model.appControls, window: "settings", route: "hotkey", sensitive: true,
                                failure: model.appControlFailure) { model.openPrivacy("Privacy_ListenEvent") }
                        case .audio:
                            MicrophoneControls(model: model, window: "settings", route: "audio", idPrefix: "settings.audio")
                            Text("Default system input · voice processing and echo cancellation enabled.")
                            CogletToggle("Play sound cues", id: "settings.audio.sound_cues", registry: model.appControls,
                                window: "settings", route: "audio", isOn: $model.startSound, failure: model.appControlFailure)
                        case .hub:
                            HubSettings(model: model)
                        case .coglets:
                            RuntimeSettings(host: model.runtime, openLogs: model.openRuntimeLogs, restart: {
                                #if DEBUG
                                // Isolated UI proofs never start the installed runtime.
                                guard !model.testing else { return }
                                #endif
                                model.ptt.cancel(); model.recognizer.cancel()
                                Task { await model.runtime.restart() }
                            }, registry: model.appControls, failure: model.appControlFailure)
                        case .updates: UpdatesView(updates: model.updates, registry: model.appControls, failure: model.appControlFailure)
                        case .privacy:
                            Text("Audio is captured only while push-to-talk is listening (maximum 120 seconds). Runtime speech delivery is a separate integration.")
                            Text("Microphone: \(model.permissions.microphone.rawValue)")
                            Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                            CogletButton("Microphone Settings", id: "settings.privacy.microphone_settings",
                                registry: model.appControls, window: "settings", route: "privacy", sensitive: true,
                                failure: model.appControlFailure) { model.openPrivacy("Privacy_Microphone") }
                            CogletButton("Input Monitoring Settings", id: "settings.privacy.input_monitoring_settings",
                                registry: model.appControls, window: "settings", route: "privacy", sensitive: true,
                                failure: model.appControlFailure) { model.openPrivacy("Privacy_ListenEvent") }
                            CogletButton("Refresh Permissions", id: "settings.privacy.refresh_permissions",
                                registry: model.appControls, window: "settings", route: "privacy",
                                failure: model.appControlFailure) { model.refreshPermissions() }
                        }
                    }
                    if let message = model.message { Text(message).modifier(SoftmaxErrorText()).textSelection(.enabled) }
                }.padding(SoftmaxDesignTokens.spacing[4]).frame(maxWidth: .infinity, alignment: .leading)
            }
        }.frame(minWidth: 760, minHeight: 540).modifier(SoftmaxTheme())
            .onReceive(NotificationCenter.default.publisher(for: .softmaxFilterInput)) { notification in
                guard let characters = notification.object as? String else { return }
                query += characters; searchFocused = true
            }
            .onExitCommand { if !query.isEmpty { query = "" } else { searchFocused = false } }
            .sheet(isPresented: $model.keyboardShortcutsOpen) {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Keyboard shortcuts").softmaxFont(SoftmaxDesignTokens.fontTitle)
                    Text("⌘K  Commands and settings\n/ or type  Filter settings\n↑ ↓  Select section\nReturn  Open first match\nEsc  Clear search or cancel speech\n⌘,  Settings\n\(model.hotkeyName)  Push-to-talk")
                    CogletButton("Done", id: "keyboard_shortcuts.done", registry: model.appControls,
                        window: "keyboard_shortcuts", route: "shortcuts", failure: model.appControlFailure) {
                            model.keyboardShortcutsOpen = false
                        }.keyboardShortcut(.cancelAction)
                }.padding(24)
                    .background(AppSheetOwner { old, new in
                        if new != nil || model.keyboardShortcutsNativeWindow === old {
                            model.keyboardShortcutsNativeWindow = new
                            model.synchronizeAppSurface?()
                        }
                    })
            }
    }
}

struct LoginItemControls: View {
    @ObservedObject var model: ShellModel
    let window: String
    let route: String
    let idPrefix: String
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[1]) {
            Text("Launch at login: \(model.loginItem.rawValue)")
            // Login approval paths are sensitive (ADR 0030); the sensitive flag follows every alias.
            CogletButton("Enable Launch at Login", id: "\(idPrefix).login_enable", registry: model.appControls,
                window: window, route: route, sensitive: true, failure: model.appControlFailure) { model.setLoginItem(enabled: true) }
            CogletButton("Disable Launch at Login", id: "\(idPrefix).login_disable", registry: model.appControls,
                window: window, route: route, sensitive: true, failure: model.appControlFailure) { model.setLoginItem(enabled: false) }
            CogletButton("Login Item Settings", id: "\(idPrefix).login_settings", registry: model.appControls,
                window: window, route: route, sensitive: true, failure: model.appControlFailure) {
                if !model.testing { SMAppService.openSystemSettingsLoginItems() }
            }
        }
    }
}

struct OnboardingView: View {
    @ObservedObject var model: ShellModel
    var openSettings: () -> Void
    var finish: () -> Void
    private var step: Int {
        get { model.onboardingStep }
        nonmutating set { model.onboardingStep = newValue }
    }
    private let titles = ["Welcome to Softmax", "Voice & shortcut", "Hub & ready"]
    private var route: String { ["welcome", "permissions", "hub"][step] }
    /// Both ViewThatFits layouts use these ids; only the chosen layout is mounted, and a
    /// duplicate mount would surface through appControlFailure rather than be hidden.
    @ViewBuilder private var inputMonitoringButtons: some View {
        CogletButton("Request Input Monitoring", id: "onboarding.permissions.request_input_monitoring",
            registry: model.appControls, window: "onboarding", route: "permissions", sensitive: true,
            failure: model.appControlFailure) { model.requestInputMonitoring() }
        CogletButton("Input Monitoring Settings", id: "onboarding.permissions.input_monitoring_settings",
            registry: model.appControls, window: "onboarding", route: "permissions", sensitive: true,
            failure: model.appControlFailure) { model.openPrivacy("Privacy_ListenEvent") }
    }
    private func advance() {
        if step < 2 { step += 1 } else { finish() }
    }
    @ViewBuilder private var advanceButton: some View {
        #if DEBUG
        if model.testing && step == 0 && ProcessInfo.processInfo.arguments.contains("--catalog-omit-welcome-continue-registration") {
            // App-local negative control: preserve native semantics, omit only the mount.
            Button("Continue", action: advance)
                .accessibilityIdentifier("onboarding.continue")
                .keyboardShortcut(.defaultAction)
        } else {
            registeredAdvanceButton
        }
        #else
        registeredAdvanceButton
        #endif
    }
    private var registeredAdvanceButton: some View {
        CogletButton(step < 2 ? "Continue" : "Finish", id: step < 2 ? "onboarding.continue" : "onboarding.finish",
            registry: model.appControls, window: "onboarding", route: route,
            failure: model.appControlFailure, action: advance)
            .keyboardShortcut(.defaultAction)
    }
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
            Text("\(step + 1) of 3").softmaxFont(SoftmaxDesignTokens.fontBody, weight: .medium)
            Text(titles[step]).softmaxFont(SoftmaxDesignTokens.fontDisplay, weight: .semibold)
            ScrollView {
                VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
                Image("onboarding-\(["welcome", "permissions", "hub"][step])").resizable().scaledToFit()
                    .frame(height: step == 1 ? 48 : 100).frame(maxWidth: .infinity).accessibilityHidden(true)
                DesignGroup(label: titles[step]) {
                    switch step {
                    case 0:
                        Text("Speak without switching apps.").softmaxFont(SoftmaxDesignTokens.fontTitle)
                        Text("Softmax lives in your menu bar. Face stays nearby when you need it, out of the way when you don’t.")
                        Text("Microphone and global shortcut access are optional. You choose when audio is live.")
                    case 1:
                        Text("Let Softmax hear you.").softmaxFont(SoftmaxDesignTokens.fontTitle, weight: .semibold)
                        Text("Microphone access is used when you hold to talk. Release the shortcut to send. No always-on listening in v1.")
                        MicrophoneControls(model: model, window: "onboarding", route: "permissions", idPrefix: "onboarding.permissions")
                        Divider()
                        Text("Input Monitoring lets Right Option activate push-to-talk outside Softmax. Typed text is not retained.")
                        Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                        ViewThatFits(in: .horizontal) {
                            HStack { inputMonitoringButtons }
                            VStack(alignment: .leading) { inputMonitoringButtons }
                        }
                    default:
                        HubSettings(model: model, window: "onboarding", route: "hub", idPrefix: "onboarding.hub")
                        LoginItemControls(model: model, window: "onboarding", route: "hub", idPrefix: "onboarding.hub")
                        Text("Voice not set up · return to Settings to review permissions and connection.")
                    }
                }
                if let message = model.message { Text(message).modifier(SoftmaxErrorText()) }
                }
            }
            HStack {
                CogletButton("Open Settings…", id: "onboarding.open_settings", registry: model.appControls,
                    window: "onboarding", route: route, failure: model.appControlFailure, action: openSettings)
                Spacer()
                if step > 0 {
                    CogletButton("Back", id: "onboarding.back", registry: model.appControls, window: "onboarding",
                        route: route, failure: model.appControlFailure) { step -= 1 }
                }
                advanceButton
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(minWidth: 620, minHeight: 600).modifier(SoftmaxTheme())
    }
}
