import SwiftUI
import Sparkle
import SoftmaxAppSurface

@MainActor private final class UpdateAvailability: NSObject, SPUUpdaterDelegate {
    var changed: ((Bool) -> Void)?
    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) { changed?(true) }
    func updaterDidNotFindUpdate(_ updater: SPUUpdater, error: Error) { changed?(false) }
}

/// One updater shared by the menu and Settings. Development/UI-test builds never
/// start network checks; only a verified Developer ID release enables updating.
@MainActor
final class UpdateController: ObservableObject {
    private let controller: SPUStandardUpdaterController
    private let availability = UpdateAvailability()
    @Published private(set) var updateAvailable = false
    let enabled: Bool
    @Published var automaticallyChecks: Bool {
        didSet { if enabled { controller.updater.automaticallyChecksForUpdates = automaticallyChecks } }
    }

    init() {
        enabled = Bundle.main.object(forInfoDictionaryKey: "SoftmaxReleaseChannel") as? String == "developer-id"
            && !ProcessInfo.processInfo.arguments.contains("--ui-testing")
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: availability, userDriverDelegate: nil)
        automaticallyChecks = controller.updater.automaticallyChecksForUpdates
        availability.changed = { [weak self] in self?.updateAvailable = $0 }
        if enabled { controller.startUpdater() }
    }

    func checkForUpdates() {
        guard enabled, controller.updater.canCheckForUpdates else { return }
        controller.checkForUpdates(nil)
    }
}

struct UpdatesView: View {
    @ObservedObject var updates: UpdateController
    let registry: ControlRegistry
    let failure: (ControlRegistryFailure) -> Void
    var window = "settings"
    var route = "updates"
    var idPrefix = "settings.updates"
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Version \(Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "development")")
            CogletToggle("Automatically check for updates", id: "\(idPrefix).automatic_checks", registry: registry,
                window: window, route: route, isOn: $updates.automaticallyChecks, failure: failure)
                .disabled(!updates.enabled)
            CogletButton("Check for Updates…", id: "\(idPrefix).check", registry: registry, window: window, route: route,
                sensitive: true, adapterSynchronous: false, failure: failure) { updates.checkForUpdates() }.disabled(!updates.enabled)
            if !updates.enabled {
                Text("Updates are disabled in development and ad-hoc builds. Install a notarized Developer ID release to enable them.")
            }
        }
    }
}
