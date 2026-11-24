//! Contract for the workspace disposable verification Cargo profile.
//! debug=0 / incremental=false must not turn off assertions or overflow checks.

const _: () = assert!(
    cfg!(debug_assertions),
    "dev/test verification must keep debug-assertions on"
);

#[test]
fn debug_assertions_remain_enabled() {
    let mut fired = false;
    debug_assert!({
        fired = true;
        true
    });
    assert!(fired, "debug_assert! must execute under this profile");
}

#[test]
fn overflow_checks_remain_enabled() {
    fn add(a: u8, b: u8) -> u8 {
        a + b
    }
    let exploded = std::panic::catch_unwind(|| {
        let _ = add(u8::MAX, 1);
    });
    assert!(
        exploded.is_err(),
        "dev/test overflow-checks must remain enabled"
    );
}
