import Foundation
import CogletSurface
import CogletTypes

/// A bounded callback mailbox. Native completion never enters the ABI or blocks
/// an audio callback; the runtime actor consumes receipts on its next poll.
final class RuntimeFaceOutput: @unchecked Sendable {
    struct Receipt {
        let action: RuntimeAction
        let result: NativeSurfaceResult
    }
    private let lock = NSLock()
    private var pending: [UUID: RuntimeAction] = [:]
    private var completed: [UUID: NativeSurfaceResult] = [:]
    private var enabled = false
    private let stopNative: @MainActor @Sendable () -> Void
    init(stopNative: @escaping @MainActor @Sendable () -> Void) { self.stopNative = stopNative }
    func preflight() throws {
        lock.lock(); defer { lock.unlock() }
        guard enabled else { throw NativeSurfaceError("cancelled") }
        guard pending.count < 16 else { throw NativeSurfaceError("resource_exhausted") }
    }
    func admit(_ action: RuntimeAction) throws {
        lock.lock(); defer { lock.unlock() }
        guard enabled else { throw NativeSurfaceError("cancelled") }
        guard pending.count < 16 else { throw NativeSurfaceError("resource_exhausted") }
        pending[action.ticket] = action
    }
    func isLive(_ action: RuntimeAction) -> Bool {
        lock.lock(); defer { lock.unlock() }
        return pending[action.ticket] != nil && completed[action.ticket] == nil
    }
    func complete(_ action: RuntimeAction, error: String?) {
        lock.lock(); defer { lock.unlock() }
        guard pending[action.ticket] != nil, completed[action.ticket] == nil else { return }
        completed[action.ticket] = error.map { .failed(NativeSurfaceError($0)) } ?? .succeeded(.map([]))
    }
    func drain() -> [Receipt] {
        lock.lock(); defer { lock.unlock() }
        let result = completed.compactMap { key, value in pending[key].map { Receipt(action: $0, result: value) } }
        for receipt in result { pending[receipt.action.ticket] = nil }
        completed.removeAll()
        return result
    }
    func stop() {
        lock.lock()
        for key in pending.keys where completed[key] == nil {
            completed[key] = .failed(NativeSurfaceError("cancelled"))
        }
        lock.unlock()
        // SDK stop is synchronous. RuntimeExecutor never runs ABI calls on the
        // main actor, and its callers await it rather than blocking the UI thread.
        if Thread.isMainThread { MainActor.assumeIsolated { stopNative() } }
        else { DispatchQueue.main.sync { MainActor.assumeIsolated { stopNative() } } }
    }
    func suspend() {
        lock.lock(); enabled = false; lock.unlock()
        stop()
    }
    func resume() { lock.lock(); enabled = true; lock.unlock() }
}

@MainActor final class RuntimeOutputStop {
    var callback: (() -> Void)?
    func stop() { callback?() }
}

/// SDK admission happens before a MESSAGE is offered to the product renderer.
/// nil keeps the durable action pending until a native played-back receipt.
final class PendingFaceSink: SurfaceNativeBackend {
    var actions: [RuntimeAction] = []
    let output: RuntimeFaceOutput
    init(output: RuntimeFaceOutput) { self.output = output }
    func preflight(action: CBORValue, now: UInt64) throws { try output.preflight() }
    func begin(action: CBORValue, epoch: UInt64, now: UInt64) throws -> NativeSurfaceResult? {
        let request = RuntimeAction(action: action, epoch: epoch)
        do { try output.admit(request) }
        catch let error as NativeSurfaceError { return .failed(error) }
        actions.append(request)
        return nil
    }
    func read(operation: String, args: CBORValue, owner: String, now: UInt64) throws -> CBORValue {
        throw NativeSurfaceError("unavailable")
    }
    func stop(owner: String?) { output.stop() }
    func reconcile(_ session: SurfaceSession) throws {
        for receipt in output.drain() {
            try session.complete(owner: receipt.action.owner, id: receipt.action.id,
                                 epoch: receipt.action.epoch, result: receipt.result)
        }
    }
}
