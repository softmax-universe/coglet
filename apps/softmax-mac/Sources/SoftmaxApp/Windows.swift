import SwiftUI
import ServiceManagement

enum SettingsTab: String, CaseIterable, Identifiable {
    case general = "General", hotkey = "Hotkey", audio = "Audio", hub = "Hub"
    case coglets = "Coglets", updates = "Updates", privacy = "Privacy"
    var id: Self { self }
    var keywords: String {
        switch self {
        case .general: "login startup appearance theme dock orb face"
        case .hotkey: "shortcut keyboard hold toggle push talk"
        case .audio: "microphone input output sound volume"
        case .hub: "account connect enrollment offline"
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
    @State private var shortcuts = false
    private var tabs: [SettingsTab] {
        SettingsTab.allCases.filter { query.isEmpty || ($0.rawValue + " " + $0.keywords).localizedCaseInsensitiveContains(query) }
    }
    var body: some View {
        HStack(spacing: 0) {
            VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
                TextField("Search settings  ⌘K", text: $query).focused($searchFocused)
                    .accessibilityLabel("Search settings").onSubmit {
                        if !tabs.contains(model.selectedTab), let tab = tabs.first { model.selectedTab = tab }
                    }
                    .onMoveCommand { direction in
                        guard !tabs.isEmpty else { return }
                        let index = tabs.firstIndex(of: model.selectedTab) ?? 0
                        if direction == .down { model.selectedTab = tabs[min(index + 1, tabs.count - 1)] }
                        if direction == .up { model.selectedTab = tabs[max(index - 1, 0)] }
                    }
                Text("\(tabs.count) sections").font(.system(size: SoftmaxDesignTokens.fontCaption))
                ForEach(tabs) { tab in
                    Button { model.selectedTab = tab } label: {
                        Text(tab.rawValue).frame(maxWidth: .infinity, alignment: .leading).padding(6)
                            .background(model.selectedTab == tab ? Color.accentColor.opacity(0.14) : .clear, in: RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusControl))
                    }.buttonStyle(.borderless)
                        .fontWeight(model.selectedTab == tab ? .semibold : .regular)
                        .accessibilityAddTraits(model.selectedTab == tab ? .isSelected : [])
                }
                if tabs.isEmpty { Text("No matching settings") }
                Spacer()
                Button("Keyboard shortcuts  ?") { shortcuts = true }
            }.padding(16).frame(width: 180)
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[4]) {
                    Text(model.selectedTab.rawValue).font(.system(size: SoftmaxDesignTokens.fontTitle, weight: .semibold))
                    DesignGroup {
                        switch model.selectedTab {
                        case .general:
                            Text("Softmax lives in your menu bar. Close windows without quitting.")
                            Picker("Appearance", selection: $appearance) { ForEach(["System", "Light", "Dark"], id: \.self) { Text($0) } }
                            Picker("Text size", selection: $textScale) {
                                Text("Standard").tag(1.0); Text("Large").tag(1.3); Text("Largest").tag(1.6)
                            }
                            Toggle("Show floating orb", isOn: $model.orbEnabled).accessibilityIdentifier("floatingOrbToggle")
                            Text("Click the orb for chat, right-click for voice, or drag it to a screen edge.")
                            LoginItemControls(model: model)
                        case .hotkey:
                            Text("Push-to-talk: \(model.hotkeyName)")
                            Button(model.recordingKey ? "Cancel Recording" : "Record Hotkey…") { model.recordHotkey() }
                            Button("Reset to Right Option") { model.resetHotkey() }
                            if model.recordingKey { Text("Press a key or chord. Modifier-only keys are supported.") }
                            Toggle("Toggle mode (press to start, press again to stop)", isOn: $model.toggleMode)
                            Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                            Button("Input Monitoring Settings") { model.openPrivacy("Privacy_ListenEvent") }
                        case .audio:
                            MicrophoneControls(model: model)
                            Text("Default system input · voice processing and echo cancellation enabled.")
                            Toggle("Play sound cues", isOn: $model.startSound)
                        case .hub:
                            Text("Hub disconnected").fontWeight(.semibold)
                            Text("No hub configured. Enrollment and connectivity are not implemented yet.")
                        case .coglets:
                            Text("No coglets loaded").fontWeight(.semibold)
                            Text("Local runtime integration is not implemented yet. No running state or grants can be reported.")
                        case .updates: UpdatesView(updates: model.updates)
                        case .privacy:
                            Text("Audio is captured only while push-to-talk is listening (maximum 120 seconds). Runtime speech delivery is a separate integration.")
                            Text("Microphone: \(model.permissions.microphone.rawValue)")
                            Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                            Button("Microphone Settings") { model.openPrivacy("Privacy_Microphone") }
                            Button("Input Monitoring Settings") { model.openPrivacy("Privacy_ListenEvent") }
                            Button("Refresh Permissions") { model.refreshPermissions() }
                        }
                    }
                    if let message = model.message { Text(message).foregroundStyle(.red).textSelection(.enabled) }
                }.padding(SoftmaxDesignTokens.spacing[4]).frame(maxWidth: .infinity, alignment: .leading)
            }
        }.frame(minWidth: 760, minHeight: 540).modifier(SoftmaxTheme())
            .background {
                Button("Search", action: { searchFocused = true }).keyboardShortcut("k").hidden()
            }
            .onExitCommand { if !query.isEmpty { query = "" } else { searchFocused = false } }
            .sheet(isPresented: $shortcuts) {
                VStack(alignment: .leading, spacing: 16) {
                    Text("Keyboard shortcuts").font(.title2)
                    Text("⌘K  Search settings\n↑ ↓  Select section\nReturn  Open first match\nEsc  Clear search or cancel speech\n⌘,  Settings\nRight Option  Push-to-talk")
                    Button("Done") { shortcuts = false }.keyboardShortcut(.cancelAction)
                }.padding(24)
            }
    }
}

