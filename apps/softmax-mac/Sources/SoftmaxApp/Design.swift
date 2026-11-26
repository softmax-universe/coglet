import SwiftUI
import AppKit
import SoftmaxAppSurface

/// This app has no Touch Bar controls. Do not expose an empty generated bar.
final class SoftmaxHostingView<Content: View>: NSHostingView<Content> {
    override func makeTouchBar() -> NSTouchBar? { nil }
}

private struct SoftmaxTextScaleKey: EnvironmentKey { static let defaultValue = 1.0 }
private struct SoftmaxHighContrastKey: EnvironmentKey { static let defaultValue = false }
private struct SoftmaxFreezeMotionKey: EnvironmentKey { static let defaultValue = false }
extension EnvironmentValues {
    var softmaxTextScale: Double { get { self[SoftmaxTextScaleKey.self] } set { self[SoftmaxTextScaleKey.self] = newValue } }
    var softmaxHighContrast: Bool { get { self[SoftmaxHighContrastKey.self] } set { self[SoftmaxHighContrastKey.self] = newValue } }
    var softmaxFreezeMotion: Bool { get { self[SoftmaxFreezeMotionKey.self] } set { self[SoftmaxFreezeMotionKey.self] = newValue } }
}

/// Keep explicit title/caption tokens in the same text-size system as body copy.
struct SoftmaxTokenFont: ViewModifier {
    var size: CGFloat
    var weight: Font.Weight = .regular
    @Environment(\.softmaxTextScale) private var scale
    func body(content: Content) -> some View {
        content.font(.system(size: size * scale, weight: weight))
    }
}
extension View {
    func softmaxFont(_ size: CGFloat, weight: Font.Weight = .regular) -> some View {
        modifier(SoftmaxTokenFont(size: size, weight: weight))
    }
}

/// Reviewed search-control geometry. Scale the text region, not the fixed chrome:
/// the standard/largest design anchors are 25/35 pt, not 25/40 pt.
enum SoftmaxSearchFieldMetrics {
    static let standardHeight: CGFloat = 25
    static let largestHeight: CGFloat = 35
    static func height(textScale: Double) -> CGFloat {
        standardHeight + (largestHeight - standardHeight) * (textScale - 1) / 0.6
    }
}

struct SoftmaxSearchFieldFrame: ViewModifier {
    @Environment(\.softmaxTextScale) private var scale
    func body(content: Content) -> some View {
        // AppKit intrinsic measurement can retain the previous text scale. Give
        // both production search surfaces the same explicit layout proposal.
        content.frame(height: SoftmaxSearchFieldMetrics.height(textScale: scale))
    }
}

/// JSON-generated values are compiled directly from Design/DesignTokens.swift.
/// Increased contrast uses native semantic foregrounds and explicit group outlines.
struct SoftmaxTheme: ViewModifier {
    @ScaledMetric(relativeTo: .body) private var bodySize = SoftmaxDesignTokens.fontBody
    @Environment(\.colorScheme) private var scheme
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.softmaxTextScale) private var scale
    @Environment(\.softmaxHighContrast) private var forceContrast
    func body(content: Content) -> some View {
        content
            .font(.system(size: bodySize * scale))
            .foregroundStyle(contrast == .increased || forceContrast ? Color.primary : (scheme == .dark ? SoftmaxDesignTokens.Dark.ink : SoftmaxDesignTokens.Light.ink))
            .tint(scheme == .dark ? SoftmaxDesignTokens.Dark.accent : SoftmaxDesignTokens.Light.accent)
            .background(scheme == .dark ? SoftmaxDesignTokens.Dark.background : SoftmaxDesignTokens.Light.background)
    }
}

/// Error copy uses the reviewed token pairs, not the low-contrast system red.
struct SoftmaxErrorText: ViewModifier {
    @Environment(\.colorScheme) private var scheme
    func body(content: Content) -> some View {
        content.foregroundStyle(scheme == .dark ? SoftmaxDesignTokens.Dark.danger : SoftmaxDesignTokens.Light.danger)
    }
}

struct DesignGroup<Content: View>: View {
    var label = "Options"
    @Environment(\.colorScheme) private var scheme
    @Environment(\.colorSchemeContrast) private var contrast
    @Environment(\.softmaxHighContrast) private var forceContrast
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) { content }
            // Native labels (including radio-group titles) must resolve against
            // this opaque surface, not a hierarchical style inherited across it.
            .foregroundColor(scheme == .dark ? SoftmaxDesignTokens.Dark.ink : SoftmaxDesignTokens.Light.ink)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(SoftmaxDesignTokens.spacing[3])
            .background(scheme == .dark ? SoftmaxDesignTokens.Dark.surface : SoftmaxDesignTokens.Light.surface,
                        in: RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusGroup))
            .overlay(RoundedRectangle(cornerRadius: SoftmaxDesignTokens.radiusGroup)
                .strokeBorder(contrast == .increased || forceContrast ? Color.primary : (scheme == .dark ? SoftmaxDesignTokens.Dark.border : SoftmaxDesignTokens.Light.border), lineWidth: contrast == .increased || forceContrast ? 2 : 1))
            .accessibilityElement(children: .contain)
            .accessibilityLabel(label)
    }
}

