import AppKit
import SwiftUI
import SoftmaxCore

enum HUDPresentation: String, CaseIterable {
    case idle, listening, thinking, speaking, cancelled, microphoneDenied = "microphone-denied", offline, error
    var title: String {
        switch self {
        case .idle: "Hold to talk"
        case .listening: "Listening · release to send"
        case .thinking: "Thinking"
        case .speaking: "Speaking · Esc to stop"
        case .cancelled: "Cancelled · nothing sent"
        case .microphoneDenied: "Microphone access is off"
        case .offline: "Hub disconnected"
        case .error: "Audio stopped · check Settings"
        }
    }
}

@MainActor final class TalkHUD {
    private var panel: NSPanel?
    private var last: AppState = .idle
    private var hideTask: Task<Void, Never>?
    func update(model: ShellModel) {
        let state: HUDPresentation
        switch model.talkState {
        case .listening: state = .listening
        case .thinking: state = .thinking
        case .speaking: state = .speaking
        case .error: state = .error
        case .offline: state = .offline
        default:
            if last == .listening { state = .cancelled }
            else { panel?.orderOut(nil); last = model.talkState; return }
        }
        let changed = last != model.talkState
        last = model.talkState
        hideTask?.cancel()
        if panel == nil {
            let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 320, height: 88),
                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.level = .statusBar; panel.hidesOnDeactivate = false
            panel.isReleasedWhenClosed = false; panel.ignoresMouseEvents = true
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            self.panel = panel
        }
        panel?.contentView = NSHostingView(rootView: SoftmaxWindowRoot {
            TalkHUDView(state: state, level: model.level, elapsed: model.elapsed, hotkey: model.hotkeyName)
        })
        if let screen = NSScreen.main ?? NSScreen.screens.first {
            let area = screen.visibleFrame
            panel?.setFrameOrigin(NSPoint(x: area.maxX - 344, y: area.maxY - 104))
        }
        if changed, let panel, !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion {
            panel.alphaValue = 0; panel.orderFrontRegardless()
            NSAnimationContext.runAnimationGroup { context in
                context.duration = Double(SoftmaxDesignTokens.motionHudFadeMs) / 1000
                panel.animator().alphaValue = 1
            }
        } else { panel?.alphaValue = 1; panel?.orderFrontRegardless() }
        if state == .cancelled {
            hideTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(1.5))
                guard !Task.isCancelled else { return }
                self?.panel?.orderOut(nil)
            }
        }
        if changed {
            NSAccessibility.post(element: NSApp as Any, notification: .announcementRequested,
                userInfo: [.announcement: state.title, .priority: NSAccessibilityPriorityLevel.medium.rawValue])
        }
    }
}

struct TalkHUDView: View {
    var state: HUDPresentation
    var level: Float
    var elapsed: TimeInterval
    var hotkey: String
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[1]) {
            HStack {
                Text(state.title).fontWeight(.semibold).fixedSize(horizontal: false, vertical: true)
                Spacer(minLength: 4)
                if state == .listening {
                    Text(String(format: "%02d:%02d", Int(elapsed) / 60, Int(elapsed) % 60)).monospacedDigit()
                }
            }
            if state == .listening {
                ProgressView(value: Double(min(1, max(0, level))), total: 1).accessibilityLabel("Microphone level")
            } else { Text(state == .idle ? hotkey : "Esc to cancel").font(.system(size: SoftmaxDesignTokens.fontCaption)) }
        }.padding(SoftmaxDesignTokens.spacing[3]).frame(width: 320, height: 88)
            .modifier(SoftmaxTheme()).clipShape(RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusWindow))
    }
}
