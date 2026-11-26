import Foundation
import CogletSurface

/// One negotiated product stream, confined to the same serialized owner as its
/// SDK session. Construct AFTER speech-token preparation (which stops backends),
/// before attaching native listeners or exposing any product payload.
public final class AppSurfaceConnection {
    public let incarnation: UInt64
    public let epoch: UInt64
    public let mailbox: AppSurfaceMailbox
    private let custody: UUID
    private let tokens: [String: ExtensionObservationToken]

    public init(session: SurfaceSession, mailbox: AppSurfaceMailbox, grant: EffectiveSurfaceGrant,
                operations: [SurfaceOperation], capture: AppSurfaceCapture) throws {
        self.mailbox = mailbox
        incarnation = try session.reserveSourceIncarnation()
        epoch = session.grantEpoch
        var opened: [String: ExtensionObservationToken] = [:]
        do {
            for operation in operations where operation.direction == .event && operation.name.hasPrefix("softmax.app.") {
                opened[operation.name] = try session.openExtensionObservationSession(owner: grant.coglet,
                    operation: operation.name, epoch: epoch)
            }
            // No native getter, disk force or subscription registration under the
            // mailbox gate. All SDK constructor/token safety stops precede this.
            try mailbox.activate(incarnation: incarnation, epoch: epoch, grant: grant, capture: capture)
        } catch {
            mailbox.stop()
            for token in opened.values { session.closeExtensionObservationSession(token) }
            throw error
        }
        tokens = opened; custody = session.egressCustody
    }
    public func validate(_ session: SurfaceSession) throws {
        guard session.egressCustody == custody, session.grantEpoch == epoch, !session.isKilled,
              mailbox.isLive(incarnation: incarnation, epoch: epoch),
              tokens.values.allSatisfy({ session.extensionObservationIsCurrent($0) }) else {
            mailbox.stop(); throw NativeSurfaceError("unavailable")
        }
    }
    public func publishEvents(_ session: SurfaceSession) throws {
        try validate(session)
        for event in mailbox.drainEvents() {
            guard let token = tokens[event.operation], try session.publishExtensionObservation(token, payload: event.payload) else {
                mailbox.stop(); throw NativeSurfaceError("permission_denied")
            }
            try validate(session)
        }
    }
    /// The send closure MUST perform one whole-frame attempt only; the embedded
    /// ABI copies all or none on would-block. Do not use with a partial writer.
    /// The permit linearizes emission admission, not completion of the ABI pump.
    public func emit<T>(session: SurfaceSession, _ attempt: () throws -> T) throws -> T {
        try validate(session)
        let permit = try mailbox.enterEmission(incarnation: incarnation, epoch: epoch)
        defer { mailbox.finishEmission(permit) }
        let result = try attempt()
        try validate(session)
        return result
    }
}