struct SoftmaxWindowRoot<Content: View>: View {
    @AppStorage("appearance") private var appearance = "System"
    @AppStorage("textScale") private var textScale = 1.0
    @ViewBuilder var content: Content
    var body: some View {
        content.modifier(SoftmaxTheme())
            .environment(\.softmaxTextScale, textScale)
            .preferredColorScheme(appearance == "Dark" ? .dark : appearance == "Light" ? .light : nil)
    }
}

/// Links remain native OpenURLAction commands, not an adapter-only actuator.
private struct AboutLink: View {
    let title: String, id: String
    let url: URL
    let model: ShellModel?
    @Environment(\.openURL) private var openURL
    private func open(_ context: ControlRegistry.EffectContext?) async throws {
        try await URLOpenCommand(url: url, open: { url, completion in
            openURL(url, completion: completion)
        }).run(context: context)
    }
    var body: some View {
        if let model {
            CogletAsyncButton(title, id: id, registry: model.appControls, window: "about", route: "about",
                failure: model.appControlFailure,
                userFailure: { model.message = "The link could not be opened." }, action: open)
                .buttonStyle(.link)
        } else {
            Link(title, destination: url)
        }
    }
}

private struct AboutControlRegistration: ViewModifier {
    let model: ShellModel?
    let id: String, role: String, label: String
    let verbs: Set<String>
    let value: () -> CBORValue
    let invoke: (CBORValue) throws -> Void
    func body(content: Content) -> some View {
        if let model {
            content.coglet(id: id, registry: model.appControls, window: "about", route: "about",
                role: role, label: label, verbs: verbs, value: value,
                failure: model.appControlFailure, invoke: invoke)
        } else { content }
    }
}

struct AboutView: View {
    @State var licenses = false
    var model: ShellModel? = nil
    private var version: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development" }
    private var build: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unversioned" }
    private var information: String { "Softmax \(version) (\(build)) · \(Bundle.main.object(forInfoDictionaryKey: "SoftmaxReleaseChannel") as? String ?? "development")\nRuntime: not connected" }
    private func copyInformation() throws {
        NSPasteboard.general.clearContents()
        guard NSPasteboard.general.setString(information, forType: .string) else {
            throw ControlRegistryFailure.unavailable
        }
    }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
                Image(nsImage: NSImage(named: NSImage.applicationIconName) ?? NSImage()).resizable().frame(width: 72, height: 72).accessibilityHidden(true)
                Text("Softmax for Mac").softmaxFont(SoftmaxDesignTokens.fontTitle, weight: .semibold)
                Text(information).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Button("Copy version information") {
                    do { try copyInformation() } catch { model?.message = "Version information could not be copied." }
                }.modifier(AboutControlRegistration(model: model, id: "about.copy_version", role: "button",
                    label: "Copy version information", verbs: ["press"],
                    value: { .map([.init("kind", .text("none"))]) }, invoke: { _ in try copyInformation() }))
                HStack {
                    AboutLink(title: "Support", id: "about.support", url: URL(string: "https://github.com/softmax-universe/coglet/issues")!, model: model)
                    AboutLink(title: "Source", id: "about.source", url: URL(string: "https://github.com/softmax-universe/coglet")!, model: model)
                }
                DisclosureGroup("Licences and acknowledgements", isExpanded: $licenses) {
                    // Do not retain actionable mounted children while collapsed.
                    if licenses {
                        VStack(alignment: .leading, spacing: 8) {
                            Text("Sparkle — MIT licence and third-party notices. Full Sparkle licence texts ship with this app. Softmax and Coglet source is available from the repository.")
                            AboutLink(title: "Sparkle licence", id: "about.sparkle_license", url: URL(string: "https://github.com/sparkle-project/Sparkle/blob/2.10.1/LICENSE")!, model: model)
                            if let url = Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt") {
                                AboutLink(title: "Read bundled licences", id: "about.bundled_licenses", url: url, model: model)
                            }
                        }.padding(.top, 8)
                    }
                }.modifier(AboutControlRegistration(model: model, id: "about.licenses", role: "disclosure",
                    label: "Licences and acknowledgements", verbs: ["toggle"],
                    value: { .map([.init("kind", .text("boolean")), .init("value", .boolean(licenses))]) },
                    invoke: { args in
                        guard case .map(let fields) = args,
                              let field = fields.first(where: { $0.key == "value" }), case .boolean(let expanded) = field.value else {
                            throw ControlRegistryFailure.unsupported
                        }
                        licenses = expanded
                    }))
            }.padding(SoftmaxDesignTokens.spacing[4])
        }.frame(minWidth: 420, minHeight: 380).modifier(SoftmaxTheme())
    }
}
