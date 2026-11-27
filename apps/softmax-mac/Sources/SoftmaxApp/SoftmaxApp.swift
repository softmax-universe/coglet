import SwiftUI
import AppKit
import AVFoundation
import CoreGraphics
import ServiceManagement
import SoftmaxCore
import FaceMac
import Combine
import SoftmaxRuntime

// Diagnostic only: no I/O during navigation, no text or arbitrary model content.
@MainActor
enum SettingsArrowCapture {
    enum Phase: String, Codable {
        case arrival, downEnter, downExit, upEnter, upExit, submitEnter, submitExit
        case selectionSet, viewAppear, viewDisappear, viewSelection, viewQuery, viewFocus
    }
    struct Metadata {
        var selected: SettingsTab?
        var previous: SettingsTab?
        var keyCode: UInt16?
        var eventTime: Double?
        var queryEmpty: Bool?
        var matchCount: Int?
        var searchFocused: Bool?
    }
    struct Record: Codable {
        let sequence: Int
        let uptime: Double
        let phase: Phase
        let active: Bool
        let hasKeyWindow: Bool
        let responder: String?
        let selected: String?
        let previous: String?
        let keyCode: UInt16?
        let eventTime: Double?
        let queryEmpty: Bool?
        let matchCount: Int?
        let searchFocused: Bool?
    }
    struct Document: Encodable {
        let schema = 1
        let boundary = "finalEscapeBeforeDispatch"
        let complete = true
        let dropped: Int
        let records: [Record]
    }
#if DEBUG
    static let outputURL: URL? = {
        guard ProcessInfo.processInfo.arguments.contains("--ui-testing"),
              let path = ProcessInfo.processInfo.environment["SOFTMAX_SETTINGS_ARROW_PROBE"] else { return nil }
        return URL(fileURLWithPath: path)
    }()
    static var records: [Record] = {
        var result: [Record] = []
        result.reserveCapacity(1024)
        return result
    }()
    static var dropped = 0
    static var exported = false
#endif
    static func record(_ phase: Phase, metadata: () -> Metadata) {
#if DEBUG
        guard outputURL != nil, !exported else { return }
        guard records.count < 1024 else { dropped += 1; return }
        let value = metadata()
        let responder = (NSApp.keyWindow?.firstResponder).map { String(String(describing: type(of: $0)).prefix(128)) }
        records.append(Record(sequence: records.count, uptime: ProcessInfo.processInfo.systemUptime,
            phase: phase, active: NSApp.isActive, hasKeyWindow: NSApp.keyWindow != nil,
            responder: responder, selected: value.selected?.rawValue, previous: value.previous?.rawValue,
            keyCode: value.keyCode, eventTime: value.eventTime, queryEmpty: value.queryEmpty,
            matchCount: value.matchCount, searchFocused: value.searchFocused))
#endif
    }
    // Only the final original Escape calls this, after earlier assertions/input.
    // Its timing is not usable as navigation-latency evidence.
    static func exportBeforeEscape() {
#if DEBUG
        guard let outputURL, !exported else { return }
        exported = true
        do {
            let data = try JSONEncoder().encode(Document(dropped: dropped, records: records))
            try data.write(to: outputURL, options: .atomic)
        } catch {
            // Missing export is an explicit diagnostic failure in the driver.
        }
#endif
    }
}

@MainActor
func settingsArrowCapture(_ phase: SettingsArrowCapture.Phase,
                          selected: @autoclosure () -> SettingsTab,
                          queryEmpty: @autoclosure () -> Bool,
                          matchCount: @autoclosure () -> Int,
                          searchFocused: @autoclosure () -> Bool) {
    SettingsArrowCapture.record(phase) {
        .init(selected: selected(), queryEmpty: queryEmpty(), matchCount: matchCount(), searchFocused: searchFocused())
    }
}

/// Process entry. Registers app defaults before SwiftUI creates NSApplication
/// or the delegate, then hands control to the SwiftUI app.
@main
enum SoftmaxEntry {
    @MainActor static func main() {
        // macOS 26 AppKit AutoFill heuristics start a per-app
        // SafariPlatformSupport.Helper on text input. Every native failure of
        // testSettingsFocusedSearchArrowNavigation lost the first Down while
        // that helper was starting (XCTest synthesis stalled ~5 s). Softmax has
        // no account login; its only secure fields hold pairing secrets that are
        // pasted or imported, so system AutoFill suggestions are unwanted.
        // NSAutoFillHeuristicsEnabled is an undocumented AppKit default: the
        // registration domain keeps user/argument overrides possible, and
        // testSettingsSearchDoesNotStartAutoFillHelper goes red if it stops working.
        UserDefaults.standard.register(defaults: ["NSAutoFillHeuristicsEnabled": false])
        SoftmaxApp.main()
    }
}

