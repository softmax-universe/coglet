import AppKit
import SwiftUI

@MainActor final class TalkHUD {
    private var panel: NSPanel?
    func update(model: ShellModel) {
        guard model.talkState == .listening else { panel?.orderOut(nil); return }
        if panel == nil {
            let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 240, height: 72),
                styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
            panel.level = .statusBar
            panel.hidesOnDeactivate = false
            panel.isReleasedWhenClosed = false
            panel.ignoresMouseEvents = true
            panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
            panel.contentView = NSHostingView(rootView: TalkHUDView(model: model))
            self.panel = panel
        }
        if let screen = NSScreen.main ?? NSScreen.screens.first {
            let area = screen.visibleFrame
            panel?.setFrameOrigin(NSPoint(x: area.maxX - 264, y: area.maxY - 88))
        }
        panel?.orderFrontRegardless()
    }
}

private struct TalkHUDView: View {
    @ObservedObject var model: ShellModel
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Image(systemName: "mic.fill").foregroundStyle(.red)
                Text("Listening")
                Spacer()
                Text(String(format: "%02d:%02d", Int(model.elapsed) / 60, Int(model.elapsed) % 60))
                    .monospacedDigit()
            }
            ProgressView(value: Double(model.level), total: 1)
                .accessibilityLabel("Microphone level")
        }
        .padding(14).frame(width: 240, height: 72)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
    }
}
