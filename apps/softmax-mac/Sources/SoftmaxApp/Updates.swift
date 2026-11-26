import SwiftUI
import Sparkle
import SoftmaxAppSurface

@MainActor private final class UpdateAvailability: NSObject, SPUUpdaterDelegate {
    var changed: ((Bool) -> Void)?
    // Sparkle ab7d712: both SPUUpdater completion paths clear session state and
    // synchronously deliver abort/finish before returning on main. This delegate
    // and its availability observer never start a new cycle reentrantly. No old
    // finish is dispatched after a successor can enter this command. Requalify
    // that ordering on a Sparkle upgrade; these callbacks have no cycle id.
    let command = UpdateCheckCommand()
    func updater(_ updater: SPUUpdater, didFindValidUpdate item: SUAppcastItem) {
        changed?(true); command.foundUpdate()
    }
    func updaterDidNotFindUpdate(_ updater: SPUUpdater, error: Error) {
        changed?(false); command.noUpdateFound()
    }
    func updater(_ updater: SPUUpdater, didAbortWithError error: Error) { command.aborted() }
    func updater(_ updater: SPUUpdater, didFinishUpdateCycleFor updateCheck: SPUUpdateCheck, error: Error?) {
        command.finishedCycle()
    }
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

    func checkForUpdates(context: ControlRegistry.EffectContext?) async throws {
        try await availability.command.run(context: context, available: {
            // canCheckForUpdates alone permits existing sessions. Never attach
            // this command's completion to a pre-existing/background cycle.
            self.enabled && self.controller.updater.canCheckForUpdates && !self.controller.updater.sessionInProgress
        }, start: {
            self.controller.checkForUpdates(nil)
            return self.controller.updater.sessionInProgress
        })
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
            CogletAsyncButton("Check for Updates…", id: "\(idPrefix).check", registry: registry, window: window, route: route,
                sensitive: true, failure: failure) { context in
                    try await updates.checkForUpdates(context: context)
                }.disabled(!updates.enabled)
            if !updates.enabled {
                Text("Updates are disabled in development and ad-hoc builds. Install a notarized Developer ID release to enable them.")
            }
        }
    }
}