struct SoftmaxApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            SoftmaxWindowRoot {
                SoftmaxMenuView(model: delegate.model, showFace: { delegate.showFace() },
                    showChat: { delegate.showConversation(voice: $0) }, settings: { delegate.showSettings() },
                    about: { delegate.showAbout() }, quit: { delegate.quit() }, searchCommands: { delegate.showCommands() })
            }
        } label: {
            StatusLabel(model: delegate.model)
        }.menuBarExtraStyle(.window)
    }
}

struct StatusLabel: View {
    @ObservedObject var model: ShellModel
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var frame = 0
    private var animated: Bool { !reduceMotion && [.listening, .speaking].contains(model.menuPresentation) }
    var body: some View {
        Image("menu-\(model.menuPresentation.rawValue)\(animated ? "-\(frame)" : "")").renderingMode(.template)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Softmax, \(model.menuPresentation.label), \(model.hubStatus)")
            .help("Softmax, \(model.menuPresentation.label), \(model.hubStatus)")
            .task(id: model.menuPresentation.rawValue + String(reduceMotion)) {
                frame = 0
                while animated && !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(Int(SoftmaxDesignTokens.motionMenuFrameMs)))
                    guard !Task.isCancelled else { return }
                    frame = (frame + 1) % 4
                }
            }
    }
}

@MainActor
final class ShellModel: ObservableObject {
    let updates = UpdateController()
    let runtime = RuntimeHost()
    lazy var faceHost = FaceHost(shell: self)
    let recognizer = LocalSpeechInput()
    @Published private(set) var hubConnection: HubConnection?
    var hubStatus: String {
        switch hubConnection?.state ?? .offline {
        case .connected: return "Router connected"
        case .connecting: return "Router connecting…"
        case .offline: return "Router offline"
        }
    }
    /// Startup is independent of microphone permissions and voice lifecycle.
    /// Isolated UI/snapshot runs never instantiate a Keychain store or transport.
    func configureHub() {
        guard !testing else { return }
        if hubConnection == nil {
            let helper = Bundle.main.bundleURL.appendingPathComponent("Contents/Helpers/softmax-hub-link")
            let connection = HubConnection(account: HubAccount(store: KeychainHubStore()),
                                           link: ProcessHubLink(executable: helper))
            connection.changed = { [weak self] in self?.objectWillChange.send() }
            hubConnection = connection
        }
        hubConnection?.start()
    }
    private var runtimeSubscription: AnyCancellable?
    private var speechDelivery: Task<Void, Never>?
    func configureRuntime() {
        guard !testing, speechDelivery == nil else { return }
        runtimeSubscription = runtime.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
        runtime.onStopOutput = { [weak self] in self?.faceHost.stopRuntimeOutput() }
        runtime.onAction = { [weak self] action in
            guard let self else { return }
            faceHost.perform(action) { [weak self] error in self?.runtime.complete(action, error: error) }
        }
        Task { await runtime.start() }
        speechDelivery = Task { [weak self] in
            guard let self else { return }
            for await input in ptt.speechInput {
                guard case .utterance(let utterance) = input,
                      ptt.operationID == utterance.operationID, ptt.state == .thinking else { continue }
                let generation = runtime.generation
                do {
                    let text = try await recognizer.transcribe(utterance)
                    guard ptt.operationID == utterance.operationID, ptt.state == .thinking else { continue }
                    await runtime.submitTranscript(text, generation: generation)
                    if runtime.state == .error { message = runtime.errorMessage }
                    finishSpeechInput(operation: utterance.operationID)
                } catch { message = error.localizedDescription; finishSpeechInput(operation: utterance.operationID) }
            }
        }
    }
    func openRuntimeLogs() { NSWorkspace.shared.open(runtime.directory.appendingPathComponent("logs")) }
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
    @Published var selectedTab: SettingsTab = .general {
        didSet {
            SettingsArrowCapture.record(.selectionSet) { .init(selected: selectedTab, previous: oldValue) }
        }
    }
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
    private let cues = SoundCues()
    lazy var ptt = PushToTalk(keys: keySource, audio: EngineAudioCapture())
    var onTalkChange: (() -> Void)?
    var routePermissions: (() -> Void)?
    /// Runtime/Face installs the actual synchronous playback stop before speaking.
    var stopFacePlayback: (() -> Void)?
    @discardableResult
    func beginFacePlayback(stop: @escaping () -> Void) -> UInt64 {
        ptt.setState(.speaking)
        stopFacePlayback = stop
        return ptt.operationID
    }
    func finishFacePlayback(operation: UInt64) {
        if ptt.complete(operation: operation, state: .speaking) { stopFacePlayback = nil }
    }
    func finishSpeechInput(operation: UInt64) { ptt.complete(operation: operation, state: .thinking) }
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
        ptt.stopPlayback = { [weak self] in
            self?.runtime.cancelOutput(); self?.stopFacePlayback?(); self?.stopFacePlayback = nil
        }
        ptt.playStartSound = { [weak self] in if self?.startSound == true { self?.cues.play(.start) } }
        ptt.onChange = { [weak self] in
            guard let self else { return }
            if startSound && ptt.state != talkState {
                if ptt.state == .thinking && talkState == .listening { cues.play(.submit) }
                if ptt.state == .error { cues.play(.error) }
            }
            talkState = ptt.state; level = ptt.inputLevel; elapsed = ptt.elapsed
            onTalkChange?()
        }
        ptt.permissions = permissions
        ptt.start()
        configureRuntime()
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
    @Published var voicePanelSelected = false
    @Published var orbEnabled: Bool = false {
        didSet { if !testing { orbPreferences.isEnabled = orbEnabled } }
    }
    private let orbPreferences = OrbPreferences()
    private var updateSubscription: AnyCancellable?

