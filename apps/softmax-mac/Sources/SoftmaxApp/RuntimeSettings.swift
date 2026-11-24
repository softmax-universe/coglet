import SwiftUI
import SoftmaxRuntime

struct RuntimeSettings: View {
    @ObservedObject var host: RuntimeHost
    let openLogs: () -> Void
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
                Button("Restart runtime") { Task { await host.restart() } }.accessibilityIdentifier("restartRuntime")
                Button("Open logs", action: openLogs)
            }
            Text("Restart reconciles the local surface ledger and establishes fresh grants. Process-fatal faults are not isolated. Speech output is currently an unavailable Face sink, not a playback-success claim.")
        }
    }
}
