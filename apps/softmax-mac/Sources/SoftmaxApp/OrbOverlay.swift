import AppKit
import SwiftUI
import SoftmaxCore
import SoftmaxAppSurface

/// Owns both panels. Hiding the optional orb also hides its attached conversation.
@MainActor
final class OrbOverlay {
    private let model: ShellModel
    private var orb: NSPanel?
    private var chat: NSPanel?
    private var screenObserver: NSObjectProtocol?
    var onChange: (() -> Void)?
    private var orbControl: NativeButtonRegistration?
    private lazy var chatControls = NativeWindowControlRegistration(registry: model.appControls)

    private func retireControls() {
        do {
            try orbControl?.invalidate()
            orbControl = nil
            try chatControls.invalidate()
        } catch { model.appControlFailure(.exhausted) }
    }

    init(model: ShellModel) {
        self.model = model
        screenObserver = NotificationCenter.default.addObserver(
            forName: NSApplication.didChangeScreenParametersNotification, object: nil, queue: .main
        ) { [weak self] _ in
            Task { @MainActor in self?.snap() }
        }
    }

    func setVisible(_ visible: Bool) {
        guard visible else {
            // Retire the presentation before native hide; queued requests must
            // not acquire the same NSPanel's next presentation.
            retireControls()
            orb?.orderOut(nil)
            chat?.orderOut(nil)
            onChange?()
            return
        }
        if orb == nil {
            let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 68, height: 68),
                                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.title = "Softmax Orb"
            panel.identifier = NSUserInterfaceItemIdentifier("softmax.orb")
            panel.level = .floating
            panel.hidesOnDeactivate = false
            panel.isReleasedWhenClosed = false
            panel.isOpaque = false
            panel.backgroundColor = .clear
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            let button = OrbButton(frame: CGRect(x: 0, y: 0, width: 68, height: 68))
            button.title = ""
            button.bezelStyle = .circular
            button.image = NSImage(systemSymbolName: "waveform.circle.fill", accessibilityDescription: "Open Softmax chat")
            button.imageScaling = .scaleProportionallyUpOrDown
            button.setAccessibilityLabel("Open Softmax chat")
            button.setAccessibilityHelp("Click for chat. Right-click for voice. Drag to a screen edge.")
            button.onClick = { [weak self] in self?.toggleChat() }
            button.onVoice = { [weak self] in self?.showChat(voice: true) }
            button.onDrop = { [weak self] in self?.snap() }
            panel.contentView = button
            if let screen = NSScreen.main {
                panel.setFrameOrigin(CGPoint(x: screen.visibleFrame.maxX - 168, y: screen.visibleFrame.minY + 100))
            }
            orb = panel
        }
        snap()
        orb?.orderFrontRegardless()
        if orbControl?.isActive != true, let button = orb?.contentView as? NSButton {
            do {
                orbControl = try NativeButtonRegistration(button: button, registry: model.appControls,
                    id: "orb.open_chat", window: "orb", route: "orb")
            } catch { model.appControlFailure(.invalidRegistration) }
        }
        onChange?()
    }

    private func snap() {
        guard let orb, let screen = screen(for: orb) else { return }
        orb.setFrameOrigin(OrbGeometry.snapped(orb.frame.origin, in: screen.visibleFrame))
        positionChat()
    }

    private func screen(for panel: NSPanel) -> NSScreen? {
        let center = CGPoint(x: panel.frame.midX, y: panel.frame.midY)
        return NSScreen.screens.first { $0.frame.contains(center) } ?? NSScreen.main
    }

    private func positionChat() {
        guard let orb, let chat, let screen = screen(for: orb) else { return }
        chat.setFrame(OrbGeometry.chatFrame(orb: orb.frame, in: screen.visibleFrame), display: true)
    }

    private func toggleChat() {
        if chat?.isVisible == true {
            do { try chatControls.invalidate() }
            catch { model.appControlFailure(.exhausted) }
            chat?.orderOut(nil)
            onChange?()
        } else { showChat(voice: false) }
    }

    func showChat(voice: Bool) {
        // Same native panel may change route without closing. Retire its old
        // titlebar incarnation before changing that route.
        if model.voicePanelSelected != voice {
            do { try chatControls.invalidate() }
            catch { model.appControlFailure(.exhausted) }
        }
        model.voicePanelSelected = voice
        if chat == nil {
            let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 408, height: 548),
                                styleMask: [.titled, .closable], backing: .buffered, defer: false)
            panel.title = "Softmax Conversation"
            panel.identifier = NSUserInterfaceItemIdentifier("softmax.chat")
            panel.level = .floating
            panel.hidesOnDeactivate = false
            panel.isReleasedWhenClosed = false
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            panel.contentView = NSHostingView(rootView: ConversationView(model: model))
            panel.center()
            chat = panel
        }
        positionChat()
        NSApp.activate(ignoringOtherApps: true)
        chat?.makeKeyAndOrderFront(nil)
        synchronizeControls()
        if voice { model.refreshPermissions() }
        onChange?()
    }

    func synchronizeControls() {
        do {
            try chatControls.synchronize(window: chat, id: "chat",
                route: model.voicePanelSelected ? "voice" : "chat")
        } catch { model.appControlFailure(.invalidRegistration) }
    }
}

