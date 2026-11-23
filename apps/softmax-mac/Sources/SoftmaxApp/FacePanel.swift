import AppKit
import SwiftUI
import Combine
import SoftmaxCore
import FaceMac

/// Runtime replaces the driver and feeds the sink; this host never synthesizes or admits actions.
@MainActor
final class FaceHost: ObservableObject {
    let driver: any FaceDriver
    let speech: FaceSpeechSink
    @Published var revision = 0
    @Published var compact = false
    @Published var demo = false
    private var timer: Timer?
    private var demoStart = 0.0
    private let playback: FacePlaybackSession
    private weak var shell: ShellModel?
    init(shell: ShellModel, driver: any FaceDriver = LocalFaceDriver()) {
        self.shell = shell; self.driver = driver
        speech = FaceSpeechSink(device: EngineFacePlayback())
        playback = FacePlaybackSession(ptt: shell.ptt, speech: speech)
        driver.onChange = { [weak self] in self?.revision &+= 1 }
        speech.onChange = { [weak self] in
            guard let self, speech.active else { return }
            driver.setState(.speaking(level: speech.level))
        }
        playback.onFinish = { [weak self] in
            guard let self else { return }
            driver.setState(.idle)
        }
    }
    func beginSpeech() -> UUID {
        stopDemo()
        let token = playback.begin()
        shell?.stopFacePlayback = { [weak self] in self?.playback.stop() }
        return token
    }
    func startDemo() {
        speech.stop(); demo = true; demoStart = ProcessInfo.processInfo.systemUptime
    }
    func stopDemo() { demo = false; driver.reset() }
    func updateFromShell() {
        guard let shell else { return }
        if shell.state == .listening && demo { stopDemo() }
        guard !demo, !speech.active else { return }
        switch shell.state {
        case .listening: driver.setState(.listening(level: Double(shell.level)))
        case .thinking: driver.setState(.thinking)
        default: driver.setState(.idle)
        }
    }
    func setVisible(_ visible: Bool) {
        timer?.invalidate(); timer = nil
        guard visible else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.driver.tick(); self.speech.tick()
                if self.demo { DemoFaceDriver(driver: self.driver).step(seconds: ProcessInfo.processInfo.systemUptime - self.demoStart) }
            }
        }
    }
    func cancel() { shell?.ptt.cancel(); speech.stop(); stopDemo() }
}

@MainActor
final class FacePanelController: NSObject, NSWindowDelegate {
    let host: FaceHost
    private let panel: NSPanel
    private var localEscape: Any?
    private var globalEscape: Any?
    private let persist: Bool
    init(shell: ShellModel) {
        host = FaceHost(shell: shell); persist = !shell.testing
        panel = FacePanel(contentRect: NSRect(x: 0, y: 0, width: 280, height: 330),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        super.init()
        panel.title = "Softmax Face"; panel.level = .floating
        panel.isReleasedWhenClosed = false; panel.isMovableByWindowBackground = true
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.delegate = self
        panel.contentView = NSHostingView(rootView: FacePanelView(host: host, hide: { [weak self] in self?.hide() }, resize: { [weak self] in self?.resize() }))
        if !persist || !panel.setFrameUsingName("SoftmaxFace") {
            if let frame = NSScreen.main?.visibleFrame {
                panel.setFrameOrigin(CGPoint(x: frame.maxX - 300, y: frame.maxY - 350))
            }
        }
        panel.setContentSize(NSSize(width: 280, height: 330))
        clampToScreen()
    }
    func show() {
        clampToScreen(); host.setVisible(true); panel.orderFrontRegardless()
        guard localEscape == nil else { return }
        localEscape = NSEvent.addLocalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 { self?.hide(); return nil }; return event
        }
        // Requires an existing keyboard-observation grant; never requests one.
        // Escape dismisses this transient Face while another app keeps focus.
        globalEscape = NSEvent.addGlobalMonitorForEvents(matching: .keyDown) { [weak self] event in
            if event.keyCode == 53 { Task { @MainActor in self?.hide() } }
        }
    }
    func hide() {
        host.cancel(); host.setVisible(false); panel.orderOut(nil)
        if let localEscape { NSEvent.removeMonitor(localEscape) }
        if let globalEscape { NSEvent.removeMonitor(globalEscape) }
        localEscape = nil; globalEscape = nil
    }
    func windowDidMove(_ notification: Notification) { if persist { panel.saveFrame(usingName: "SoftmaxFace") } }
    private func resize() {
        let size = host.compact ? NSSize(width: 100, height: 140) : NSSize(width: 280, height: 330)
        let top = panel.frame.maxY
        panel.setFrame(NSRect(x: panel.frame.minX, y: top - size.height, width: size.width, height: size.height), display: true)
        clampToScreen()
    }
    private func clampToScreen() {
        let screen = NSScreen.screens.first { $0.visibleFrame.intersects(panel.frame) } ?? NSScreen.main
        guard let frame = screen?.visibleFrame else { return }
        panel.setFrameOrigin(CGPoint(x: max(frame.minX, min(panel.frame.minX, frame.maxX - panel.frame.width)),
                                     y: max(frame.minY, min(panel.frame.minY, frame.maxY - panel.frame.height))))
    }
}

@MainActor
private final class FacePanel: NSPanel {
    override var canBecomeKey: Bool { true }
    override var canBecomeMain: Bool { false }
}

struct FacePanelView: View {
    @ObservedObject var host: FaceHost
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var hide: () -> Void
    var resize: () -> Void
    var body: some View {
        VStack(spacing: 4) {
            FaceArtwork(parameters: host.driver.parameters(reduceMotion: reduceMotion))
                .animation(reduceMotion ? nil : .easeOut(duration: 0.07), value: host.revision)
                .accessibilityElement().accessibilityLabel("Face, \(host.driver.state.label)")
            if !host.compact {
                Text(host.demo ? "Simulated demo · no audio" : host.driver.state.label)
                HStack {
                    Button(host.demo ? "Stop demo" : "Start simulated demo") {
                        if host.demo { host.stopDemo() } else { host.startDemo() }
                    }
                    Button("Stop") { host.cancel() }
                }
            }
            HStack {
                Button(host.compact ? "Expand Face" : "Compact Face") { host.compact.toggle(); resize() }
                Button("Hide Face", action: hide).keyboardShortcut(.escape, modifiers: [])
            }.font(.caption)
        }.padding(8).background(.regularMaterial)
    }
}
