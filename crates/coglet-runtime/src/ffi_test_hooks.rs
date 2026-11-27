//! Non-production qualification exports. Never included without ffi-test-hooks.
use super::*;
use timing::obstruction::PumpHold;

/// Install a per-link optional-pump hold and return independently owned control.
/// # Safety
/// The live link must have exclusive custody. Release/join its worker before
/// close. Install once per experiment; release is sticky, not re-armable. The
/// returned control must be freed exactly once after all users join.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_test_hold(link: *mut CogletSurfaceLink) -> *const PumpHold {
    if link.is_null() {
        return ptr::null();
    }
    let hold = Arc::new(PumpHold::default());
    unsafe { (&mut *link).hold_recv_pump(hold.clone()) };
    Arc::into_raw(hold)
}
/// # Safety
/// `hold` is a live control returned by test_hold, not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_test_release(hold: *const PumpHold) {
    if !hold.is_null() {
        unsafe { (&*hold).release() };
    }
}
/// # Safety
/// Same control custody as test_release. Does not access the surface handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_test_entries(hold: *const PumpHold) -> usize {
    if hold.is_null() {
        0
    } else {
        unsafe { (&*hold).entries() }
    }
}
/// # Safety
/// All control users and the ABI worker must be joined; free exactly once.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_test_free_hold(hold: *const PumpHold) {
    if !hold.is_null() {
        drop(unsafe { Arc::from_raw(hold) });
    }
}

/// Bounded JSON diagnostic capture, with ordinary recv size/copy conventions.
/// Snapshot does not call tick, pump, or refresh any protocol receipt clock.
/// Join the worker first. An in-flight call is recorded only at return/unwind;
/// the independent hold control, not this link snapshot, observes its entry.
/// # Safety
/// Same exclusive link/buffer custody and nonaliasing as coglet_surface_recv.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn coglet_surface_test_timings(
    link: *mut CogletSurfaceLink,
    out: *mut u8,
    cap: usize,
    n: *mut usize,
) -> i32 {
    guarded(|| {
        if link.is_null() || n.is_null() || (out.is_null() && cap != 0) {
            return -1;
        }
        let link = unsafe { &*link };
        let (recent, first_failure) = link.timings();
        let Ok(bytes) = serde_json::to_vec(&serde_json::json!({
            "recent":recent, "first_failure":first_failure,
            "clock_domain":"per-link monotonic microseconds", "adapter_fifo_depth":null
        })) else {
            return -2;
        };
        unsafe { n.write(bytes.len()) };
        if cap < bytes.len() {
            return 2;
        }
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), out, bytes.len()) };
        0
    })
}