struct ConversationView: View {
    @ObservedObject var model: ShellModel
    @State private var draft = ""
    @FocusState private var draftFocused: Bool
    private var route: String { model.voicePanelSelected ? "voice" : "chat" }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Picker("Conversation mode", selection: $model.voicePanelSelected) {
                    Text("Chat").tag(false)
                    Text("Voice").tag(true)
                }.pickerStyle(.segmented)
                    .coglet(id: "chat.mode", registry: model.appControls, window: "chat", route: route,
                        role: "picker", label: "Conversation mode", verbs: ["select"], value: {
                            .map([.init("kind", .text("selection")), .init("value", .text(route))])
                        }, failure: model.appControlFailure, invoke: { args in
                            guard case .map(let fields) = args,
                                  let field = fields.first(where: { $0.key == "value" }), case .text(let choice) = field.value,
                                  ["chat", "voice"].contains(choice) else { throw ControlRegistryFailure.unsupported }
                            model.voicePanelSelected = choice == "voice"
                        })
                Menu {
                    CogletButton("Clear draft", id: "chat.clear_draft", registry: model.appControls, window: "chat",
                        route: route, failure: model.appControlFailure) { draft = "" }
                    CogletButton("Hide orb", id: "chat.hide_orb", registry: model.appControls, window: "chat",
                        route: route, failure: model.appControlFailure) { model.orbEnabled = false }
                } label: { Image(systemName: "ellipsis.circle") }.fixedSize()
            }
            if model.voicePanelSelected {
                Text("Voice").font(.title2)
                MicrophoneControls(model: model, window: "chat", route: "voice", idPrefix: "chat.voice")
                Text("Use the push-to-talk hotkey to capture audio. Runtime speech delivery is not connected yet.")
                Text("Push-to-talk: \(model.state.rawValue)")
            } else {
                Text("Chat").font(.title2)
                Text("Your local runtime is not connected yet. Draft a message here; sending will be available once connected.")
                TextEditor(text: $draft).accessibilityLabel("Message draft").focused($draftFocused)
                    .coglet(id: "chat.draft", registry: model.appControls, window: "chat", route: "chat", role: "text_editor",
                        label: "Message draft", verbs: ["type_text"], value: {
                            .map([.init("kind", .text("text")), .init("value", .text(draft))])
                        }, focus: { draftFocused = true; return true }, focused: { draftFocused },
                        failure: model.appControlFailure, invoke: { args in
                            guard case .map(let fields) = args,
                                  let field = fields.first(where: { $0.key == "value" }), case .text(let text) = field.value else {
                                throw ControlRegistryFailure.unsupported
                            }
                            draft = text
                        })
                CogletButton("Send", id: "chat.send", registry: model.appControls, window: "chat", route: "chat",
                    failure: model.appControlFailure) {}.disabled(true)
            }
            Spacer(minLength: 0)
            Text("Hub offline · Runtime not connected").font(.caption).foregroundStyle(.secondary)
        }.padding(20).frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct MicrophoneControls: View {
    @ObservedObject var model: ShellModel
    let window: String
    let route: String
    let idPrefix: String
    @ViewBuilder private var permissionButtons: some View {
        CogletButton("Microphone Settings", id: "\(idPrefix).microphone_settings", registry: model.appControls,
            window: window, route: route, sensitive: true, failure: model.appControlFailure) { model.openPrivacy("Privacy_Microphone") }
        CogletButton("Refresh Permissions", id: "\(idPrefix).refresh_permissions", registry: model.appControls,
            window: window, route: route, failure: model.appControlFailure) { model.refreshPermissions() }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Microphone: \(model.permissions.microphone.rawValue)")
            if let guidance = model.permissions.microphone.microphoneGuidance {
                Text(guidance).modifier(SoftmaxErrorText()).fixedSize(horizontal: false, vertical: true)
            }
            if model.permissions.microphone == .notDetermined {
                CogletAsyncButton("Allow Microphone", id: "\(idPrefix).allow_microphone", registry: model.appControls,
                    window: window, route: route, sensitive: true,
                    failure: model.appControlFailure) { context in try await model.requestMicrophone(context: context) }
            }
            ViewThatFits(in: .horizontal) {
                HStack { permissionButtons }
                VStack(alignment: .leading) { permissionButtons }
            }
        }
    }
}
