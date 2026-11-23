import SwiftUI
import AppKit
import AVFoundation
import CoreGraphics
import ServiceManagement
import SoftmaxCore
import FaceMac

@main
struct SoftmaxApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            Button("Open Face") { delegate.showFace() }
            Text("Push-to-talk: \(delegate.model.hotkeyName)")
            Text("Hub: offline (not connected)")
            Divider()
            Button("Settings…") { delegate.showSettings() }.keyboardShortcut(",")
            Button("Check for Updates…") { delegate.showSettings(tab: .updates) }
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
        Label("Softmax: \(model.state.rawValue)", systemImage: model.state.symbolName)
    }
}

@MainActor
final class ShellModel: ObservableObject {
    let testing = ProcessInfo.processInfo.arguments.contains("--ui-testing")
    @Published var machine: AppStateMachine {
        didSet {
            // Runtime owns external speech/hub transitions; keep barge-in in sync.
            if machine.state != oldValue.state { ptt.setState(machine.state) }
        }
    }
    @Published var permissions = PermissionSnapshot(microphone: .unknown, inputMonitoring: .unknown)
    @Published var loginItem: LoginItemStatus = .unknown
    @Published var message: String?
    @Published var selectedTab: SettingsTab = .general
    @Published var talkState: AppState = .idle
    @Published var level: Float = 0
    @Published var elapsed: TimeInterval = 0
    @Published var recordingKey = false
    @Published var hotkeyName = "Right Option"
    @Published var toggleMode = UserDefaults.standard.bool(forKey: "talkToggle") {
        didSet { ptt.mode = toggleMode ? .toggle : .hold; UserDefaults.standard.set(toggleMode, forKey: "talkToggle") }
    }
    @Published var startSound = UserDefaults.standard.bool(forKey: "talkStartSound") {
        didSet { UserDefaults.standard.set(startSound, forKey: "talkStartSound") }
    }
    let keySource = GlobalTalkKey()
    lazy var ptt = PushToTalk(keys: keySource, audio: EngineAudioCapture())
    var onTalkChange: (() -> Void)?
    var routePermissions: (() -> Void)?
    /// Runtime/Face installs the actual synchronous playback stop before speaking.
    var stopFacePlayback: (() -> Void)?
    func beginFacePlayback(stop: @escaping () -> Void) {
        stopFacePlayback = stop
        ptt.setState(.speaking)
    }
    func finishFacePlayback() {
        stopFacePlayback = nil
        ptt.setState(.idle)
    }
    func finishSpeechInput() { ptt.setState(.idle) }
    var state: AppState {
        talkState == .idle ? machine.state : talkState
    }

    func configureTalk() {
        guard !testing else { return }
        if UserDefaults.standard.object(forKey: "talkKeyCode") != nil {
            keySource.keyCode = UserDefaults.standard.integer(forKey: "talkKeyCode")
            keySource.modifiers = UInt64(UserDefaults.standard.integer(forKey: "talkModifiers"))
        }
        updateHotkeyName()
        keySource.onRecorded = { [weak self] code, modifiers in
            guard let self else { return }
            keySource.keyCode = code; keySource.modifiers = modifiers
            UserDefaults.standard.set(code, forKey: "talkKeyCode")
            UserDefaults.standard.set(Int(modifiers), forKey: "talkModifiers")
            recordingKey = false; updateHotkeyName()
        }
        ptt.mode = toggleMode ? .toggle : .hold
        ptt.routeToPermissions = { [weak self] in self?.routePermissions?() }
        ptt.permissionProvider = {
            PermissionSnapshot(
                microphone: .microphone(rawValue: AVCaptureDevice.authorizationStatus(for: .audio).rawValue),
                inputMonitoring: .inputMonitoring(granted: CGPreflightListenEventAccess()))
        }
        ptt.stopPlayback = { [weak self] in self?.stopFacePlayback?(); self?.stopFacePlayback = nil }
        ptt.playStartSound = { [weak self] in if self?.startSound == true { NSSound(named: "Tink")?.play() } }
        ptt.onChange = { [weak self] in
            guard let self else { return }
            talkState = ptt.state; level = ptt.inputLevel; elapsed = ptt.elapsed
            onTalkChange?()
        }
        ptt.permissions = permissions
        ptt.start()
    }
    func recordHotkey() {
        recordingKey.toggle(); keySource.recording = recordingKey
    }
    func resetHotkey() {
        keySource.recording = false; recordingKey = false
        ptt.cancel(); keySource.keyCode = 61; keySource.modifiers = 0
        UserDefaults.standard.removeObject(forKey: "talkKeyCode")
        UserDefaults.standard.removeObject(forKey: "talkModifiers")
        updateHotkeyName()
    }
    private func updateHotkeyName() {
        hotkeyName = keySource.keyCode == 61 && keySource.modifiers == 0 ? "Right Option" :
            "Key \(keySource.keyCode) · modifiers \(keySource.modifiers)"
    }

    init() {
        let testing = ProcessInfo.processInfo.arguments.contains("--ui-testing")
        machine = AppStateMachine(onboardingComplete: !testing && UserDefaults.standard.bool(forKey: "onboardingComplete"))
        refreshPermissions()
    }

    func refreshPermissions() {
        guard !testing else { return }
        permissions = PermissionSnapshot(
            microphone: .microphone(rawValue: AVCaptureDevice.authorizationStatus(for: .audio).rawValue),
            inputMonitoring: .inputMonitoring(granted: CGPreflightListenEventAccess()))
        loginItem = LoginItemStatus(serviceRawValue: SMAppService.mainApp.status.rawValue)
        ptt.permissions = permissions
    }

    func requestMicrophone() async {
        guard !testing else { return }
        _ = await AVCaptureDevice.requestAccess(for: .audio)
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
        configureTalk()
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
    private let talkHUD = TalkHUD()

    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.accessory)
        model.onTalkChange = { [weak self] in
            guard let self else { return }
            talkHUD.update(model: model)
        }
        model.routePermissions = { [weak self] in self?.showSettings(tab: .privacy) }
        if model.state == .onboarding { showOnboarding() }
        else { model.configureTalk() }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        model.refreshPermissions()
        if !model.testing && model.machine.state != .onboarding && model.permissions.canUseGlobalPushToTalk {
            model.ptt.start()
        }
    }
    func applicationWillTerminate(_ notification: Notification) { model.ptt.shutdown() }
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
            Image(systemName: model.state.symbolName).font(.system(size: 56))
            Text("Face · \(model.state.rawValue)")
            Button("Hide Face", action: close)
        }.frame(maxWidth: .infinity, maxHeight: .infinity).background(.regularMaterial)
    }
}
