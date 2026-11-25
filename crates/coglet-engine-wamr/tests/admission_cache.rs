#![cfg(feature = "interpreter")]
use coglet_engine::{Engine, Error, Limits};
use coglet_engine_wamr::{COMPUTE_MODE, Wamr};
use std::time::{Duration, Instant};
fn limits() -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 1000,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        deadline: Instant::now() + Duration::from_secs(1),
    }
}
fn guest(extra: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module {extra} (memory (export "memory") 1 2)
      (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#
    ))
    .unwrap()
}
#[test]
fn admitted_module_rechecks_changed_memory_and_import_policy() {
    let engine = Wamr::interpreter();
    let module = engine.load(&guest(""), &limits()).unwrap();
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
            &guest(r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#),
            &limits(),
        )
        .unwrap();
    assert!(matches!(
        engine.instantiate(&db, limits()),
        Err(Error::UnqualifiedModule)
    ));
}
#[test]
fn admitted_module_never_caches_dynamic_limit_or_deadline_checks() {
    let engine = Wamr::interpreter();
    let module = engine.load(&guest(""), &limits()).unwrap();
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
}
