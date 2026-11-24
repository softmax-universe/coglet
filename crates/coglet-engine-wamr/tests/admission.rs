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
fn guest(extra: &str, memory: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module {extra} (memory (export "memory") {memory})
 (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
 (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#
    ))
    .unwrap()
}
#[test]
fn production_admission_rejects_ambient_and_wrong_signature_imports() {
    let engine = Wamr::interpreter();
    for name in ["clock_time_get", "random_get", "poll_oneoff"] {
        let bytes = guest(
            &format!(r#"(import "wasi_snapshot_preview1" "{name}" (func))"#),
            "1 2",
        );
        assert!(matches!(
            engine.load(&bytes, &limits()),
            Err(Error::UnqualifiedModule)
        ));
    }
    for name in ["timer", "emit", "llm", "log"] {
        assert!(
            engine
                .load(
                    &guest(
                        &format!(
                            r#"(import "coglet_v0" "{name}" (func (param i32 i32) (result i32)))"#
                        ),
                        "1 2"
                    ),
                    &limits()
                )
                .is_ok()
        );
        assert!(matches!(
            engine.load(
                &guest(
                    &format!(r#"(import "coglet_v0" "{name}" (func (param i32) (result i32)))"#),
                    "1 2"
                ),
                &limits()
            ),
            Err(Error::UnqualifiedModule)
        ));
    }
}
#[test]
fn both_initial_and_declared_maximum_memory_are_admitted_before_execution() {
    let engine = Wamr::interpreter();
    assert!(engine.load(&guest("", "2 2"), &limits()).is_ok());
    for memory in ["3 3", "1 3", "1"] {
        assert!(matches!(
            engine.load(&guest("", memory), &limits()),
            Err(Error::MemoryExhausted)
        ));
    }
}
#[test]
fn ffi_budget_zero_overflow_rejected() {
    let engine = Wamr::interpreter();
    let bytes = guest("", "1 2");
    for compute in [0, i32::MAX as u64 + 1, u64::MAX] {
        let mut l = limits();
        l.compute = compute;
        assert!(matches!(
            engine.load(&bytes, &l),
            Err(Error::InvalidArgument)
        ));
    }
    let mut l = limits();
    l.compute_mode = "unqualified".into();
    assert!(matches!(
        engine.load(&bytes, &l),
        Err(Error::UnsupportedMode)
    ));
}
#[test]
fn unqualified_aot_mode_rejected_before_execution() {
    let mut l = limits();
    l.compute_mode = "aot".into();
    assert!(matches!(
        Wamr::aot().load(&guest("", "1 2"), &l),
        Err(Error::UnsupportedMode)
    ));
}
#[test]
fn frozen_abi_rejects_db_import() {
    let engine = Wamr::interpreter();
    let bytes = guest(
        r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#,
        "1 2",
    );
    assert!(matches!(
        engine.load(&bytes, &limits()),
        Err(Error::UnqualifiedModule)
    ));
}
#[test]
fn abi02_opt_in_admits_exact_db_signature_only() {
    let engine = Wamr::interpreter().allow_db();
    assert!(
        engine
            .load(
                &guest(
                    r#"(import "coglet_v0" "db" (func (param i32 i32 i32 i32) (result i32)))"#,
                    "1 2"
                ),
                &limits()
            )
            .is_ok()
    );
    assert!(matches!(
        engine.load(
            &guest(
                r#"(import "coglet_v0" "db" (func (param i32 i32) (result i32)))"#,
                "1 2"
            ),
            &limits()
        ),
        Err(Error::UnqualifiedModule)
    ));
}
