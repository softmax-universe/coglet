use super::*;
use coglet_engine::{Engine, Instance};
use std::time::Duration;
unsafe extern "C" {
    fn ce_owner_id() -> u64;
    fn ce_owner_loads() -> u64;
    fn ce_owner_failure_stage() -> u32;
}
// Each test owns the process-global native runtime, including its observations.
static TEST: Mutex<()> = Mutex::new(());
type ExitHook = Option<(u8, Box<dyn FnOnce()>)>;
thread_local! { static EXIT_HOOK: RefCell<ExitHook> = const { RefCell::new(None) }; }
pub(super) fn exit_hook(phase: u8) {
    let hook = EXIT_HOOK.with(|h| {
        let mut h = h.borrow_mut();
        if h.as_ref().is_some_and(|(p, _)| *p == phase) {
            h.take()
        } else {
            None
        }
    });
    if let Some((_, hook)) = hook {
        hook();
    }
}
fn limits() -> Limits {
    Limits {
        compute_mode: crate::COMPUTE_MODE.into(),
        compute: 10000,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn guest(extra: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module {extra} (memory (export "memory") 1 2)
    (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body}))"#
    ))
    .unwrap()
}
fn module() -> crate::Module {
    crate::Wamr::interpreter()
        .load(&guest("", "i32.const 0"), &limits())
        .unwrap()
}
struct NoHost;
impl Host for NoHost {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        _: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        panic!("unexpected import")
    }
}
fn input() -> StepInput {
    StepInput {
        events: vec![0x80],
        step: 0,
        logical_time: 0,
        seed: 0,
    }
}
fn run(module: &crate::Module) {
    run_limits(module, limits(), false);
}
fn run_limits(module: &crate::Module, limits: Limits, db: bool) {
    let engine = if db {
        crate::Wamr::interpreter().allow_db()
    } else {
        crate::Wamr::interpreter()
    };
    assert_eq!(
        engine
            .instantiate(module, limits)
            .unwrap()
            .handle(input(), &mut NoHost)
            .result,
        Ok(())
    );
}
fn id() -> u64 {
    let _g = RuntimeGuard::enter().unwrap();
    unsafe { ce_owner_id() }
}
fn loads() -> u64 {
    let _g = RuntimeGuard::enter().unwrap();
    unsafe { ce_owner_loads() }
}
#[test]
fn retained_owner_final_release_idle_clone_last_unexecuted_instance() {
    let _test = TEST.lock().unwrap();
    let engine = crate::Wamr::interpreter();
    let module = module();
    run(&module);
    assert_ne!(id(), 0, "loaded owner must survive a step");
    let count = loads();
    run(&module);
    assert_eq!(loads(), count);
    let clone = module.clone();
    let unexecuted = engine.instantiate(&module, limits()).unwrap();
    drop(module);
    drop(clone);
    assert_ne!(id(), 0);
    drop(unexecuted);
    // Raw observation before guard entry: entry cleanup must not mask a leak.
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
#[test]
fn stale_a_release_after_b_replacement_a_b_a_equal_bytes_distinct_id() {
    let _test = TEST.lock().unwrap();
    let a = module();
    let b = module();
    assert_ne!(a.lease.id, b.lease.id);
    let start = loads();
    for m in [&a, &b, &a, &b] {
        run(m);
        assert_eq!(id(), m.lease.id);
    }
    assert_eq!(loads(), start + 4);
    drop(a);
    assert_eq!(id(), b.lease.id);
    // Generation-matched stale native release cannot retire the new owner.
    let _guard = RuntimeGuard::enter().unwrap();
    unsafe {
        ce_release_owner(b.lease.id - 1);
    }
    assert_eq!(unsafe { ce_owner_id() }, b.lease.id);
    drop(_guard);
    drop(b);
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
#[test]
fn fresh_globals_memory_tables() {
    let _test = TEST.lock().unwrap();
    let bytes = guest(
        r#"(global $g (mut i32) (i32.const 0))
        (table 1 2 funcref) (func $f) (elem (i32.const 0) $f)"#,
        "global.get $g if unreachable end i32.const 1 global.set $g \
        i32.const 8 i32.load8_u if unreachable end i32.const 8 i32.const 1 i32.store8 \
        i32.const 0 table.get ref.is_null if unreachable end \
        i32.const 0 ref.null func table.set i32.const 0",
    );
    let m = crate::Wamr::interpreter().load(&bytes, &limits()).unwrap();
    let n = loads();
    for _ in 0..3 {
        run(&m);
    }
    assert_eq!(loads(), n + 1);
}
#[test]
fn runtime_page_stack_import_changes() {
    let _test = TEST.lock().unwrap();
    let m = module();
    run(&m);
    let count = loads();
    let mut l = limits();
    l.memory_pages = 3;
    l.stack_bytes *= 2;
    run_limits(&m, l, false);
    assert_eq!(loads(), count);
    let mut l = limits();
    l.runtime_bytes *= 2;
    run_limits(&m, l, false);
    assert_eq!(loads(), count + 1);
    run_limits(&m, limits(), true);
    assert_eq!(loads(), count + 2);
    run(&m);
    assert_eq!(loads(), count + 3);
}
#[test]
fn final_drop_around_unlock_no_idle_retention() {
    let _test = TEST.lock().unwrap();
    for phase in 0..3 {
        let m = module();
        run(&m);
        let guard = RuntimeGuard::enter().unwrap();
        EXIT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some((
                phase,
                Box::new(move || {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let thread = std::thread::spawn(move || {
                        drop(m);
                        tx.send(()).unwrap();
                    });
                    rx.recv_timeout(Duration::from_secs(2))
                        .expect("lease drop blocked on runtime");
                    thread.join().unwrap();
                }),
            ))
        });
        drop(guard);
        assert_eq!(
            unsafe { ce_owner_id() },
            0,
            "phase {phase} retained idle owner"
        );
    }
}
#[test]
fn postunlock_contender_reclaims_final_drop() {
    let _test = TEST.lock().unwrap();
    let m = module();
    run(&m);
    let guard = RuntimeGuard::enter().unwrap();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let (held_tx, held_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    EXIT_HOOK.with(|h| {
        *h.borrow_mut() = Some((
            2,
            Box::new(move || {
                std::thread::spawn(move || {
                    let g = RuntimeGuard::enter().unwrap();
                    held_tx.send(()).unwrap();
                    release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                    drop(g);
                    done_tx.send(()).unwrap();
                });
                held_rx.recv_timeout(Duration::from_secs(2)).unwrap();
                drop(m); // final drop and old guard's post-unlock sweep both lose.
            }),
        ))
    });
    drop(guard);
    release_tx.send(()).unwrap();
    done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
struct Callback {
    unrelated: Option<crate::Module>,
    threaded: bool,
    nested: Option<crate::Module>,
}
impl Host for Callback {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        _: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        if let Some(m) = self.unrelated.take() {
            if self.threaded {
                let (tx, rx) = std::sync::mpsc::channel();
                let t = std::thread::spawn(move || {
                    drop(m);
                    tx.send(()).unwrap();
                });
                rx.recv_timeout(Duration::from_secs(2))
                    .expect("callback drop deadlocked");
                t.join().unwrap();
            } else {
                drop(m);
            }
        }
        if let Some(m) = self.nested.take() {
            let out = crate::Wamr::interpreter()
                .instantiate(&m, limits())
                .unwrap()
                .handle(input(), &mut NoHost);
            assert_eq!(
                out.result,
                Err(Error::Trap(
                    "nested native execution is not supported".into()
                ))
            );
        }
        Ok(0)
    }
}
#[test]
fn callback_unrelated_lastdrop_cross_thread_dropjoin_nested_refusal() {
    let _test = TEST.lock().unwrap();
    let engine = crate::Wamr::interpreter();
    let m = engine
        .load(
            &guest(
                r#"(import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))"#,
                "i32.const 0 i32.const 0 call $log",
            ),
            &limits(),
        )
        .unwrap();
    for threaded in [false, true] {
        let unrelated = module();
        run(&unrelated);
        let mut h = Callback {
            unrelated: Some(unrelated),
            threaded,
            nested: Some(module()),
        };
        assert_eq!(
            engine
                .instantiate(&m, limits())
                .unwrap()
                .handle(input(), &mut h)
                .result,
            Ok(())
        );
        assert_eq!(id(), m.lease.id);
    }
}
#[test]
fn concurrent_callers() {
    let _test = TEST.lock().unwrap();
    let m = module();
    let n = loads();
    std::thread::scope(|s| {
        for _ in 0..8 {
            let m = &m;
            s.spawn(move || {
                for _ in 0..5 {
                    run(m);
                }
            });
        }
    });
    assert_eq!(loads(), n + 1);
    drop(m);
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
#[test]
fn malformed_load_instance_env_failures_recover_runtimecapacity_buffer_boundary() {
    let _test = TEST.lock().unwrap();
    let m = module();
    run(&m);
    let guard = RuntimeGuard::enter().unwrap();
    // These calls bypass admission deliberately to exercise the native boundary.
    let create = |bytes: &[u8], cap, stack, pages| unsafe {
        ce_create(
            m.lease.id,
            bytes.as_ptr(),
            bytes.len() as u32,
            cap,
            stack,
            pages,
            0,
        )
    };
    let bytes = guest("", "i32.const 0");
    for cap in [
        0,
        (2 * bytes.len()) as u32 - 1,
        (2 * bytes.len()) as u32,
        (2 * bytes.len()) as u32 + 1023,
    ] {
        assert!(create(&bytes, cap, 65536, 2).is_null());
        assert_eq!(unsafe { ce_owner_id() }, 0);
    }
    assert!(create(b"malformed", limits().runtime_bytes, 65536, 2).is_null());
    assert_eq!(unsafe { ce_owner_failure_stage() }, 1);
    assert_eq!(unsafe { ce_owner_id() }, 0);
    // A large table fails instantiation, not module loading. Linear memory may
    // use platform virtual memory outside the WAMR allocator pool.
    let large = bytes_for_large_memory();
    assert!(create(&large, 1024 * 1024, 65536, 64).is_null());
    assert_eq!(unsafe { ce_owner_failure_stage() }, 2);
    assert_eq!(unsafe { ce_owner_id() }, 0);
    // Instantiation succeeds, then an oversized execution stack exhausts pool.
    assert!(create(&bytes, 1024 * 1024, 2 * 1024 * 1024, 2).is_null());
    assert_eq!(unsafe { ce_owner_failure_stage() }, 3);
    assert_eq!(unsafe { ce_owner_id() }, 0);
    let ctx = create(&bytes, limits().runtime_bytes, 65536, 2);
    assert!(!ctx.is_null());
    unsafe {
        ce_destroy(ctx);
    }
    drop(guard);
    run(&m);
    drop(m);
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
fn bytes_for_large_memory() -> Vec<u8> {
    wat::parse_str("(module (memory 1) (table 1000000 funcref))").unwrap()
}

#[test]
fn nonblocking_cleanup_holder_final_drop_around_unlock() {
    let _test = TEST.lock().unwrap();
    for phase in 3..6 {
        let m = module();
        run(&m);
        let unrelated = module();
        EXIT_HOOK.with(|hook| {
            *hook.borrow_mut() = Some((
                phase,
                Box::new(move || {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let thread = std::thread::spawn(move || {
                        drop(m);
                        tx.send(()).unwrap();
                    });
                    rx.recv_timeout(Duration::from_secs(2))
                        .expect("cleanup holder blocked final drop");
                    thread.join().unwrap();
                }),
            ))
        });
        drop(unrelated);
        assert_eq!(unsafe { ce_owner_id() }, 0, "cleanup phase {phase} leaked");
    }
}

#[test]
fn poisoned_execution_fails_closed_final_drop_still_reclaims() {
    let _test = TEST.lock().unwrap();
    let m = module();
    run(&m);
    let panic = std::thread::spawn(|| {
        let _g = RuntimeGuard::enter().unwrap();
        panic!("deliberate runtime poisoning");
    })
    .join();
    assert!(panic.is_err());
    let instance = crate::Wamr::interpreter()
        .instantiate(&m, limits())
        .unwrap();
    assert_eq!(
        instance.handle(input(), &mut NoHost).result,
        Err(Error::UnqualifiedModule)
    );
    drop(m);
    assert_eq!(unsafe { ce_owner_id() }, 0);
    RUNTIME.clear_poison(); // Test isolation only; production remains fail-closed.
    run(&module());
}

#[test]
fn original_bytes_checked_even_for_same_lease_id() {
    let _test = TEST.lock().unwrap();
    let m = module();
    run(&m);
    let _g = RuntimeGuard::enter().unwrap();
    let n = unsafe { ce_owner_loads() };
    let changed = guest("", "i32.const 1");
    let ctx = unsafe {
        ce_create(
            m.lease.id,
            changed.as_ptr(),
            changed.len() as u32,
            limits().runtime_bytes,
            65536,
            2,
            0,
        )
    };
    assert!(!ctx.is_null());
    assert_eq!(unsafe { ce_owner_loads() }, n + 1);
    unsafe {
        ce_destroy(ctx);
    }
    drop(_g);
    run(&m);
    assert_eq!(loads(), n + 2);
}

#[test]
fn callback_panic_caught_owner_recovers_without_poison() {
    let _test = TEST.lock().unwrap();
    let engine = crate::Wamr::interpreter();
    let m = engine
        .load(
            &guest(
                r#"(import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))"#,
                "i32.const 0 i32.const 0 call $log",
            ),
            &limits(),
        )
        .unwrap();
    assert_eq!(
        engine
            .instantiate(&m, limits())
            .unwrap()
            .handle(input(), &mut NoHost)
            .result,
        Err(Error::Trap("host panicked".into()))
    );
    let n = loads();
    let mut host = Callback {
        unrelated: None,
        threaded: false,
        nested: None,
    };
    assert_eq!(
        engine
            .instantiate(&m, limits())
            .unwrap()
            .handle(input(), &mut host)
            .result,
        Ok(())
    );
    assert_eq!(loads(), n);
    drop(m);
    assert_eq!(unsafe { ce_owner_id() }, 0);
}
