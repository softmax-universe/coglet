import SwiftUI
import Sparkle

/// One updater shared by the menu and Settings. Development/UI-test builds never
/// start network checks; only a verified Developer ID release enables updating.
@MainActor
final class UpdateController: ObservableObject {
    private let controller: SPUStandardUpdaterController
    let enabled: Bool
    @Published var automaticallyChecks: Bool {
        didSet { if enabled { controller.updater.automaticallyChecksForUpdates = automaticallyChecks } }
    }

    init() {
        enabled = Bundle.main.object(forInfoDictionaryKey: "SoftmaxReleaseChannel") as? String == "developer-id"
            && !ProcessInfo.processInfo.arguments.contains("--ui-testing")
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: nil, userDriverDelegate: nil)
        automaticallyChecks = controller.updater.automaticallyChecksForUpdates
        if enabled { controller.startUpdater() }
    }

    func checkForUpdates() {
        guard enabled, controller.updater.canCheckForUpdates else { return }
        controller.checkForUpdates(nil)
    }
}

struct UpdatesView: View {
    @ObservedObject var updates: UpdateController
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development")")
            Toggle("Automatically check for updates", isOn: $updates.automaticallyChecks)
                .disabled(!updates.enabled)
            Button("Check for Updates…") { updates.checkForUpdates() }.disabled(!updates.enabled)
            if !updates.enabled {
                Text("Updates are disabled in development and ad-hoc builds. Install a notarized Developer ID release to enable them.")
            }
        }
    }
}
