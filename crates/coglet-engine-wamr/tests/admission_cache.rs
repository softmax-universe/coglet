#![cfg(feature = "interpreter")]
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::{COMPUTE_MODE, Wamr};
use std::time::{Duration, Instant};
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
        panic!("unexpected host import")
    }
}
fn limits() -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 1000,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn input() -> StepInput {
    StepInput {
        events: vec![0x81, 0xa0],
        step: 0,
        logical_time: 0,
        seed: 0,
    }
}
fn guest(extra: &str, handle: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module {extra} (memory (export "memory") 1 2)
      (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {handle}))"#
    ))
    .unwrap()
}
const NOOP: &str = "i32.const 0";
#[test]
fn admitted_module_rechecks_changed_memory_and_import_policy() {
    let engine = Wamr::interpreter();
    let module = engine.load(&guest("", NOOP), &limits()).unwrap();
    assert!(engine.instantiate(&module, limits()).is_ok());
    let mut lower = limits();
    lower.memory_pages = 1;
    assert!(matches!(
        engine.instantiate(&module, lower),
        Err(Error::MemoryExhausted)
    ));
    let db = Wamr::interpreter()
        .allow_db()
        .load(
            &guest(
                r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#,
                NOOP,
            ),
            &limits(),
        )
        .unwrap();
    assert!(
        Wamr::interpreter()
            .allow_db()
            .instantiate(&db, limits())
            .is_ok()
    );
    assert!(matches!(
        engine.instantiate(&db, limits()),
        Err(Error::UnqualifiedModule)
    ));
}
#[test]
fn admitted_module_rechecks_changed_runtime_byte_cap() {
    let engine = Wamr::interpreter();
    // ~400 KiB passive data segment: admitted under 8 MiB (cap/4 = 2 MiB) only.
    let big = guest(&format!(r#"(data "{}")"#, "a".repeat(400 * 1024)), NOOP);
    let module = engine.load(&big, &limits()).unwrap();
    assert!(engine.instantiate(&module, limits()).is_ok());
    let mut small = limits();
    small.runtime_bytes = 1024 * 1024;
    assert!(matches!(
        engine.instantiate(&module, small.clone()),
        Err(Error::MemoryExhausted)
    ));
    assert!(matches!(
        engine.load(&big, &small),
        Err(Error::MemoryExhausted)
    ));
    // Failure does not poison the cached proof for the original policy.
    assert!(engine.instantiate(&module, limits()).is_ok());
}
#[test]
fn different_code_never_inherits_another_modules_proof() {
    let engine = Wamr::interpreter();
    let good = engine.load(&guest("", NOOP), &limits()).unwrap();
    assert!(engine.instantiate(&good, limits()).is_ok());
    let bad = guest(r#"(import "env" "x" (func))"#, NOOP);
    assert!(matches!(
        engine.load(&bad, &limits()),
        Err(Error::UnqualifiedModule)
    ));
    let other = engine.load(&guest("", "i32.const 1"), &limits()).unwrap();
    let out = engine
        .instantiate(&other, limits())
        .unwrap()
        .handle(input(), &mut NoHost);
    assert_ne!(out.result, Ok(()));
    let out = engine
        .instantiate(&good, limits())
        .unwrap()
        .handle(input(), &mut NoHost);
    assert_eq!(out.result, Ok(()));
}
#[test]
fn same_proof_fresh_instance_resets_guest_memory_and_globals() {
    let engine = Wamr::interpreter();
    let handle = "global.get $g i32.const 1 i32.add global.set $g \
        global.get $g i32.const 1 i32.ne if unreachable end \
        i32.const 8 i32.load8_u if unreachable end \
        i32.const 8 i32.const 1 i32.store8 i32.const 0";
    let module = engine
        .load(
            &guest("(global $g (mut i32) (i32.const 0))", handle),
            &limits(),
        )
        .unwrap();
    for _ in 0..3 {
        let out = engine
            .instantiate(&module, limits())
            .unwrap()
            .handle(input(), &mut NoHost);
        assert_eq!(out.result, Ok(()));
    }
}
#[test]
fn admitted_module_never_caches_dynamic_limit_mode_or_deadline_checks() {
    let engine = Wamr::interpreter();
    let module = engine.load(&guest("", NOOP), &limits()).unwrap();
    let mut zero = limits();
    zero.compute = 0;
    assert!(matches!(
        engine.instantiate(&module, zero),
        Err(Error::InvalidArgument)
    ));
    let mut wrong = limits();
    wrong.compute_mode = "other".into();
    assert!(matches!(
        engine.instantiate(&module, wrong),
        Err(Error::UnsupportedMode)
    ));
    let mut expired = limits();
    expired.deadline = Instant::now();
    assert!(matches!(
        engine.instantiate(&module, expired),
        Err(Error::DeadlineExceeded)
    ));
    assert!(matches!(
        Wamr::aot().instantiate(&module, limits()),
        Err(Error::UnsupportedMode)
    ));
    // A deadline that expires after instantiate is enforced at handle.
    let mut soon = limits();
    soon.deadline = Instant::now() + Duration::from_millis(50);
    let instance = engine.instantiate(&module, soon).unwrap();
    std::thread::sleep(Duration::from_millis(80));
    assert_eq!(
        instance.handle(input(), &mut NoHost).result,
        Err(Error::DeadlineExceeded)
    );
}
#[test]
fn same_proof_enforces_each_instances_compute_budget() {
    let engine = Wamr::interpreter();
    let module = engine
        .load(&guest("", "(loop $l br $l) i32.const 0"), &limits())
        .unwrap();
    for budget in [100, 1000, 100] {
        let mut l = limits();
        l.compute = budget;
        let out = engine
            .instantiate(&module, l)
            .unwrap()
            .handle(input(), &mut NoHost);
        assert_eq!(out.result, Err(Error::ComputeExhausted));
        assert_eq!(out.compute_used, budget);
    }
}
