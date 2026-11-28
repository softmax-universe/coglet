//! Explicit input to the signed qualification fixture, never a runtime default.
// The retained original 100 ms archive failed before guest entry at 408399 us.
// Five seconds matches other native fixture budgets and leaves >12x that
// observed wall usage for mandatory host/WAL work. This is selected BEFORE
// source commit, profile pinning and signing, never patched into admitted bytes.
pub const WALL_TIME_US: u64 = 5_000_000;
