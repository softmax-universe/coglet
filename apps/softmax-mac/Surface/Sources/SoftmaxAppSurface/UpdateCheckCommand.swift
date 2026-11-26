/// One native updater session, shared by user and adapter presses. Success means
/// Sparkle reported a check decision (found / no update), NEVER installation.
/// The session remains occupied until Sparkle ends its cycle, even if a decision
/// or cancellation already completed the caller. No native cancellation is implied.
@MainActor
public final class UpdateCheckCommand {
    private var active = false
    private var pending: CheckedContinuation<Void, Swift.Error>?
    public init() {}

    public func run(context: ControlRegistry.EffectContext?, available: @escaping @MainActor () -> Bool,
                    start: @escaping @MainActor () -> Bool) async throws {
        try await withCheckedThrowingContinuation { continuation in
            do {
                let initiate = {
                    try Task.checkCancellation()
                    guard !self.active, available() else { throw ControlRegistryFailure.unavailable }
                    self.active = true
                    self.pending = continuation
                    // Pinned Sparkle sets sessionInProgress synchronously. A
                    // refused/no-op initiation must not strand a continuation.
                    if !start() { self.finishedCycle() }
                }
                if let context { try context.perform(initiate) } else { try initiate() }
            } catch { continuation.resume(throwing: error) }
        }
        try Task.checkCancellation()
    }

    public func foundUpdate() { decide(.success(())) }
    public func noUpdateFound() { decide(.success(())) }
    public func aborted() { decide(.failure(ControlRegistryFailure.unavailable)) }
    public func finishedCycle() {
        // A normal cycle end alone is not evidence a check decision was reached.
        decide(.failure(ControlRegistryFailure.unavailable))
        active = false
    }
    private func decide(_ result: Result<Void, Swift.Error>) {
        guard active, let continuation = pending else { return }
        pending = nil
        continuation.resume(with: result)
    }
}
