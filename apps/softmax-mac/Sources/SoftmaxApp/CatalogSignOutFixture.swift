#if DEBUG
import Foundation
import SoftmaxCore

/// No identity bytes, enrollment, persistence or deletion. Even an accidental
/// confirmation cannot mutate storage; all mutation entry points fail closed.
@MainActor
private final class CatalogSignOutErrorStore: HubStore {
    private(set) var identityLoads = 0
    private(set) var mutations = 0
    func loadKey() throws -> Data? { identityLoads += 1; throw HubError.storage }
    func loadHub() throws -> HubPairing? { nil }
    func saveKey(_ value: Data) throws { mutations += 1; throw HubError.storage }
    func deleteKey() throws { mutations += 1; throw HubError.storage }
    func saveHub(_ value: HubPairing?) throws { mutations += 1; throw HubError.storage }
}

/// Inert transport boundary: never launches a helper, opens a socket, retains
/// an event callback or emits a device identity. start/connect are forbidden.
@MainActor
private final class CatalogSignOutInertLink: HubLink {
    private(set) var starts = 0, connects = 0, stops = 0
    func start(key: Data, event: @escaping (HubLinkEvent) -> Void) throws {
        starts += 1; throw HubError.unavailable
    }
    func connect(_ pairing: HubPairing) throws { connects += 1; throw HubError.unavailable }
    func stop() { stops += 1 }
}

/// Safe catalog checkpoint prerequisite, not a synthetic sheet or authority.
/// The sole caller runs after the existing private catalog-directory validation.
/// BOTH DEBUG and isolated --ui-testing plus this closed selector are required.
@MainActor
final class CatalogSignOutFixture {
    static let flag = "--catalog-sign-out-startup-error"
    struct Witness: Encodable {
        let identityLoads: Int
        let storageMutations: Int
        let linkStarts: Int
        let linkConnects: Int
        let linkStops: Int
        let startupError: Bool
        let canSignOut: Bool
        let identityAbsent: Bool
        let pairingAbsent: Bool
        let offline: Bool
    }
    private let store: CatalogSignOutErrorStore
    private let link: CatalogSignOutInertLink
    private let connection: HubConnection

    private init(store: CatalogSignOutErrorStore, link: CatalogSignOutInertLink, connection: HubConnection) {
        self.store = store; self.link = link; self.connection = connection
    }

    static func install(into model: ShellModel) throws -> CatalogSignOutFixture {
        let arguments = ProcessInfo.processInfo.arguments
        guard model.testing, arguments.contains(flag),
              arguments.contains("--catalog-probe-directory"),
              !arguments.contains("--sdk-wire-proof-directory"),
              model.runtime.state == .stopped, model.hubConnection == nil else { throw HubError.unavailable }
        let store = CatalogSignOutErrorStore(), link = CatalogSignOutInertLink()
        let account = HubAccount(store: store, generate: { throw HubError.identity })
        let connection = HubConnection(account: account, link: link)
        // Exercise the real public startup failure. stop() synchronously cancels
        // its retry before the MainActor can yield; it preserves errorMessage.
        connection.start()
        connection.stop()
        guard store.identityLoads == 1, store.mutations == 0,
              link.starts == 0, link.connects == 0, link.stops == 2,
              connection.errorMessage != nil, connection.canSignOut,
              connection.deviceID == nil, connection.pairing == nil,
              connection.state == .offline else { throw HubError.unavailable }
        // Reuse the sole existing guarded non-production connection writer.
        // Never replace an existing connection, fabricate canSignOut or enroll.
        try model.installWireProofHubFixture(connection)
        return CatalogSignOutFixture(store: store, link: link, connection: connection)
    }

    /// Closed counters/Booleans only, through the existing nonce-bound probe.
    var witness: Witness {
        Witness(identityLoads: store.identityLoads, storageMutations: store.mutations,
            linkStarts: link.starts, linkConnects: link.connects, linkStops: link.stops,
            startupError: connection.errorMessage != nil, canSignOut: connection.canSignOut,
            identityAbsent: connection.deviceID == nil, pairingAbsent: connection.pairing == nil,
            offline: connection.state == .offline)
    }
}
#endif
