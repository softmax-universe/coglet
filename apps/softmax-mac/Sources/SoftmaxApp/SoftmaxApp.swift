import SwiftUI
import AppKit
import AVFoundation
import CoreGraphics
import ServiceManagement
import SoftmaxCore
import FaceMac
import Combine

@main
struct SoftmaxApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            Button("Open Face") { delegate.showFace() }
            Button("Open Chat") { delegate.showConversation() }
            Button("Open Voice") { delegate.showConversation(voice: true) }
            Text("Push-to-talk: not configured")
            Text("Hub: offline (not connected)")
            Divider()
            Button("Settings…") { delegate.showSettings() }.keyboardShortcut(",")
            Button("Check for Updates…") { delegate.model.updates.checkForUpdates() }
                .disabled(!delegate.model.updates.enabled)
            Divider()
            Button("Quit Softmax") { NSApp.terminate(nil) }.keyboardShortcut("q")
        } label: {
            StatusLabel(model: delegate.model)
        }
    }
}

struct StatusLabel: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        Label("Softmax: \(model.machine.state.rawValue)", systemImage: model.machine.state.symbolName)
    }
}

@MainActor
final class ShellModel: ObservableObject {
    let updates = UpdateController()
    let testing = ProcessInfo.processInfo.arguments.contains("--ui-testing")
    @Published var machine: AppStateMachine
    @Published var permissions = PermissionSnapshot(microphone: .unknown, inputMonitoring: .unknown)
    @Published var loginItem: LoginItemStatus = .unknown
    @Published var message: String?
    @Published var selectedTab: SettingsTab = .general
    @Published var voicePanelSelected = false
    @Published var orbEnabled: Bool = false {
        didSet { if !testing { orbPreferences.isEnabled = orbEnabled } }
    }
    private let orbPreferences = OrbPreferences()

    init() {
        let testing = ProcessInfo.processInfo.arguments.contains("--ui-testing")
        machine = AppStateMachine(onboardingComplete: !testing && UserDefaults.standard.bool(forKey: "onboardingComplete"))
        refreshPermissions()
        orbEnabled = !testing && orbPreferences.isEnabled
    }

    func refreshPermissions() {
        guard !testing else { return }
        permissions = PermissionSnapshot(
            microphone: .microphone(rawValue: AVCaptureDevice.authorizationStatus(for: .audio).rawValue),
            inputMonitoring: .inputMonitoring(granted: CGPreflightListenEventAccess()))
        loginItem = LoginItemStatus(serviceRawValue: SMAppService.mainApp.status.rawValue)
    }

    func requestMicrophone() async {
        guard !testing else { return }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            _ = await AVCaptureDevice.requestAccess(for: .audio)
        }
        refreshPermissions()
    }

    func requestInputMonitoring() {
        guard !testing else { return }
        _ = CGRequestListenEventAccess()
        refreshPermissions()
    }

    func setLoginItem(enabled: Bool) {
        guard !testing else { return }
        do {
            if enabled { try SMAppService.mainApp.register() }
            else { try SMAppService.mainApp.unregister() }
            message = nil
        } catch {
            message = "Launch at login: \(error.localizedDescription)"
        }
        refreshPermissions()
    }

    func finishOnboarding() {
        if !testing { UserDefaults.standard.set(true, forKey: "onboardingComplete") }
        machine.send(.finishOnboarding)
    }

    func openPrivacy(_ pane: String) {
        guard !testing, let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") else { return }
        NSWorkspace.shared.open(url)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    let model = ShellModel()
    private var settingsWindow: NSWindow?
    private var onboardingWindow: NSWindow?
    private var facePanel: NSPanel?
    private lazy var orbOverlay = OrbOverlay(model: model)
    private var orbSubscription: AnyCancellable?

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        orbSubscription = model.$orbEnabled.removeDuplicates().sink { [weak self] enabled in
            self?.orbOverlay.setVisible(enabled)
        }
        if model.machine.state == .onboarding { showOnboarding() }
    }

    func applicationDidBecomeActive(_ notification: Notification) { model.refreshPermissions() }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func showSettings(tab: SettingsTab? = nil) {
        if let tab { model.selectedTab = tab }
        if settingsWindow == nil {
            settingsWindow = window(title: "Softmax Settings", size: NSSize(width: 680, height: 420),
                                    view: SettingsView(model: model))
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    func showOnboarding() {
        onboardingWindow = window(title: "Welcome to Softmax", size: NSSize(width: 560, height: 400),
            view: OnboardingView(model: model, openSettings: { [weak self] in self?.showSettings() },
                finish: { [weak self] in
                    self?.model.finishOnboarding()
                    self?.onboardingWindow?.close()
                }))
        NSApp.activate(ignoringOtherApps: true)
        onboardingWindow?.makeKeyAndOrderFront(nil)
    }

    func showFace() {
        if facePanel == nil {
            let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 220, height: 220),
                                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.level = .floating
            panel.isReleasedWhenClosed = false
            panel.isMovableByWindowBackground = true
            panel.hidesOnDeactivate = false
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            panel.contentView = NSHostingView(rootView: FacePlaceholder(model: model, close: { [weak panel] in panel?.orderOut(nil) }))
            panel.center()
            facePanel = panel
        }
        facePanel?.orderFrontRegardless()
    }

    func showConversation(voice: Bool = false) { orbOverlay.showChat(voice: voice) }

    private func window<V: View>(title: String, size: NSSize, view: V) -> NSWindow {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                              styleMask: [.titled, .closable, .miniaturizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentView = NSHostingView(rootView: view)
        window.center()
        return window
    }
}

struct FacePlaceholder: View {
    @ObservedObject var model: ShellModel
    var close: () -> Void
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: model.machine.state.symbolName).font(.system(size: 56))
            Text("Face · \(model.machine.state.rawValue)")
            Button("Hide Face", action: close)
        }.frame(maxWidth: .infinity, maxHeight: .infinity).background(.regularMaterial)
    }
}
