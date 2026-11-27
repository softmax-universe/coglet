import AppKit
import SwiftUI
import Combine
import SoftmaxCore
import FaceMac
import SoftmaxRuntime

/// Product renderer for admitted runtime actions; this host grants no authority.
@MainActor
final class FaceHost: ObservableObject {
    let driver: any FaceDriver
    let speech: FaceSpeechSink
    @Published var revision = 0
    @Published var compact = false
    @Published var demo = false
    /// True only while the Face panel is on screen; hidden panels keep their hosting view.
    @Published private(set) var visible = false
    private var timer: Timer?
    private var demoStart = 0.0
    private let playback: FacePlaybackSession
    private let runtimeSpeech: RuntimeSpeechPlayback
    private weak var shell: ShellModel?
    init(shell: ShellModel, driver: any FaceDriver = LocalFaceDriver()) {
        self.shell = shell; self.driver = driver
        speech = FaceSpeechSink(device: EngineFacePlayback())
        playback = FacePlaybackSession(ptt: shell.ptt, speech: speech)
        runtimeSpeech = RuntimeSpeechPlayback(session: playback, synthesis: AppleSpeechSynthesis())
        driver.onChange = { [weak self] in self?.revision &+= 1 }
        speech.onChange = { [weak self] in
            guard let self, speech.active else { return }
            driver.setState(.speaking(level: speech.level))
        }
        playback.onFinish = { [weak self] in
            guard let self else { return }
            driver.setState(.idle)
        }
        runtimeSpeech.willBegin = { [weak self] in self?.stopDemo() }
    }
    func perform(_ action: RuntimeAction, completion: @escaping (String?) -> Void) {
        switch action.capability {
        case "speech.speak_chunk":
            guard let text = action.text, let stream = action.stream, let chunk = action.chunk else {
                completion("invalid_argument"); return
            }
            runtimeSpeech.enqueue(text: text, stream: stream, chunk: chunk, final: action.final, deadline: action.deadline, completion: completion)
        case "expression.set_expression":
            guard let name = action.expression, let intensity = action.intensity, intensity <= 1000,
                  let duration = action.durationMS, duration <= 60000 else { completion("invalid_argument"); return }
            do {
                try driver.setExpression(name, intensity: Int(intensity), durationMS: Int(duration))
                completion(nil)
            } catch { completion("invalid_argument") }
        default: completion("unavailable")
        }
    }
    func stopRuntimeOutput() { runtimeSpeech.stop(); driver.reset() }
    func beginSpeech() -> UUID {
        stopDemo()
        let token = playback.begin()
        shell?.stopFacePlayback = { [weak self] in self?.playback.stop() }
        return token
    }
    func startDemo() {
        shell?.runtime.cancelOutput(); demo = true; demoStart = ProcessInfo.processInfo.systemUptime
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
        if self.visible != visible { self.visible = visible }
        guard visible else { return }
        timer = Timer.scheduledTimer(withTimeInterval: 1.0 / 30, repeats: true) { [weak self] _ in
            Task { @MainActor in
                guard let self else { return }
                self.driver.tick(); self.speech.tick()
                if self.demo { DemoFaceDriver(driver: self.driver).step(seconds: ProcessInfo.processInfo.systemUptime - self.demoStart) }
            }
        }
    }
    func cancel() { shell?.runtime.cancelOutput(); shell?.ptt.cancel(); stopRuntimeOutput(); stopDemo() }
}

@MainActor
final class FacePanelController: NSObject, NSWindowDelegate {
    var onChange: (() -> Void)?
    let host: FaceHost
    private let panel: NSPanel
    private var localEscape: Any?
    private var globalEscape: Any?
    private let persist: Bool
    init(shell: ShellModel) {
        host = shell.faceHost; persist = !shell.testing
        panel = FacePanel(contentRect: NSRect(x: 0, y: 0, width: 280, height: 330),
                        styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: false)
        super.init()
        panel.title = "Softmax Face"; panel.level = .floating
        panel.identifier = NSUserInterfaceItemIdentifier("softmax.face")
        panel.isReleasedWhenClosed = false; panel.isMovableByWindowBackground = true
        panel.hidesOnDeactivate = false
        panel.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]
        panel.delegate = self
        panel.contentView = NSHostingView(rootView: SoftmaxWindowRoot { FacePanelView(host: host, hide: { [weak self] in self?.hide() }, resize: { [weak self] in self?.resize() }, model: shell) })
        if !persist || !panel.setFrameUsingName("SoftmaxFace") {
            if let frame = NSScreen.main?.visibleFrame {
                panel.setFrameOrigin(CGPoint(x: frame.maxX - 300, y: frame.minY + 20))
            }
        }
        panel.setContentSize(NSSize(width: 280, height: 330))
        clampToScreen()
    }
    func show() {
        clampToScreen(); host.setVisible(true); panel.orderFrontRegardless()
        onChange?()
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
        onChange?()
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
    @Environment(\.softmaxFreezeMotion) private var freezeMotion
    var hide: () -> Void
    var resize: () -> Void
    /// nil only for design snapshots, which have no registry to register with.
    var model: ShellModel? = nil
    /// Snapshots (model nil) always render controls; a model-backed hidden panel does not.
    private var mountsControls: Bool { model == nil || host.visible }
    var body: some View {
        VStack(spacing: SoftmaxDesignTokens.spacing[0]) {
            FaceArtwork(parameters: host.driver.parameters(reduceMotion: reduceMotion || freezeMotion))
                .animation(reduceMotion || freezeMotion ? nil : .easeOut(duration: 0.07), value: host.revision)
                .accessibilityElement().accessibilityLabel("Face, \(host.driver.state.label)")
            if mountsControls && !host.compact {
                Text(host.demo ? "Simulated demo · no audio" : host.driver.state.label)
                HStack {
                    OptionalCogletButton(host.demo ? "Stop demo" : "Start simulated demo", id: "face.demo_toggle", model: model,
                        window: "face", route: "face") {
                        if host.demo { host.stopDemo() } else { host.startDemo() }
                    }
                    OptionalCogletButton("Stop", id: "face.stop", model: model, window: "face", route: "face") { host.cancel() }
                }
            }
            if mountsControls { HStack {
                OptionalCogletButton(host.compact ? "Expand Face" : "Compact Face", id: "face.compact_toggle", model: model,
                    window: "face", route: "face") { host.compact.toggle(); resize() }
                OptionalCogletButton("Hide Face", id: "face.hide", model: model, window: "face", route: "face", action: hide)
                    .keyboardShortcut(.escape, modifiers: [])
            }.softmaxFont(SoftmaxDesignTokens.fontCaption) }
        }.padding(SoftmaxDesignTokens.spacing[1]).modifier(SoftmaxTheme())
    }
}
