import SwiftUI
import SoftmaxRuntime
import SoftmaxAppSurface

struct RuntimeSettings: View {
    @ObservedObject var host: RuntimeHost
    let openLogs: () -> Void
    let restart: () -> Void
    let registry: ControlRegistry
    let failure: (ControlRegistryFailure) -> Void
    var window = "settings"
    var route = "coglets"
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Runtime: \(host.state.rawValue)").fontWeight(.semibold).accessibilityIdentifier("runtimeStatus")
            if let error = host.errorMessage { Text(error).foregroundStyle(.red).textSelection(.enabled) }
            if host.coglets.isEmpty { Text("No installed coglet loaded. Install a trusted runtime configuration first.") }
            ForEach(host.coglets) { coglet in
                VStack(alignment: .leading) {
                    Text(coglet.id).textSelection(.enabled)
                    Text("Manifest \(coglet.version) · \(coglet.status.rawValue)")
                }
            }
            HStack {
                // Keeps the pre-existing stable UI-test identifier as the semantic id.
                CogletButton("Restart runtime", id: "restartRuntime", registry: registry, window: window,
                    route: route, adapterSynchronous: false, failure: failure, action: restart)
                CogletButton("Open logs", id: "settings.coglets.open_logs", registry: registry, window: window,
                    route: route, failure: failure, action: openLogs)
            }
            Text("Restart reconciles the local surface ledger and establishes fresh grants. Process-fatal faults are not isolated. Speech output is currently an unavailable Face sink, not a playback-success claim.")
        }
    }
}
