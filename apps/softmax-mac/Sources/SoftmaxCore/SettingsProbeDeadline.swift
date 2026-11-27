#if DEBUG
/// Bounds for the opt-in Settings transition evidence producer, not product input timeouts.
public enum SettingsProbeDeadline {
    public static func expired(now: Double, start: Double, ticks: Int) -> Bool {
        // Reject non-monotonic/nonfinite inputs instead of admitting unbounded evidence.
        !now.isFinite || !start.isFinite || now < start || now - start >= 5 || ticks > 500
    }
}
#endif
