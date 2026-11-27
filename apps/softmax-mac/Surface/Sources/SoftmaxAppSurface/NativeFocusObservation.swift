#if canImport(AppKit)
import AppKit

/// Structural supersession only: retain app-local focus unless a resolved owner
/// has an actually visible attached sheet. Unknown ownership must not hide a
/// conflicting witness. This does not grant interaction or qualify a modal.
@MainActor
public enum NativeFocusObservation {
    public static func isEligible(owner: NSWindow?) -> Bool {
        owner?.attachedSheet?.isVisible != true
    }
}
#endif
