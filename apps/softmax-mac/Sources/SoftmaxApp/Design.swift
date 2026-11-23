import SwiftUI
import AppKit

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

struct AboutView: View {
    @State var licenses = false
    private var version: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development" }
    private var build: String { Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unversioned" }
    private var information: String { "Softmax \(version) (\(build)) · \(Bundle.main.object(forInfoDictionaryKey: "SoftmaxReleaseChannel") as? String ?? "development")\nRuntime: not connected" }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
                Image(nsImage: NSImage(named: NSImage.applicationIconName) ?? NSImage()).resizable().frame(width: 72, height: 72).accessibilityHidden(true)
                Text("Softmax for Mac").font(.system(size: SoftmaxDesignTokens.fontTitle, weight: .semibold))
                Text(information).textSelection(.enabled).fixedSize(horizontal: false, vertical: true)
                Button("Copy version information") {
                    NSPasteboard.general.clearContents(); NSPasteboard.general.setString(information, forType: .string)
                }
                HStack {
                    Link("Support", destination: URL(string: "https://github.com/softmax-universe/coglet/issues")!)
                    Link("Source", destination: URL(string: "https://github.com/softmax-universe/coglet")!)
                }
                DisclosureGroup("Licences and acknowledgements", isExpanded: $licenses) {
                    VStack(alignment: .leading, spacing: 8) {
                        Text("Sparkle — MIT licence and third-party notices. Full Sparkle licence texts ship with this app. Softmax and Coglet source is available from the repository.")
                        Link("Sparkle licence", destination: URL(string: "https://github.com/sparkle-project/Sparkle/blob/2.10.1/LICENSE")!)
                        if let url = Bundle.main.url(forResource: "ThirdPartyNotices", withExtension: "txt") { Link("Read bundled licences", destination: url) }
                    }.padding(.top, 8)
                }
            }.padding(SoftmaxDesignTokens.spacing[4])
        }.frame(minWidth: 420, minHeight: 380).modifier(SoftmaxTheme())
    }
}