struct LoginItemControls: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[1]) {
            Text("Launch at login: \(model.loginItem.rawValue)")
            Button("Enable Launch at Login") { model.setLoginItem(enabled: true) }
            Button("Disable Launch at Login") { model.setLoginItem(enabled: false) }
            Button("Login Item Settings") { if !model.testing { SMAppService.openSystemSettingsLoginItems() } }
        }
    }
}

struct OnboardingView: View {
    @ObservedObject var model: ShellModel
    var openSettings: () -> Void
    var finish: () -> Void
    @State var step = 0
    private let titles = ["Welcome to Softmax", "Voice & shortcut", "Hub & ready"]
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
            Text("\(step + 1) of 3").font(.system(size: SoftmaxDesignTokens.fontCaption))
            Text(titles[step]).font(.system(size: SoftmaxDesignTokens.fontDisplay, weight: .semibold))
            ScrollView {
                Image("onboarding-\(["welcome", "permissions", "hub"][step])").resizable().scaledToFit()
                    .frame(height: 100).frame(maxWidth: .infinity).accessibilityHidden(true)
                DesignGroup {
                    switch step {
                    case 0:
                        Text("Speak without switching apps.").font(.system(size: SoftmaxDesignTokens.fontTitle))
                        Text("Softmax lives in your menu bar. Face stays nearby when you need it, out of the way when you don’t.")
                        Text("Microphone and global shortcut access are optional. You choose when audio is live.")
                    case 1:
                        Text("Let Softmax hear you.").font(.system(size: SoftmaxDesignTokens.fontTitle, weight: .semibold))
                        Text("Microphone access is used when you hold to talk. Release the shortcut to send. No always-on listening in v1.")
                        MicrophoneControls(model: model)
                        Divider()
                        Text("Input Monitoring lets Right Option activate push-to-talk outside Softmax. Typed text is not retained.")
                        Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                        Button("Request Input Monitoring") { model.requestInputMonitoring() }
                        Button("Input Monitoring Settings") { model.openPrivacy("Privacy_ListenEvent") }
                    default:
                        Text("Hub disconnected").fontWeight(.semibold)
                        Text("Enrollment is not implemented yet. Voice delivery requires a runtime connection; microphone permission alone does not make voice ready.")
                        LoginItemControls(model: model)
                        Text("Voice not set up · return to Settings to review permissions and connection.")
                    }
                }
                if let message = model.message { Text(message).foregroundStyle(.red) }
            }
            HStack {
                Button("Open Settings…", action: openSettings)
                Spacer()
                if step > 0 { Button("Back") { step -= 1 } }
                Button(step < 2 ? "Continue" : "Finish") { if step < 2 { step += 1 } else { finish() } }
                    .keyboardShortcut(.defaultAction)
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(minWidth: 620, minHeight: 500).modifier(SoftmaxTheme())
    }
}
