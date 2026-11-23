import AppKit
import SwiftUI
import SoftmaxCore

/// Owns both panels. Hiding the optional orb also hides its attached conversation.
@MainActor
final class OrbOverlay {
    private let model: ShellModel
    private var orb: NSPanel?
    private var chat: NSPanel?
    private var screenObserver: NSObjectProtocol?

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
            orb?.orderOut(nil)
            chat?.orderOut(nil)
            return
        }
        if orb == nil {
            let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 68, height: 68),
                                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.title = "Softmax Orb"
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
        if chat?.isVisible == true { chat?.orderOut(nil) }
        else { showChat(voice: false) }
    }

    func showChat(voice: Bool) {
        model.voicePanelSelected = voice
        if chat == nil {
            let panel = NSPanel(contentRect: CGRect(x: 0, y: 0, width: 408, height: 548),
                                styleMask: [.titled, .closable], backing: .buffered, defer: false)
            panel.title = "Softmax Conversation"
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
        if voice { model.refreshPermissions() }
    }
}

@MainActor
private final class OrbButton: NSButton {
    var onClick: (() -> Void)?
    var onVoice: (() -> Void)?
    var onDrop: (() -> Void)?

    private var dragStart: CGPoint?
    private var windowStart: CGPoint?
    private var moved = false

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        guard let window else { return }
        dragStart = window.convertPoint(toScreen: event.locationInWindow)
        windowStart = window.frame.origin
        moved = false
    }

    override func mouseDragged(with event: NSEvent) {
        guard let window, let dragStart, let windowStart else { return }
        let pointer = window.convertPoint(toScreen: event.locationInWindow)
        let delta = CGPoint(x: pointer.x - dragStart.x, y: pointer.y - dragStart.y)
        if hypot(delta.x, delta.y) > 4 { moved = true }
        if moved { window.setFrameOrigin(CGPoint(x: windowStart.x + delta.x, y: windowStart.y + delta.y)) }
    }

    override func mouseUp(with event: NSEvent) {
        guard dragStart != nil else { return }
        dragStart = nil
        windowStart = nil
        if moved { onDrop?() } else { onClick?() }
    }

    override func rightMouseDown(with event: NSEvent) { onVoice?() }
    override func accessibilityPerformPress() -> Bool { onClick?(); return true }
}

struct ConversationView: View {
    @ObservedObject var model: ShellModel
    @State private var draft = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack {
                Picker("Conversation mode", selection: $model.voicePanelSelected) {
                    Text("Chat").tag(false)
                    Text("Voice").tag(true)
                }.pickerStyle(.segmented)
                Menu {
                    Button("Clear draft") { draft = "" }
                    Button("Hide orb") { model.orbEnabled = false }
                } label: { Image(systemName: "ellipsis.circle") }.fixedSize()
            }
            if model.voicePanelSelected {
                Text("Voice").font(.title2)
                MicrophoneControls(model: model)
                Text("Use the push-to-talk hotkey to capture audio. Runtime speech delivery is not connected yet.")
                Text("Push-to-talk: \(model.state.rawValue)")
            } else {
                Text("Chat").font(.title2)
                Text("Your local runtime is not connected yet. Draft a message here; sending will be available once connected.")
                TextEditor(text: $draft).accessibilityLabel("Message draft")
                Button("Send") {}.disabled(true)
            }
            Spacer(minLength: 0)
            Text("Hub offline · Runtime not connected").font(.caption).foregroundStyle(.secondary)
        }.padding(20).frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

struct MicrophoneControls: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Microphone: \(model.permissions.microphone.rawValue)")
            if let guidance = model.permissions.microphone.microphoneGuidance {
                Text(guidance).foregroundStyle(.red).fixedSize(horizontal: false, vertical: true)
            }
            if model.permissions.microphone == .notDetermined {
                Button("Allow Microphone") { Task { await model.requestMicrophone() } }
            }
            Button("Microphone Settings") { model.openPrivacy("Privacy_Microphone") }
            Button("Refresh Permissions") { model.refreshPermissions() }
        }
    }
}
