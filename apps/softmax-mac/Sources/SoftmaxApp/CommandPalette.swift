import AppKit
import SwiftUI
import SoftmaxAppSurface

extension Notification.Name {
    static let softmaxFilterInput = Notification.Name("SoftmaxFilterInput")
}

struct SoftmaxCommand: Identifiable {
    let id: String
    let title: String
    let keywords: String
    let action: () -> Void
    static func catalog(settings: @escaping (SettingsTab) -> Void, face: @escaping () -> Void,
                        chat: @escaping (Bool) -> Void, about: @escaping () -> Void) -> [Self] {
        SettingsTab.allCases.map { tab in
            Self(id: "settings-\(tab.rawValue)", title: "Settings → \(tab.rawValue)", keywords: tab.keywords, action: { settings(tab) })
        } + [
            Self(id: "face", title: "Open Face", keywords: "companion expression", action: face),
            Self(id: "chat", title: "Open Chat", keywords: "conversation message", action: { chat(false) }),
            Self(id: "voice", title: "Open Voice", keywords: "talk speech", action: { chat(true) }),
            Self(id: "about", title: "About Softmax", keywords: "version licences licenses support source", action: about)
        ]
    }
}

/// Registers with the app control registry when a model is supplied (design
/// snapshots pass none); the same action closure serves user and adapter paths.
struct OptionalCogletButton: View {
    let title: String, id: String, window: String, route: String
    let model: ShellModel?
    let action: () -> Void
    init(_ title: String, id: String, model: ShellModel?, window: String, route: String, action: @escaping () -> Void) {
        self.title = title; self.id = id; self.model = model; self.window = window; self.route = route; self.action = action
    }
    var body: some View {
        if let model {
            CogletButton(title, id: id, registry: model.appControls, window: window, route: route,
                failure: model.appControlFailure, action: action)
        } else {
            Button(title, action: action)
        }
    }
}

private struct PaletteRow<Label: View>: View {
    let command: SoftmaxCommand
    let model: ShellModel?
    let action: () -> Void
    @ViewBuilder let label: Label
    var body: some View {
        if let model {
            Button(action: action) { label }.buttonStyle(.plain)
                .coglet(id: "command_palette.command.\(command.id)", registry: model.appControls, window: "command_palette",
                    route: "commands", role: "button", label: command.title, verbs: ["press"],
                    value: { .map([.init("kind", .text("none"))]) }, failure: model.appControlFailure,
                    invoke: { _ in action() })
        } else {
            Button(action: action) { label }.buttonStyle(.plain)
        }
    }
}

private struct PaletteSearchRegistration: ViewModifier {
    let model: ShellModel?
    @Binding var query: String
    let focused: () -> Bool
    let focus: () -> Void
    func body(content: Content) -> some View {
        if let model {
            content.coglet(id: "command_palette.search", registry: model.appControls, window: "command_palette",
                route: "commands", role: "text_field", label: "Search commands and settings", verbs: ["type_text"],
                value: { .map([.init("kind", .text("text")), .init("value", .text(query))]) },
                focus: { focus(); return true }, focused: focused,
                failure: model.appControlFailure, invoke: { args in
                    guard case .map(let fields) = args,
                          let value = fields.first(where: { $0.key == "value" }), case .text(let text) = value.value else {
                        throw ControlRegistryFailure.unsupported
                    }
                    query = text
                })
        } else { content }
    }
}

func highlightedMatch(_ text: String, query: String) -> Text {
    guard !query.isEmpty, let range = text.range(of: query, options: .caseInsensitive) else { return Text(text) }
    return Text(String(text[..<range.lowerBound])) + Text(String(text[range])).bold().underline()
        + Text(String(text[range.upperBound...]))
}

