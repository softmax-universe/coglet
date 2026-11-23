import SwiftUI
import ServiceManagement

enum SettingsTab: String, CaseIterable, Identifiable {
    case general = "General", hotkey = "Hotkey", audio = "Audio", hub = "Hub"
    case coglets = "Coglets", updates = "Updates", privacy = "Privacy"
    var id: Self { self }
}

struct SettingsView: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            HStack {
                ForEach(SettingsTab.allCases) { tab in
                    Button(tab.rawValue) { model.selectedTab = tab }
                        .accessibilityAddTraits(model.selectedTab == tab ? .isSelected : [])
                }
            }
            Divider()
            Text(model.selectedTab.rawValue).font(.title2)
            switch model.selectedTab {
            case .general:
                Text("Softmax lives in your menu bar. Close windows without quitting.")
                LoginItemControls(model: model)
            case .hotkey:
                Text("Global push-to-talk is not wired in this skeleton. Hotkey configuration will appear here.")
                Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
            case .audio:
                Text("Microphone: \(model.permissions.microphone.rawValue)")
                Text("Device selection and audio capture are not connected yet.")
            case .hub:
                Text("Offline — no hub configured. Enrollment and connectivity are not implemented yet.")
            case .coglets:
                Text("No coglets loaded. Local runtime integration is not implemented yet.")
            case .updates:
                Text("Updates are not connected in this development build. Sparkle, signing and notarization follow in the release integration.")
            case .privacy:
                Text("No audio is captured or sent by this skeleton.")
                Text("Microphone: \(model.permissions.microphone.rawValue)")
                Text("Input Monitoring: \(model.permissions.inputMonitoring.rawValue)")
                Button("Microphone Settings") { model.openPrivacy("Privacy_Microphone") }
                Button("Input Monitoring Settings") { model.openPrivacy("Privacy_ListenEvent") }
                Button("Refresh Permissions") { model.refreshPermissions() }
            }
            if let message = model.message { Text(message).foregroundStyle(.red).textSelection(.enabled) }
            Spacer()
        }.padding(24).frame(width: 680, height: 420)
    }
}

struct LoginItemControls: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Launch at login: \(model.loginItem.rawValue)")
            HStack {
                Button("Enable Launch at Login") { model.setLoginItem(enabled: true) }
                Button("Disable Launch at Login") { model.setLoginItem(enabled: false) }
                Button("Login Item Settings") {
                    if !model.testing { SMAppService.openSystemSettingsLoginItems() }
                }
            }
        }
    }
}

struct OnboardingView: View {
    @ObservedObject var model: ShellModel
    var openSettings: () -> Void
    var finish: () -> Void
    @State private var step = 0
    private let titles = ["Microphone", "Input Monitoring", "Launch at login"]

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("Welcome to Softmax").font(.largeTitle)
            Text("Step \(step + 1) of 3 · \(titles[step])").font(.headline)
            switch step {
            case 0:
                Text("Allow the microphone for future push-to-talk conversations. You can continue without access and enable it later in Privacy settings.")
                Text("Status: \(model.permissions.microphone.rawValue)")
                Button("Allow Microphone") { Task { await model.requestMicrophone() } }
                Button("Microphone Settings") { model.openPrivacy("Privacy_Microphone") }
            case 1:
                Text("Input Monitoring will allow the global push-to-talk key to work outside Softmax. The hotkey itself is not connected in this skeleton.")
                Text("Status: \(model.permissions.inputMonitoring.rawValue)")
                Button("Request Input Monitoring") { model.requestInputMonitoring() }
                Button("Input Monitoring Settings") { model.openPrivacy("Privacy_ListenEvent") }
            default:
                Text("Optionally start Softmax when you sign in. macOS may require approval in Login Items.")
                LoginItemControls(model: model)
            }
            if let message = model.message { Text(message).foregroundStyle(.red) }
            Spacer()
            HStack {
                Button("Open Settings…", action: openSettings)
                Spacer()
                if step > 0 { Button("Back") { step -= 1 } }
                Button(step < 2 ? "Continue" : "Finish") {
                    if step < 2 { step += 1 } else { finish() }
                }.keyboardShortcut(.defaultAction)
            }
        }.padding(24).frame(width: 560, height: 400)
    }
}
