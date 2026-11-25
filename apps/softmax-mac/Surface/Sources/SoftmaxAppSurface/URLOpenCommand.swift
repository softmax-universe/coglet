import Foundation

/// Completion-bearing native URL initiation shared by the user and adapter.
/// Acceptance means the native OpenURLAction accepted the URL, not that another
/// application loaded it. This never grants authority over that application.
@MainActor
public struct URLOpenCommand {
    private let url: URL
    private let open: (URL, @escaping (Bool) -> Void) -> Void

    public init(url: URL, open: @escaping (URL, @escaping (Bool) -> Void) -> Void) {
        self.url = url; self.open = open
    }

    public func run(context: ControlRegistry.EffectContext?) async throws {
        let accepted: Bool = try await withCheckedThrowingContinuation { continuation in
            do {
                let initiate = {
                    open(url) { continuation.resume(returning: $0) }
                }
                if let context { try context.perform(initiate) } else { initiate() }
            } catch { continuation.resume(throwing: error) }
        }
        guard accepted else { throw ControlRegistryFailure.unavailable }
    }
}