struct CommandPaletteView: View {
    let commands: [SoftmaxCommand]
    let dismiss: () -> Void
    @State var query = ""
    /// nil only for design snapshots, which have no registry to register with.
    var model: ShellModel? = nil
    @State private var selected = 0
    @FocusState private var focused: Bool
    @Environment(\.colorScheme) private var scheme
    private var accent: Color { scheme == .dark ? SoftmaxDesignTokens.Dark.accent : SoftmaxDesignTokens.Light.accent }
    private var matches: [SoftmaxCommand] {
        commands.filter { query.isEmpty || ($0.title + " " + $0.keywords).localizedCaseInsensitiveContains(query) }
    }
    private func open(_ command: SoftmaxCommand) { dismiss(); command.action() }
    private func revealSelection(using proxy: ScrollViewProxy) {
        guard matches.indices.contains(selected) else { return }
        // Immediate minimal scrolling keeps keyboard navigation usable with Reduce Motion too.
        var transaction = Transaction(animation: nil)
        transaction.disablesAnimations = true
        withTransaction(transaction) { proxy.scrollTo(matches[selected].id) }
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            TextField("Search commands and settings", text: $query)
                .modifier(SoftmaxSearchFieldFrame())
                .accessibilityLabel("Search commands and settings").focused($focused)
                .onChange(of: query) { _ in selected = 0 }
                .modifier(PaletteSearchRegistration(model: model, query: $query, focused: { focused }, focus: { focused = true }))
                .onSubmit { if matches.indices.contains(selected) { open(matches[selected]) } }
                // A focused TextField consumes arrow keys before onMoveCommand sees them.
                .onKeyPress(.downArrow) { selected = min(selected + 1, max(0, matches.count - 1)); return .handled }
                .onKeyPress(.upArrow) { selected = max(0, selected - 1); return .handled }
            Text("\(matches.count) results · ↑ ↓ select · Return open · Esc cancel")
                .softmaxFont(SoftmaxDesignTokens.fontCaption)
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 4) {
                        ForEach(Array(matches.enumerated()), id: \.element.id) { index, command in
                            PaletteRow(command: command, model: model, action: { open(command) }) {
                                highlightedMatch(command.title, query: query).frame(maxWidth: .infinity, alignment: .leading).padding(8)
                                    .background(index == selected ? accent.opacity(0.14) : .clear, in: RoundedRectangle(cornerRadius: 6))
                                    .overlay(RoundedRectangle(cornerRadius: 6).strokeBorder(index == selected ? accent : .clear, lineWidth: 2))
                            }
                                .accessibilityAddTraits(index == selected ? .isSelected : [])
                                .id(command.id)
                        }
                        if matches.isEmpty { Text("No matching commands or settings") }
                    }
                }
                .onChange(of: selected) { _ in revealSelection(using: proxy) }
                .onChange(of: matches.map(\.id)) { _ in revealSelection(using: proxy) }
            }
        }.padding(20).frame(width: 460, height: 480)
            .modifier(SoftmaxTheme()).onAppear { DispatchQueue.main.async { focused = true } }
            .onExitCommand(perform: dismiss)
    }
}

@MainActor final class CommandPaletteController: NSObject, NSWindowDelegate {
    var onChange: (() -> Void)?
    private var panel: NSPanel?
    private weak var previous: NSWindow?
    func show(commands: [SoftmaxCommand], model: ShellModel) {
        if let panel, panel.isVisible { NSApp.activate(ignoringOtherApps: true); panel.makeKeyAndOrderFront(nil); return }
        previous = NSApp.keyWindow
        let panel = NSPanel(contentRect: NSRect(x: 0, y: 0, width: 460, height: 480),
                            styleMask: [.titled, .closable], backing: .buffered, defer: false)
        panel.title = "Softmax Commands"; panel.isReleasedWhenClosed = false
        panel.identifier = NSUserInterfaceItemIdentifier("softmax.command_palette")
        panel.delegate = self
        panel.contentView = NSHostingView(rootView: SoftmaxWindowRoot {
            CommandPaletteView(commands: commands, dismiss: { [weak self] in self?.close() }, model: model)
        })
        self.panel = panel
        panel.center(); NSApp.activate(ignoringOtherApps: true); panel.makeKeyAndOrderFront(nil)
        onChange?()
    }
    func close() { panel?.close(); onChange?() }
    func windowWillClose(_ notification: Notification) {
        previous?.makeKeyAndOrderFront(nil)
        previous = nil
        // willClose precedes committed visibility. Capture the ending instance,
        // never let a deferred callback close or mutate a successor panel.
        let closing = panel
        Task { @MainActor [weak self, weak closing] in
            guard let self, self.panel === closing else { return }
            self.onChange?()
        }
    }
}
