#![cfg(feature = "interpreter")]
// Native qualification before any production admission path is enabled.
use coglet_engine_wamr as _;
use std::{
    ffi::{CString, c_char, c_void},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

unsafe extern "C" {
    fn ce_create(bytes: *mut u8, len: u32, pool: u32, stack: u32, pages: u32) -> *mut c_void;
    fn ce_call(ctx: *mut c_void, name: *const c_char, argc: u32, argv: *mut u32) -> i32;
    fn ce_set_budget(ctx: *mut c_void, budget: i32);
    fn ce_remaining(ctx: *mut c_void) -> i32;
    fn ce_terminate(ctx: *mut c_void);
    fn ce_destroy(ctx: *mut c_void);
}
static LOCK: Mutex<()> = Mutex::new(());
struct Native {
    ctx: *mut c_void,
    _bytes: Vec<u8>,
}
impl Native {
    fn new(wat: &str) -> Self {
        let mut bytes = wat::parse_str(wat).unwrap();
        let ctx = unsafe {
            ce_create(
                bytes.as_mut_ptr(),
                bytes.len() as u32,
                8 * 1024 * 1024,
                64 * 1024,
                2,
            )
        };
        assert!(!ctx.is_null());
        Self { ctx, _bytes: bytes }
    }
    fn call(&mut self, name: &str, arg: u32) -> (i32, u32) {
        let mut argv = [arg];
        let name = CString::new(name).unwrap();
        let ok = unsafe { ce_call(self.ctx, name.as_ptr(), 0, argv.as_mut_ptr()) };
        (ok, argv[0])
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { ce_destroy(self.ctx) }
    }
}
const GUEST: &str = r#"(module
 (memory (export "memory") 1 2)
 (func (export "init") nop nop)
 (func (export "alloc") (result i32) i32.const 0)
 (func (export "handle") (result i32) i32.const 42)
 (func (export "runaway") (loop $l br $l))
 (func (export "grow") (result i32) i32.const 1 memory.grow)
)"#;

#[test]
fn native_remaining_is_exact_across_export_calls() {
    let _lock = LOCK.lock().unwrap();
    let mut n = Native::new(GUEST);
    unsafe { ce_set_budget(n.ctx, 1000) };
    assert_eq!(n.call("init", 0).0, 1);
    let after_init = unsafe { ce_remaining(n.ctx) };
    assert!(
        after_init < 1000 && after_init > 0,
        "count must persist on return"
    );
    assert_eq!(n.call("alloc", 0), (1, 0));
    let after_alloc = unsafe { ce_remaining(n.ctx) };
    assert!(after_alloc < after_init);
    assert_eq!(n.call("handle", 0), (1, 42));
    assert!(unsafe { ce_remaining(n.ctx) } < after_alloc);
}

#[test]
fn native_runaway_budget_is_deterministic() {
    let _lock = LOCK.lock().unwrap();
    for _ in 0..5 {
        let mut n = Native::new(GUEST);
        unsafe { ce_set_budget(n.ctx, 1000) };
        assert_eq!(n.call("runaway", 0).0, 0);
        assert_eq!(unsafe { ce_remaining(n.ctx) }, 0);
    }
}

#[test]
fn native_runaway_deadline_then_fresh_recovery() {
    let _lock = LOCK.lock().unwrap();
    let mut n = Native::new(GUEST);
    unsafe { ce_set_budget(n.ctx, i32::MAX) };
    let address = n.ctx as usize;
    let cancellation = Arc::new((Mutex::new(false), Condvar::new()));
    let signal = cancellation.clone();
    let watchdog = std::thread::spawn(move || {
        let (lock, cv) = &*signal;
        let (done, timeout) = cv
            .wait_timeout_while(lock.lock().unwrap(), Duration::from_millis(10), |v| !*v)
            .unwrap();
        if timeout.timed_out() && !*done {
            unsafe { ce_terminate(address as *mut c_void) };
        }
    });
    assert_eq!(n.call("runaway", 0).0, 0);
    {
        let (lock, cv) = &*cancellation;
        *lock.lock().unwrap() = true;
        cv.notify_one();
    }
    watchdog.join().unwrap(); // Must happen BEFORE deinstantiation.
    drop(n);
    let mut next = Native::new(GUEST);
    unsafe { ce_set_budget(next.ctx, 1000) };
    assert_eq!(next.call("handle", 0), (1, 42));
}

#[test]
fn native_linear_memory_growth_limit_holds() {
    let _lock = LOCK.lock().unwrap();
    let mut n = Native::new(GUEST);
    unsafe { ce_set_budget(n.ctx, 1000) };
    assert_eq!(n.call("grow", 0), (1, 1));
    assert_eq!(n.call("grow", 0), (1, u32::MAX));
}
