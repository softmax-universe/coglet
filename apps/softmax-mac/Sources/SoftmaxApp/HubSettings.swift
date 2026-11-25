import SwiftUI
import AppKit
import CoreImage
import UniformTypeIdentifiers
import SoftmaxCore
import SoftmaxAppSurface

/// Shared Configure surface; the host owns its existing group and navigation.
struct HubSettings: View {
    @ObservedObject var model: ShellModel
    /// Host window/route and id namespace; Settings defaults, onboarding overrides.
    var window = "settings"
    var route = "hub"
    var idPrefix = "settings.hub"
    @State private var pairing = false
    @State private var confirmSignOut = false
    @State private var identityExpanded = false
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
            Text(model.hubStatus).fontWeight(.semibold)
            if let name = model.hubConnection?.pairing?.name { Text(name).textSelection(.enabled) }
            if let device = model.hubConnection?.deviceID {
                DisclosureGroup("Device identity", isExpanded: $identityExpanded) {
                    Text(device).textSelection(.enabled).lineLimit(nil)
                    CogletButton("Copy device ID", id: "\(idPrefix).copy_device_id", registry: model.appControls,
                        window: window, route: route, failure: model.appControlFailure) {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(device, forType: .string)
                    }
                }
                .coglet(id: "\(idPrefix).device_identity", registry: model.appControls, window: window, route: route,
                    role: "disclosure", label: "Device identity", verbs: ["toggle"], value: {
                        .map([.init("kind", .text("boolean")), .init("value", .boolean(identityExpanded))])
                    }, failure: model.appControlFailure, invoke: { args in
                        guard case .map(let fields) = args,
                              let field = fields.first(where: { $0.key == "value" }), case .boolean(let value) = field.value else {
                            throw ControlRegistryFailure.unsupported
                        }
                        identityExpanded = value
                    })
            }
            if let error = model.hubConnection?.errorMessage {
                Text(error).modifier(SoftmaxErrorText())
            }
            if model.hubConnection?.pairing != nil {
                if model.hubConnection?.state != .connected {
                    CogletAsyncButton("Reconnect router", id: "\(idPrefix).reconnect", registry: model.appControls, window: window,
                        route: route, failure: model.appControlFailure) { context in
                            guard let connection = model.hubConnection else { throw ControlRegistryFailure.unavailable }
                            try await connection.reconnectOnce(authorized: { if let context { try context.perform {} } })
                        }
                }
            } else {
                Text("Pair this device using a code or QR image from your router. You can finish setup without pairing.")
                CogletButton("Pair router…", id: "\(idPrefix).pair", registry: model.appControls, window: window,
                        route: route, sensitive: true, failure: model.appControlFailure) { model.configureHub(); pairing = true }
                    .disabled(model.hubConnection?.state == .connecting)
            }
            if model.hubConnection?.canSignOut == true {
                CogletButton("Sign out…", id: "\(idPrefix).sign_out", registry: model.appControls, window: window,
                        route: route, sensitive: true, failure: model.appControlFailure) { confirmSignOut = true }
            }
        }
        .sheet(isPresented: $pairing) { HubPairingSheet(model: model, idPrefix: idPrefix) }
        // Limit: confirmationDialog buttons are system-presented alert/sheet items;
        // no CogletButton can be mounted in them, so "Sign out"/"Cancel" below are
        // intentionally NOT registered (the sensitive opener above is). No fake coverage.
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
    private var window: String { "hub_pairing" }
    private var route: String { manual ? "manual" : "code" }
    let idPrefix: String
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
                    SecureField("Pairing code", text: $code)
                        .pairingField("pairing.code", "Pairing code", $code, secure: true, sheet: self).disabled(manual)
                    CogletButton("Import QR image…", id: "\(idPrefix).pairing.import_qr", registry: model.appControls,
                        window: window, route: route, sensitive: true, adapterSynchronous: false, failure: model.appControlFailure, action: importQR)
                        .disabled(manual)
                    DisclosureGroup("Developer: manual router address", isExpanded: $manual) {
                        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[2]) {
                            TextField("Router name", text: $name)
                                .pairingField("pairing.router_name", "Router name", $name, secure: false, sheet: self)
                            TextField("Pinned router node ID (64 lowercase hex)", text: $node)
                                .pairingField("pairing.router_node", "Pinned router node ID", $node, secure: false, sheet: self)
                            TextField("IP:port addresses, comma-separated", text: $address)
                                .pairingField("pairing.router_addresses", "Router addresses", $address, secure: false, sheet: self)
                            TextField("HTTPS relay URL (optional)", text: $relay)
                                .pairingField("pairing.relay_url", "Relay URL", $relay, secure: false, sheet: self)
                            SecureField("Invitation (64 lowercase hex, optional for enrolled device)", text: $invite)
                                .pairingField("pairing.invitation", "Invitation", $invite, secure: true, sheet: self)
                        }.padding(.top, SoftmaxDesignTokens.spacing[1])
                    }
                    if let error { Text(error).modifier(SoftmaxErrorText()) }
                    if !model.testing && model.hubConnection?.deviceID == nil {
                        Text("Preparing device identity… Close this sheet and retry if the router helper is unavailable.")
                    }
                }.frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                CogletButton("Cancel", id: "\(idPrefix).pairing.cancel", registry: model.appControls, window: window,
                    route: route, failure: model.appControlFailure) { clearSecrets(); dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                CogletButton("Connect router", id: "\(idPrefix).pairing.connect", registry: model.appControls, window: window,
                    route: route, sensitive: true, adapterSynchronous: false, failure: model.appControlFailure, action: connect)
                    .disabled(configuration == nil || (!model.testing && model.hubConnection?.deviceID == nil))
                    .keyboardShortcut(.defaultAction)
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(width: 500, height: 440)
            .modifier(SoftmaxTheme())
            .onAppear { model.hubPairingRoute = route }
            .onChange(of: manual) { _, _ in model.hubPairingRoute = route }
            .onDisappear { clearSecrets(); model.hubPairingRoute = nil }
    }
    @MainActor fileprivate func registerField(_ view: some View, _ id: String, _ label: String, _ text: Binding<String>, secure: Bool) -> some View {
        view.coglet(id: "\(idPrefix).\(id)", registry: model.appControls, window: window, route: route,
            role: secure ? "secure_field" : "text_field", label: label, sensitive: true, secure: secure,
            verbs: ["type_text"], value: {
                // Secure fields never expose their binding to the registry.
                secure ? .map([.init("kind", .text("redacted"))])
                       : .map([.init("kind", .text("text")), .init("value", .text(text.wrappedValue))])
            }, failure: model.appControlFailure, invoke: { args in
                guard case .map(let fields) = args,
                      let field = fields.first(where: { $0.key == "value" }), case .text(let value) = field.value else {
                    throw ControlRegistryFailure.unsupported
                }
                text.wrappedValue = value
            })
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

private extension View {
    /// All pairing fields are identity-pairing input: sensitive always, secure for secrets.
    @MainActor func pairingField(_ id: String, _ label: String, _ text: Binding<String>, secure: Bool, sheet: HubPairingSheet) -> some View {
        sheet.registerField(self, id, label, text, secure: secure)
    }
}
