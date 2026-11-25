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
                        route: route, failure: model.appControlFailure,
                        userFailure: { model.message = "Could not complete router reconnection." }) { context in
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
        .sheet(isPresented: $confirmSignOut) {
            HubSignOutSheet(model: model)
        }
    }
}

/// App-owned confirmation mounts the real visible controls; no system-dialog
/// proxy or separate automation implementation.
private struct HubSignOutSheet: View {
    @ObservedObject var model: ShellModel
    @Environment(\.dismiss) private var dismiss
    @State private var failed = false
    var body: some View {
        VStack(alignment: .leading, spacing: SoftmaxDesignTokens.spacing[3]) {
            Text("Remove this device’s local identity and router enrollment?").fontWeight(.semibold)
            Text("A new device identity and a new invitation will be needed to pair again. This does not revoke the old device on the router.")
            if failed { Text("Sign out could not remove local identity or hub data. Try again.").modifier(SoftmaxErrorText()) }
            HStack {
                CogletButton("Cancel", id: "hub.sign_out.cancel", registry: model.appControls,
                    window: "sign_out_confirmation", route: "sign_out", failure: model.appControlFailure) { dismiss() }
                    .keyboardShortcut(.cancelAction)
                Spacer()
                CogletAsyncButton("Sign out", id: "hub.sign_out.confirm", registry: model.appControls,
                    window: "sign_out_confirmation", route: "sign_out", sensitive: true,
                    failure: model.appControlFailure, userFailure: { failed = true }) { context in
                        let command = {
                            guard let connection = model.hubConnection else { throw HubError.unavailable }
                            try connection.signOutOnce()
                            dismiss()
                        }
                        do { if let context { try context.perform(command) } else { try command() } }
                        catch {
                            if let context { try? context.perform { failed = true } } else { failed = true }
                            throw error
                        }
                    }
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(width: 440).modifier(SoftmaxTheme())
            .onAppear { model.signOutConfirmationOpen = true }
            .onDisappear { model.signOutConfirmationOpen = false }
            .background(AppSheetOwner { old, new in
                if new != nil || model.signOutNativeWindow === old {
                    model.signOutNativeWindow = new
                    model.synchronizeAppSurface?()
                }
            })
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
                CogletAsyncButton("Connect router", id: "\(idPrefix).pairing.connect", registry: model.appControls, window: window,
                    route: route, sensitive: true, failure: model.appControlFailure,
                    action: connect)
                    .disabled(configuration == nil || (!model.testing && model.hubConnection?.deviceID == nil))
                    .keyboardShortcut(.defaultAction)
            }
        }.padding(SoftmaxDesignTokens.spacing[4]).frame(width: 500, height: 440)
            .modifier(SoftmaxTheme())
            .onAppear { model.hubPairingRoute = route }
            .onChange(of: manual) { _, _ in model.hubPairingRoute = route }
            .onDisappear { clearSecrets(); model.hubPairingRoute = nil }
            .background(AppSheetOwner { old, new in
                if new != nil || model.hubPairingNativeWindow === old {
                    model.hubPairingNativeWindow = new
                    model.synchronizeAppSurface?()
                }
            })
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
    @MainActor private func connect(context: ControlRegistry.EffectContext?) async throws {
        guard let configuration else { throw HubError.pairing }
        guard !model.testing, let connection = model.hubConnection else {
            throw HubError.unavailable
        }
        do {
            try await connection.enrollOnce(configuration, authorized: { if let context { try context.perform {} } })
            // Do not dismiss (and retire the mount) until actual persisted
            // completion. The same command is used for user and adapter presses.
            if let context { try context.perform { clearSecrets(); dismiss() } }
            else { clearSecrets(); dismiss() }
        } catch {
            let showError = { self.error = "Could not complete pairing. Check the invitation’s device ID and router configuration." }
            if let context { try? context.perform(showError) } else { showError() }
            throw error
        }
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

/// Captures only this view's actual AppKit sheet. It never dismisses a foreign
/// sheet or treats all native modals as qualified merely because a flag is set.
private struct AppSheetOwner: NSViewRepresentable {
    let changed: (NSWindow?, NSWindow?) -> Void
    final class Probe: NSView {
        var changed: ((NSWindow?, NSWindow?) -> Void)?
        weak var previous: NSWindow?
        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            publish(window)
        }
        func publish(_ next: NSWindow?) {
            guard previous !== next else { return }
            let old = previous
            previous = next
            changed?(old, next)
        }
    }
    func makeNSView(context: Context) -> Probe {
        let view = Probe()
        view.changed = changed
        return view
    }
    func updateNSView(_ view: Probe, context: Context) {
        view.changed = changed
        view.publish(view.window)
    }
    static func dismantleNSView(_ view: Probe, coordinator: ()) {
        view.publish(nil)
        view.changed = nil
    }
}

private extension View {
    /// All pairing fields are identity-pairing input: sensitive always, secure for secrets.
    @MainActor func pairingField(_ id: String, _ label: String, _ text: Binding<String>, secure: Bool, sheet: HubPairingSheet) -> some View {
        sheet.registerField(self, id, label, text, secure: secure)
    }
}