    init() {
        let testing = ProcessInfo.processInfo.arguments.contains("--ui-testing")
        machine = AppStateMachine(onboardingComplete: !testing && UserDefaults.standard.bool(forKey: "onboardingComplete"))
        refreshPermissions()
        orbEnabled = !testing && orbPreferences.isEnabled
        updateSubscription = updates.objectWillChange.sink { [weak self] _ in self?.objectWillChange.send() }
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
        configureTalk()
    }

    func openPrivacy(_ pane: String) {
        guard !testing, let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(pane)") else { return }
        NSWorkspace.shared.open(url)
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate, NSWindowDelegate {
    let model = ShellModel()
    private var settingsWindow: NSWindow?
    private var onboardingWindow: NSWindow?
    private var aboutWindow: NSWindow?
    private lazy var face = FacePanelController(shell: model)
    private let talkHUD = TalkHUD()
    private lazy var orbOverlay = OrbOverlay(model: model)
    private var orbSubscription: AnyCancellable?
    private let commands = CommandPaletteController()
    private var commandMonitor: Any?

    func applicationDidFinishLaunching(_ notification: Notification) {
        let arguments = ProcessInfo.processInfo.arguments
        if model.testing, let index = arguments.firstIndex(of: "--design-snapshots"), arguments.count > index + 1 {
            do { try DesignSnapshots.run(to: arguments[index + 1]); exit(0) }
            catch { print("Native snapshot render failed: \(error)"); exit(1) }
        }
        if model.testing && arguments.contains("--contrast-calibration") {
            let probe = NSWindow(contentRect: NSRect(x: 100, y: 100, width: 760, height: 540), styleMask: [.titled, .closable], backing: .buffered, defer: false)
            probe.title = "Contrast calibration"
            probe.isReleasedWhenClosed = false
            probe.contentView = NSHostingView(rootView: ContrastCalibrationView())
            settingsWindow = probe
            NSApp.activate(ignoringOtherApps: true)
            probe.makeKeyAndOrderFront(nil)
            return
        }
        NSApp.setActivationPolicy(.accessory)
        commandMonitor = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            guard let self else { return event }
            SettingsArrowCapture.record(.arrival) {
                .init(selected: self.model.selectedTab, keyCode: event.keyCode, eventTime: event.timestamp)
            }
            if event.keyCode == 53 { SettingsArrowCapture.exportBeforeEscape() }
            let modifiers = event.modifierFlags.intersection([.command, .control, .option, .shift])
            if modifiers == .command && event.charactersIgnoringModifiers == "k" {
                self.showCommands(); return nil
            }
            // Type-ahead belongs only to the key Settings window and must never
            // steal input from an editor or observe another application's keys.
            guard self.settingsWindow?.isKeyWindow == true,
                  !(self.settingsWindow?.firstResponder is NSTextView),
                  modifiers.isEmpty || modifiers == .shift,
                  let characters = event.characters, !characters.isEmpty,
                  characters == "/" || characters.allSatisfy({ $0.isLetter || $0.isNumber }) else { return event }
            NotificationCenter.default.post(name: .softmaxFilterInput, object: characters == "/" ? "" : characters)
            return nil
        }
        orbSubscription = model.$orbEnabled.removeDuplicates().sink { [weak self] enabled in
            self?.orbOverlay.setVisible(enabled)
        }
        model.onTalkChange = { [weak self] in
            guard let self else { return }
            talkHUD.update(model: model)
            face.host.updateFromShell()
        }
        model.configureHub()
        model.routePermissions = { [weak self] in self?.showSettings(tab: .privacy) }
        if model.state == .onboarding { showOnboarding() }
        else { model.configureTalk() }
        if ProcessInfo.processInfo.arguments.contains("--show-face") { showFace() }
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        model.refreshPermissions()
        if !model.testing && model.machine.state != .onboarding && model.permissions.canUseGlobalPushToTalk {
            model.ptt.start()
        }
    }
    func applicationWillTerminate(_ notification: Notification) {
        if let commandMonitor { NSEvent.removeMonitor(commandMonitor) }
        model.hubConnection?.stop()
        face.hide(); model.ptt.shutdown()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func showSettings(tab: SettingsTab? = nil) {
        // Preserve the unfinished chapter, but never leave it occluded behind Settings.
        onboardingWindow?.orderOut(nil)
        if let tab { model.selectedTab = tab }
        if settingsWindow == nil {
            settingsWindow = window(title: "Softmax Settings", size: NSSize(width: 760, height: 540),
                                    view: SettingsView(model: model))
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
    }

    func windowWillClose(_ notification: Notification) {
        if let closed = notification.object as? NSWindow, closed === settingsWindow,
           model.state == .onboarding {
            onboardingWindow?.makeKeyAndOrderFront(nil)
        }
    }

    func showOnboarding() {
        onboardingWindow = window(title: "Welcome to Softmax", size: NSSize(width: 620, height: 600),
            view: OnboardingView(model: model, openSettings: { [weak self] in self?.showSettings() },
                finish: { [weak self] in
                    self?.model.finishOnboarding()
                    self?.onboardingWindow?.close()
                }))
        NSApp.activate(ignoringOtherApps: true)
        onboardingWindow?.makeKeyAndOrderFront(nil)
    }

    func showFace() {
        face.show()
    }

    func showConversation(voice: Bool = false) { orbOverlay.showChat(voice: voice) }

    func showCommands() {
        commands.show(commands: SoftmaxCommand.catalog(
            settings: { [weak self] in self?.showSettings(tab: $0) },
            face: { [weak self] in self?.showFace() },
            chat: { [weak self] in self?.showConversation(voice: $0) },
            about: { [weak self] in self?.showAbout() }))
    }

    func showAbout() {
        if aboutWindow == nil { aboutWindow = window(title: "About Softmax", size: NSSize(width: 420, height: 420), view: AboutView()) }
        NSApp.activate(ignoringOtherApps: true)
        aboutWindow?.makeKeyAndOrderFront(nil)
    }

    func quit() {
        if [.listening, .thinking, .speaking].contains(model.state) {
            let alert = NSAlert()
            alert.messageText = "Quit and stop this conversation?"
            alert.informativeText = "Unsent audio will be discarded and speech will stop."
            alert.addButton(withTitle: "Quit Softmax"); alert.addButton(withTitle: "Cancel")
            guard alert.runModal() == .alertFirstButtonReturn else { return }
        }
        model.ptt.cancel()
        Task { await model.runtime.stop(); NSApp.terminate(nil) }
    }

    private func window<V: View>(title: String, size: NSSize, view: V) -> NSWindow {
        let window = NSWindow(contentRect: NSRect(origin: .zero, size: size),
                              styleMask: [.titled, .closable, .miniaturizable, .resizable], backing: .buffered, defer: false)
        window.title = title
        window.isReleasedWhenClosed = false
        window.contentMinSize = size
        window.delegate = self
        let host = SoftmaxHostingView(rootView: SoftmaxWindowRoot { view }
            .accessibilityElement(children: .contain).accessibilityLabel(title))
        host.setAccessibilityLabel(title)
        window.contentView = host
        window.center()
        return window
    }
}
