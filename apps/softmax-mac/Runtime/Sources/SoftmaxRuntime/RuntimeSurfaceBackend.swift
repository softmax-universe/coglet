import CogletSurface
import CogletTypes
import class SoftmaxAppSurface.AppSurfaceBackend
import class SoftmaxAppSurface.AppSurfaceMailbox

/// Confined to RuntimeExecutor. Product actions NEVER enter RuntimeAction's
/// speech/Face decoding or its renderer queue. Both backends share SDK admission.
final class RuntimeSurfaceBackend: SurfaceNativeBackend {
    let face: PendingFaceSink
    let app: AppSurfaceBackend?
    init(face: PendingFaceSink, mailbox: AppSurfaceMailbox?) {
        self.face = face; app = mailbox.map(AppSurfaceBackend.init)
    }
    private func destination(_ operation: String) throws -> any SurfaceNativeBackend {
        if operation.hasPrefix("softmax.app.") {
            guard let app else { throw NativeSurfaceError("unavailable") }; return app
        }
        guard ["speech.speak_chunk", "expression.set_expression"].contains(operation) else {
            throw NativeSurfaceError("unavailable")
        }
        return face
    }
    func preflight(action: CBORValue, now: UInt64) throws {
        guard case .text(let name)? = action["capability"] else { throw NativeSurfaceError("invalid_argument") }
        try destination(name).preflight(action: action, now: now)
    }
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        guard case .text(let name)? = action["capability"] else { throw NativeSurfaceError("invalid_argument") }
        return try destination(name).begin(action: action, epoch: epoch, now: now)
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        try destination(operation).read(operation: operation, args: args, owner: owner, now: now)
    }
    func stop(owner: String?) { app?.stop(owner: owner); face.stop(owner: owner) }
    func reconcile(_ session: SurfaceSession) throws { try face.reconcile(session); try app?.reconcile(session) }
}
