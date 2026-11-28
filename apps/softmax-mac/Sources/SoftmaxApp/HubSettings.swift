import SwiftUI
import AppKit
import CoreImage
import UniformTypeIdentifiers
import SoftmaxCore

/// Shared Configure surface; the host owns its existing group and navigation.
struct HubSettings: View {
    @ObservedObject var model: ShellModel
    @State private var pairing = false
    @State private var confirmSignOut = false
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
            Text(model.hubStatus).fontWeight(.semibold).accessibilityIdentifier("hub.status")
            #if DEBUG
            Text("Development identity — local unencrypted file, separate from production Keychain. Use only test routers.")
                .accessibilityIdentifier("hub.development-warning")
            #endif
            if let name = model.hubConnection?.pairing?.name { Text(name).textSelection(.enabled) }
            if let device = model.hubConnection?.deviceID {
                DisclosureGroup("Device identity") {
                    Text(device).textSelection(.enabled).lineLimit(nil).accessibilityIdentifier("hub.device-id")
                    Button("Copy device ID") {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(device, forType: .string)
                    }
                }
            }
            if let error = model.hubConnection?.errorMessage {
                Text(error).modifier(SoftmaxErrorText())
            }
            if model.hubConnection?.pairing != nil {
                if model.hubConnection?.state != .connected {
                    Button("Reconnect router") { model.hubConnection?.retryNow() }
                }
            } else {
                Text("Pair this device using a code or QR image from your router. You can finish setup without pairing.")
                Button("Pair router…") { model.configureHub(); pairing = true }
                    .disabled(model.hubConnection?.state == .connecting)
            }
            if model.hubConnection?.canSignOut == true {
                Button("Sign out…") { confirmSignOut = true }
            }
        }
        .sheet(isPresented: $pairing) { HubPairingSheet(model: model) }
        .confirmationDialog("Remove this device’s local identity and router enrollment?", isPresented: $confirmSignOut) {
            Button("Sign out", role: .destructive) { model.hubConnection?.signOut() }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A new device identity and a new invitation will be needed to pair again. This does not revoke the old device on the router.")
        }
    }
}

private struct HubPairingSheet: View {
    @ObservedObject var model: ShellModel
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var manual = false
    @State private var name = ""
    @State private var node = ""
    @State private var address = ""
    @State private var relay = ""
    @State private var invite = ""
    @State private var error: String?
    private var configuration: HubPairing? {
        if !manual { return try? HubPairing.parse(code) }
        var fields: [String: Any] = ["name": name, "hub": node,
            "addresses": address.split(separator: ",").map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }]
        if !relay.isEmpty { fields["relay"] = relay }
        if !invite.isEmpty { fields["invite"] = invite }
        guard let data = try? JSONSerialization.data(withJSONObject: fields) else { return nil }
        return try? HubPairing.parse(String(decoding: data, as: UTF8.self))
    }
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
            Text("Pair router").softmaxFont(SoftmaxDesignTokens.fontTitle, weight: .semibold)
            ScrollView {
                VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
                    Text("Copy this device’s public ID in Settings and use it when creating the router invitation. Paste the local pairing envelope or import its QR image; no lookup service is contacted.")
                    SecureField("Pairing code", text: $code).disabled(manual)
                    Button("Import QR image…", action: importQR).disabled(manual)
                    DisclosureGroup("Developer: manual router address", isExpanded: $manual) {
                        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
                            TextField("Router name", text: $name)
                            TextField("Pinned router node ID (64 lowercase hex)", text: $node)
                            TextField("IP:port addresses, comma-separated", text: $address)
                            TextField("HTTPS relay URL (optional)", text: $relay)
                            SecureField("Invitation (64 lowercase hex, optional for enrolled device)", text: $invite)
                        }.padding(.top, SoftmaxDesignTokens.spacing[1])
                    }
                    if let error { Text(error).modifier(SoftmaxErrorText()) }
                    if !model.testing && model.hubConnection?.deviceID == nil {
                        Text("Preparing device identity… Close this sheet and retry if the router helper is unavailable.")
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Button("Cancel") { clearSecrets(); dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                Button("Connect router", action: connect)
                    .disabled(configuration == nil || (!model.testing && model.hubConnection?.deviceID == nil))
                    .keyboardShortcut(.defaultAction)
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(width: 500, height: 440)
            .modifier(SoftmaxTheme())
            .onDisappear { clearSecrets() }
    }
    private func clearSecrets() { code = ""; invite = "" }
    private func connect() {
        guard let configuration else { error = "Check the pairing envelope or pinned router address."; return }
        guard !model.testing, let connection = model.hubConnection else {
            error = "Pairing is disabled in isolated previews and UI tests."; return
        }
        do {
            try connection.enroll(configuration)
            // The summary owns connecting/offline/error state. Never retain a
            // second copy of the invitation in a dismissed view or preferences.
            clearSecrets(); dismiss()
        } catch { self.error = "Could not begin pairing. Check the invitation’s device ID and router configuration." }
    }
    private func importQR() {
        let panel = NSOpenPanel()
        panel.allowedContentTypes = [.png, .jpeg, .heic, .tiff]
        panel.allowsMultipleSelection = false
        guard panel.runModal() == .OK, let url = panel.url else { return }
        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            guard let size = attributes[.size] as? NSNumber, size.intValue <= 10 * 1024 * 1024,
                  let image = CIImage(contentsOf: url),
                  let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil,
                                           options: [CIDetectorAccuracy: CIDetectorAccuracyHigh]) else { throw HubError.pairing }
            let values = detector.features(in: image).compactMap { ($0 as? CIQRCodeFeature)?.messageString }
            guard values.count == 1, let value = values.first, (try? HubPairing.parse(value)) != nil else { throw HubError.pairing }
            code = value; error = nil
        } catch { self.error = "Choose an image containing one valid router pairing QR code." }
    }
}
