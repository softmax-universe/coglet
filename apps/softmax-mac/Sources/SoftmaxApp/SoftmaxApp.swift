import SwiftUI
import AppKit
import AVFoundation
import CoreGraphics
import ServiceManagement
import SoftmaxCore
import FaceMac
import Combine
import SoftmaxRuntime
import SoftmaxAppSurface

@main
struct SoftmaxApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate

    var body: some Scene {
        MenuBarExtra {
            SoftmaxWindowRoot {
                SoftmaxMenuView(model: delegate.model, showFace: { delegate.showFace() },
                    showChat: { delegate.showConversation(voice: $0) }, settings: { delegate.showSettings() },
                    about: { delegate.showAbout() }, quit: { delegate.quit() }, searchCommands: { delegate.showCommands() })
            }
            .onAppear { delegate.model.menuBarOpen = true }
            .onDisappear { delegate.model.menuBarOpen = false }
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
    /// Actual window visibility and existing Settings selection, not shadow state.
    lazy var appControls = ControlRegistry(revision: 0, interactionAllowed: { [weak self] window in
        self?.appInteractionAllowed?(window) ?? false
    }, focusObservationEligible: { [weak self] window in
        self?.appFocusObservationEligible?(window) ?? true
    }) { [weak self] window in
        self?.appSurfaceRoute?(window)
    }
    // Installed by the actual window owner; absent ownership is unavailable.
    var appSurfaceRoute: ((String) -> String?)?
    var appInteractionAllowed: ((String) -> Bool)?
    // Unknown focus ownership remains eligible: only positive supersession excludes it.
    var appFocusObservationEligible: ((String) -> Bool)?
    var appSurface: AppSurfaceAdapter?
    #if DEBUG
    var wireProofShortcutPresses = 0
    #endif
    var synchronizeAppSurface: (() -> Void)?
    var hubPairingRoute: String? {
        didSet { if hubPairingRoute != oldValue { synchronizeAppSurface?() } }
    }
    var signOutConfirmationOpen = false {
        didSet { if signOutConfirmationOpen != oldValue { synchronizeAppSurface?() } }
    }
    @Published var keyboardShortcutsOpen = false {
        didSet { if keyboardShortcutsOpen != oldValue { synchronizeAppSurface?() } }
    }
    weak var keyboardShortcutsNativeWindow: NSWindow?
    weak var hubPairingNativeWindow: NSWindow?
    weak var signOutNativeWindow: NSWindow?
    @Published var onboardingStep = 0 {
        didSet {
            guard onboardingStep != oldValue else { return }
            do { try appControls.nativeStateChanged() }
            catch { appControlFailure(.exhausted) }
            synchronizeAppSurface?()
        }
    }
    var menuBarOpen = false {
        didSet { if menuBarOpen != oldValue { synchronizeAppSurface?() } }
    }
    @Published private(set) var appControlRegistrationFailed = false
    func appControlFailure(_ failure: ControlRegistryFailure) {
        // Never log native values, errors, tokens or text-field input.
        appControlRegistrationFailed = true
    }
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
    #if DEBUG
    /// Isolated wire-proof seam only: the sole non-production writer of
    /// `hubConnection`. It never replaces an existing connection and requires
    /// the UI-testing launch with a stopped runtime (no production override).
    func installWireProofHubFixture(_ connection: HubConnection) throws {
        guard testing, runtime.state == .stopped, hubConnection == nil else { throw HubError.unavailable }
        connection.changed = { [weak self] in self?.objectWillChange.send() }
        hubConnection = connection
    }
    #endif
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
            guard selectedTab != oldValue else { return }
            do { try appControls.nativeStateChanged() }
            catch { appControlFailure(.exhausted) }
            synchronizeAppSurface?()
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
    @Published var voicePanelSelected = false {
        didSet { if voicePanelSelected != oldValue { synchronizeAppSurface?() } }
    }
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
        try? await requestMicrophone(context: nil)
    }
    /// Same completion-bearing command for user and adapter. Entering an OS
    /// prompt is irreversible; revocation fences later effects, not that prompt.
    func requestMicrophone(context: ControlRegistry.EffectContext?) async throws {
        guard !testing else { throw ControlRegistryFailure.unavailable }
        if AVCaptureDevice.authorizationStatus(for: .audio) == .notDetermined {
            let granted: Bool = try await withCheckedThrowingContinuation { continuation in
                let request = {
                    AVCaptureDevice.requestAccess(for: .audio) { allowed in continuation.resume(returning: allowed) }
                }
                do {
                    if let context { try context.perform(request) } else { request() }
                } catch { continuation.resume(throwing: error) }
            }
            if let context { try context.perform { refreshPermissions() } } else { refreshPermissions() }
            guard granted else { throw ControlRegistryFailure.permissionDenied }
        } else {
            if let context { try context.perform { refreshPermissions() } } else { refreshPermissions() }
            guard AVCaptureDevice.authorizationStatus(for: .audio) == .authorized else { throw ControlRegistryFailure.permissionDenied }
        }
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
    private var nativeWindowControls: [String: NativeWindowControlRegistration] = [:]
    private lazy var face = FacePanelController(shell: model)
    private let talkHUD = TalkHUD()
    private lazy var orbOverlay = OrbOverlay(model: model)
    private var orbSubscription: AnyCancellable?
    private let commands = CommandPaletteController()
    private var commandMonitor: Any?
    #if DEBUG
    private var catalogProbeTask: Task<Void, Never>?
    private var catalogSignOutFixture: CatalogSignOutFixture?
    private var wireProof: AppSurfaceWireProof?
    #endif

    func applicationDidFinishLaunching(_ notification: Notification) {
        installAppSurface()
        #if DEBUG
        startCatalogProbe()
        #endif
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
            self?.invalidateAppControlSnapshots()
        }
        model.onTalkChange = { [weak self] in
            guard let self else { return }
            talkHUD.update(model: model)
            face.host.updateFromShell()
            invalidateAppControlSnapshots()
        }
        model.configureHub()
        model.routePermissions = { [weak self] in self?.showSettings(tab: .privacy) }
        if model.state == .onboarding { showOnboarding() }
        else { model.configureTalk() }
        if ProcessInfo.processInfo.arguments.contains("--show-face") { showFace() }
        #if DEBUG
        startWireProof()
        #endif
    }

    func applicationDidBecomeActive(_ notification: Notification) {
        model.refreshPermissions()
        if !model.testing && model.machine.state != .onboarding && model.permissions.canUseGlobalPushToTalk {
            model.ptt.start()
        }
    }
    func applicationWillTerminate(_ notification: Notification) {
        #if DEBUG
        wireProof?.stop()
        #endif
        if let commandMonitor { NSEvent.removeMonitor(commandMonitor) }
        model.hubConnection?.stop()
        face.hide(); model.ptt.shutdown()
    }
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    func showSettings(tab: SettingsTab? = nil) {
        // Preserve the unfinished chapter, but never leave it occluded behind Settings.
        do { try nativeWindowControls["onboarding"]?.invalidate() }
        catch { model.appControlFailure(.exhausted) }
        onboardingWindow?.orderOut(nil)
        if let tab { model.selectedTab = tab }
        if settingsWindow == nil {
            settingsWindow = window(title: "Softmax Settings", size: NSSize(width: 760, height: 540),
                                    view: SettingsView(model: model))
            settingsWindow?.identifier = NSUserInterfaceItemIdentifier("softmax.settings")
        }
        NSApp.activate(ignoringOtherApps: true)
        settingsWindow?.makeKeyAndOrderFront(nil)
        invalidateAppControlSnapshots()
    }

    func windowWillClose(_ notification: Notification) {
        let closing = notification.object as? NSWindow
        if let closed = notification.object as? NSWindow, closed === settingsWindow,
           model.state == .onboarding {
            onboardingWindow?.makeKeyAndOrderFront(nil)
        }
        Task { @MainActor [weak self, weak closing] in
            guard let self, let closing, !closing.isVisible else { return }
            self.invalidateAppControlSnapshots()
        }
    }

    func windowDidMiniaturize(_ notification: Notification) { invalidateAppControlSnapshots() }
    func windowDidDeminiaturize(_ notification: Notification) { invalidateAppControlSnapshots() }
    private func synchronizeNativeWindowControls() throws {
        orbOverlay.synchronizeControls()
        for id in ["settings", "onboarding", "about", "command_palette"] {
            let owner = nativeWindowControls[id] ?? NativeWindowControlRegistration(registry: model.appControls)
            nativeWindowControls[id] = owner
            try owner.synchronize(window: appWindow(id), id: id, route: appRoute(id))
        }
    }

    private func invalidateAppControlSnapshots() {
        do {
            try synchronizeNativeWindowControls()
            try model.appControls.nativeStateChanged()
            try model.appControls.synchronizeNativeState(windows: Self.appWindows)
        }
        catch { model.appControlFailure(.exhausted) }
    }

    func showOnboarding() {
        onboardingWindow = window(title: "Welcome to Softmax", size: NSSize(width: 620, height: 600),
            view: OnboardingView(model: model, openSettings: { [weak self] in self?.showSettings() },
                finish: { [weak self] in
                    self?.model.finishOnboarding()
                    self?.onboardingWindow?.close()
                }))
        onboardingWindow?.identifier = NSUserInterfaceItemIdentifier("softmax.onboarding")
        NSApp.activate(ignoringOtherApps: true)
        onboardingWindow?.makeKeyAndOrderFront(nil)
        invalidateAppControlSnapshots()
    }

    func showFace() {
        face.show()
        invalidateAppControlSnapshots()
    }

    func showConversation(voice: Bool = false) {
        orbOverlay.showChat(voice: voice)
        invalidateAppControlSnapshots()
    }

    func showCommands() {
        commands.show(commands: SoftmaxCommand.catalog(
            settings: { [weak self] in self?.showSettings(tab: $0) },
            face: { [weak self] in self?.showFace() },
            chat: { [weak self] in self?.showConversation(voice: $0) },
            about: { [weak self] in self?.showAbout() }), model: model)
        invalidateAppControlSnapshots()
    }

    func showAbout() {
        if aboutWindow == nil {
            aboutWindow = window(title: "About Softmax", size: NSSize(width: 420, height: 420), view: AboutView(model: model))
            aboutWindow?.identifier = NSUserInterfaceItemIdentifier("softmax.about")
        }
        NSApp.activate(ignoringOtherApps: true)
        aboutWindow?.makeKeyAndOrderFront(nil)
        invalidateAppControlSnapshots()
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

    // Product native dispatch; the runtime actor owns transport, SDK admission
    // and durable sequencing. These closures neither grant authority nor run a guest.
    private func installAppSurface() {
        orbOverlay.onChange = { [weak self] in self?.invalidateAppControlSnapshots() }
        commands.onChange = { [weak self] in self?.invalidateAppControlSnapshots() }
        talkHUD.onChange = { [weak self] in self?.invalidateAppControlSnapshots() }
        face.onChange = { [weak self] in self?.invalidateAppControlSnapshots() }
        model.appSurfaceRoute = { [weak self] in self?.appRoute($0) }
        model.appFocusObservationEligible = { [weak self] window in
            guard let self else { return true }
            let owner: NSWindow?
            switch window {
            case "hub_pairing": owner = self.model.hubPairingNativeWindow
            case "sign_out_confirmation": owner = self.model.signOutNativeWindow
            case "keyboard_shortcuts": owner = self.model.keyboardShortcutsNativeWindow
            default: owner = self.appWindow(window)
            }
            return NativeFocusObservation.isEligible(owner: owner)
        }
        model.appInteractionAllowed = { [weak self] window in
            guard let self, !self.hasUnqualifiedAppModal else { return false }
            if self.model.keyboardShortcutsOpen { return window == "keyboard_shortcuts" }
            if self.model.signOutConfirmationOpen { return window == "sign_out_confirmation" }
            return self.model.hubPairingRoute == nil || window == "hub_pairing"
        }
        model.synchronizeAppSurface = { [weak self] in
            guard let self else { return }
            do {
                try self.synchronizeNativeWindowControls()
                try self.model.appControls.synchronizeNativeState(windows: Self.appWindows)
            }
            catch { self.model.appControlFailure(.invalidRegistration) }
        }
        model.appSurface = AppSurfaceAdapter(registry: model.appControls,
            captureState: { [weak self] in
                guard let self else { throw AppSurfaceFailure.unavailable }
                return try self.appSnapshot()
            }, currentRoute: { [weak self] in self?.appRoute($0) },
            navigate: { [weak self] window, route in
                guard let self else { throw AppSurfaceFailure.unavailable }
                guard !self.hasUnqualifiedAppModal, self.model.hubPairingRoute == nil,
                      !self.model.signOutConfirmationOpen, !self.model.keyboardShortcutsOpen else { throw AppSurfaceFailure.unavailable }
                switch window {
                case "settings":
                    guard let tab = SettingsTab.allCases.first(where: { $0.rawValue.lowercased() == route }) else {
                        throw AppSurfaceFailure.unsupported
                    }
                    self.showSettings(tab: tab)
                case "onboarding":
                    guard let index = ["welcome", "permissions", "hub"].firstIndex(of: route) else {
                        throw AppSurfaceFailure.unsupported
                    }
                    self.model.onboardingStep = index
                case "chat": self.showConversation(voice: route == "voice")
                default:
                    guard self.appRoute(window) == route else { throw AppSurfaceFailure.unsupported }
                }
            }, open: { [weak self] window in
                guard let self else { throw AppSurfaceFailure.unavailable }
                guard !self.hasUnqualifiedAppModal, self.model.hubPairingRoute == nil,
                      !self.model.signOutConfirmationOpen, !self.model.keyboardShortcutsOpen else { throw AppSurfaceFailure.unavailable }
                switch window {
                case "settings": self.showSettings()
                case "about": self.showAbout()
                case "face": self.showFace()
                case "chat": self.showConversation()
                case "command_palette": self.showCommands()
                case "orb": self.model.orbEnabled = true
                // There is no main window, and native onboarding/confirmations
                // cannot be fabricated or skip their own entry preconditions.
                default: throw AppSurfaceFailure.unsupported
                }
            }, close: { [weak self] window in
                guard let self else { throw AppSurfaceFailure.unavailable }
                guard !self.hasUnqualifiedAppModal, self.model.hubPairingRoute == nil,
                      !self.model.signOutConfirmationOpen, !self.model.keyboardShortcutsOpen else { throw AppSurfaceFailure.unavailable }
                switch window {
                case "face": self.face.hide()
                case "orb": self.model.orbEnabled = false
                case "command_palette": self.commands.close()
                case "settings", "about", "chat", "onboarding":
                    guard let native = self.appWindow(window) else { throw AppSurfaceFailure.unavailable }
                    native.performClose(nil)
                // Permission/identity confirmation and quit are not ordinary
                // close aliases. No keyboard injection or other-app control.
                default: throw AppSurfaceFailure.unsupported
                }
            })
        #if DEBUG
        // This isolated seam is the only event/mailbox owner for this launch.
        // It never configures or opens the production runtime's directory.
        if model.testing && ProcessInfo.processInfo.arguments.contains("--sdk-wire-proof-directory") { return }
        #endif
        if let adapter = model.appSurface {
            model.runtime.installAppSurface(adapter, refresh: { [weak self] in
                guard let self else { throw AppSurfaceFailure.unavailable }
                // A fresh sampled native revision includes transient meters and
                // hub status too, not only SwiftUI control binding mutations.
                try self.synchronizeNativeWindowControls()
                try self.model.appControls.nativeStateChanged()
                try self.model.appControls.synchronizeNativeState(windows: Self.appWindows)
            }, events: { [weak self] handler in
                self?.model.appControls.onChange = handler
            })
        }
    }

    #if DEBUG
    private func startWireProof() {
        let args = ProcessInfo.processInfo.arguments
        guard model.testing, let i = args.firstIndex(of: "--sdk-wire-proof-directory"),
              args.indices.contains(i + 1) else { return }
        do {
            let proof = try AppSurfaceWireProof(model: model,
                directory: URL(fileURLWithPath: args[i + 1]).standardizedFileURL,
                credentialFixture: args.contains(WireProofCredentialFixture.flag),
                refresh: { [weak self] in
                    guard let self else { throw AppSurfaceFailure.unavailable }
                    try self.synchronizeNativeWindowControls()
                    try self.model.appControls.nativeStateChanged()
                    try self.model.appControls.synchronizeNativeState(windows: Self.appWindows)
                }, diagnosticContext: { [weak self] in
                    // Closed Boolean context only; never expose route strings,
                    // sheet/control text, window identity or registration values.
                    let sheets = NSApp.windows.compactMap { $0.attachedSheet }.filter { $0.isVisible }
                    return WireProofNativeContext(
                        hasUnqualifiedAppModal: self?.hasUnqualifiedAppModal ?? true,
                        modalWindowPresent: NSApp.modalWindow != nil,
                        visibleAttachedSheet: !sheets.isEmpty,
                        pairingOwnerMatches: sheets.contains { $0 === self?.model.hubPairingNativeWindow },
                        pairingRoutePresent: self?.model.hubPairingRoute != nil,
                        pairingOwnerVisible: self?.model.hubPairingNativeWindow?.isVisible == true,
                        registrationFailed: self?.model.appControlRegistrationFailed == true)
                })
            wireProof = proof; proof.start()
        } catch { model.appControlFailure(.unavailable) }
    }

    /// Test-only membership evidence, never an adapter or guest observation.
    /// Requires BOTH the isolated UI-test mode and an existing private temp dir.
    /// Each request is a fresh UUID; no control values/labels are serialized.
    private func startCatalogProbe() {
        let arguments = ProcessInfo.processInfo.arguments
        guard model.testing, let index = arguments.firstIndex(of: "--catalog-probe-directory"),
              arguments.indices.contains(index + 1) else { return }
        let directory = URL(fileURLWithPath: arguments[index + 1]).standardizedFileURL
        let temporary = FileManager.default.temporaryDirectory.resolvingSymlinksInPath().path + "/"
        guard directory.resolvingSymlinksInPath().path.hasPrefix(temporary),
              let attributes = try? FileManager.default.attributesOfItem(atPath: directory.path),
              attributes[.type] as? FileAttributeType == .typeDirectory,
              (attributes[.posixPermissions] as? NSNumber)?.intValue == 0o700,
              (attributes[.ownerAccountID] as? NSNumber)?.uint32Value == getuid() else { return }
        // A closed prerequisite selector, not a sheet opener or credential
        // fixture. Only this validated, isolated DEBUG catalog launch owns it.
        if arguments.contains(CatalogSignOutFixture.flag) {
            do { catalogSignOutFixture = try CatalogSignOutFixture.install(into: model) }
            catch { model.appControlFailure(.unavailable); return }
        }
        CogletMenuButton.enableDiagnostics(true)
        struct Probe: Encodable {
            let request: String
            let revision: UInt64
            let registrationFailed: Bool
            let registrations: [ControlRegistry.Registration]
            let menuDiagnostics: CogletMenuButton.DiagnosticSnapshot
            let signOutFixture: CatalogSignOutFixture.Witness?
        }
        catalogProbeTask = Task { @MainActor [weak self] in
            var lastRequest: String?
            while !Task.isCancelled {
                do { try await Task.sleep(for: .milliseconds(100)) }
                catch { return }
                guard let self else { return }
                let input = directory.appendingPathComponent("request")
                guard let metadata = try? FileManager.default.attributesOfItem(atPath: input.path),
                      metadata[.type] as? FileAttributeType == .typeRegular,
                      (metadata[.size] as? NSNumber)?.intValue == 36,
                      let bytes = try? Data(contentsOf: input), bytes.count == 36,
                      let request = String(data: bytes, encoding: .utf8), UUID(uuidString: request) != nil,
                      request != lastRequest else { continue }
                // Refresh the real native owner before copying actual membership.
                self.model.synchronizeAppSurface?()
                let probe = Probe(request: request, revision: self.model.appControls.revision,
                    registrationFailed: self.model.appControlRegistrationFailed,
                    registrations: self.model.appControls.registrationInventory(),
                    menuDiagnostics: CogletMenuButton.diagnosticSnapshot,
                    signOutFixture: self.catalogSignOutFixture?.witness)
                do {
                    let output = try JSONEncoder().encode(probe)
                    try output.write(to: directory.appendingPathComponent("response.json"), options: .atomic)
                    lastRequest = request
                } catch { return } // UI test times out, never consumes a stale nonce.
            }
        }
    }
    #endif

    private static let appWindows = ["main", "menu_bar", "settings", "onboarding", "face", "chat", "orb", "hud",
        "command_palette", "about", "keyboard_shortcuts", "hub_pairing", "sign_out_confirmation", "quit_confirmation"]
    private func appWindow(_ id: String) -> NSWindow? {
        NSApp.windows.first { $0.identifier?.rawValue == "softmax." + id }
    }
    private var hasUnqualifiedAppModal: Bool {
        // Until every native confirmation/sheet has its own qualified owner,
        // refuse an incomplete snapshot rather than claim those panels closed.
        if NSApp.modalWindow != nil { return true }
        return NSApp.windows.contains {
            guard let sheet = $0.attachedSheet, sheet.isVisible else { return false }
            if model.keyboardShortcutsOpen, sheet === model.keyboardShortcutsNativeWindow { return false }
            if model.hubPairingRoute != nil, sheet === model.hubPairingNativeWindow { return false }
            if model.signOutConfirmationOpen, sheet === model.signOutNativeWindow { return false }
            return true
        }
    }
    private func appRoute(_ window: String) -> String? {
        if window == "menu_bar" { return model.menuBarOpen ? "menu" : nil }
        if window == "keyboard_shortcuts" {
            return model.keyboardShortcutsOpen && model.keyboardShortcutsNativeWindow?.isVisible == true ? "shortcuts" : nil
        }
        if window == "hub_pairing" {
            return model.hubPairingNativeWindow?.isVisible == true ? model.hubPairingRoute : nil
        }
        if window == "sign_out_confirmation" {
            return model.signOutConfirmationOpen && model.signOutNativeWindow?.isVisible == true ? "sign_out" : nil
        }
        guard let native = appWindow(window), native.isVisible, !native.isMiniaturized else { return nil }
        switch window {
        case "settings": return model.selectedTab.rawValue.lowercased()
        case "onboarding":
            guard (0...2).contains(model.onboardingStep) else { return nil }
            return ["welcome", "permissions", "hub"][model.onboardingStep]
        case "chat": return model.voicePanelSelected ? "voice" : "chat"
        case "command_palette": return "commands"
        case "keyboard_shortcuts": return "shortcuts"
        case "face", "orb", "hud", "about", "main": return window
        // A dialog must supply its actual route before it is observable.
        default: return nil
        }
    }
    private func appSnapshot() throws -> SoftmaxAppSurface.CBORValue {
        typealias Value = SoftmaxAppSurface.CBORValue
        func object(_ fields: [(String, Value)]) -> Value {
            .map(fields.map { SoftmaxAppSurface.CBORMapEntry($0.0, $0.1) })
        }
        func meter(_ elapsed: Double, _ level: Float) throws -> (Value, Value) {
            guard elapsed.isFinite, (0...120).contains(elapsed), level.isFinite, (0...1).contains(level) else {
                throw AppSurfaceFailure.unavailable
            }
            return (.unsigned(UInt64((elapsed * 1000).rounded(.down))), .unsigned(UInt64((Double(level) * 1000).rounded(.down))))
        }
        guard !model.appControlRegistrationFailed, !hasUnqualifiedAppModal else { throw AppSurfaceFailure.unavailable }
        let defaults = UserDefaults.standard
        let appearance = defaults.string(forKey: "appearance") ?? "System"
        let scale = defaults.object(forKey: "textScale") == nil ? 1.0 : defaults.double(forKey: "textScale")
        let textSize: String
        switch scale { case 1.0: textSize = "standard"; case 1.3: textSize = "large"; case 1.6: textSize = "largest"
        default: throw AppSurfaceFailure.unavailable }
        let modifierBits: [(UInt64, String)] = [(UInt64(NSEvent.ModifierFlags.command.rawValue), "command"),
            (UInt64(NSEvent.ModifierFlags.control.rawValue), "control"), (UInt64(NSEvent.ModifierFlags.option.rawValue), "option"),
            (UInt64(NSEvent.ModifierFlags.shift.rawValue), "shift")]
        let mask = modifierBits.reduce(UInt64(0)) { $0 | $1.0 }
        guard (0...127).contains(model.keySource.keyCode), model.keySource.modifiers & ~mask == 0 else {
            throw AppSurfaceFailure.unavailable
        }
        let modifiers = modifierBits.filter { model.keySource.modifiers & $0.0 != 0 }.map { Value.text($0.1) }
        let settings = object([("appearance", .text(appearance)), ("text_size", .text(textSize)),
            ("floating_orb", .boolean(model.orbEnabled)), ("talk_mode", .text(model.toggleMode ? "toggle" : "hold")),
            ("sound_cues", .boolean(model.startSound)), ("launch_at_login", .text(model.loginItem.rawValue)),
            ("hotkey", object([("key_code", .unsigned(UInt64(model.keySource.keyCode))), ("modifiers", .array(modifiers))]))])
        let pttMeter = try meter(model.elapsed, model.level)
        let hudMeter = try meter(talkHUD.surfaceElapsed, talkHUD.surfaceLevel)
        let ptt = object([("state", .text(model.talkState.rawValue)), ("visible", .boolean(talkHUD.surfaceVisible)),
            ("elapsed_ms", pttMeter.0), ("level", pttMeter.1)])
        let hud = object([("state", .text(talkHUD.surfaceState)), ("visible", .boolean(talkHUD.surfaceVisible)),
            ("elapsed_ms", hudMeter.0), ("level", hudMeter.1)])
        let faceState = object([("visible", .boolean(appRoute("face") != nil)),
            ("mode", .text(model.faceHost.compact ? "compact" : "full")),
            ("expression", .text(model.faceHost.driver.expression.rawValue)),
            ("status", .text(model.faceHost.driver.state.label.lowercased()))])
        let hub: String
        switch model.hubConnection?.state ?? .offline { case .connected: hub = "connected"
        case .connecting: hub = "connecting"; case .offline: hub = "offline" }
        let windows = Self.appWindows.map { id -> Value in
            let route = appRoute(id)
            return object([("window", .text(id)), ("open", .boolean(route != nil)), ("route", route.map(Value.text) ?? .null)])
        }
        let snapshot = object([("revision", .unsigned(model.appControls.revision)), ("windows", .array(windows)),
            ("focus", try model.appControls.focusedControl()), ("settings", settings), ("ptt", ptt), ("hud", hud),
            ("hub", .text(hub)), ("face", faceState)])
        try SoftmaxAppSurface.CBORCodec.validate("app-state-result", snapshot)
        return snapshot
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
