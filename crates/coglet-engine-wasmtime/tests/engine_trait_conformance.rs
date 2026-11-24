use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wasmtime::{COMPUTE_MODE, Wasmtime};
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
        panic!("unexpected import")
    }
}
fn limits(compute: u64) -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn guest(init: &str, alloc: &str, handle: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module (memory (export "memory") 1 2)
(func (export "_initialize") {init})
(func (export "abi_alloc") (param i32) (result i32) {alloc})
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {handle}))"#
    ))
    .unwrap()
}
fn run(b: &[u8], l: Limits) -> coglet_engine::StepOutcome {
    let m = Wasmtime::default().load(b, &l).unwrap();
    Wasmtime::default().instantiate(&m, l).unwrap().handle(
        StepInput {
            events: vec![0x81, 0xa0],
            step: 0,
            logical_time: 0,
            seed: 0,
        },
        &mut NoHost,
    )
}
#[test]
fn engine_trait_conformance() {
    assert_eq!(
        run(&guest("", "i32.const 0", "i32.const 0"), limits(1000)).result,
        Ok(())
    );
}
#[test]
fn cumulative_init_alloc_handle_budget() {
    let b = guest("i32.const 1 drop", "i32.const 0", "i32.const 0");
    let out = run(&b, limits(1000));
    assert_eq!(out.result, Ok(()));
    assert!(out.compute_used > 0);
    assert_eq!(run(&b, limits(out.compute_used)).result, Ok(()));
    assert_eq!(
        run(&b, limits(out.compute_used - 1)).result,
        Err(Error::ComputeExhausted)
    );
}
#[test]
fn exhausted_init_and_alloc_never_reach_next_export() {
    for b in [
        guest("(loop $l br $l)", "unreachable", "unreachable"),
        guest("", "(loop $l br $l) i32.const 0", "unreachable"),
    ] {
        let out = run(&b, limits(1000));
        assert_eq!(out.result, Err(Error::ComputeExhausted));
        assert_eq!(out.compute_used, 1000);
    }
}
#[test]
fn runaway_deadline_then_fresh_success() {
    // A bare branch can burn even the maximum admitted fuel before the
    // watchdog runs. Query memory growth per iteration: real guest work (no
    // host deadline checks), with ample wall-clock work per fuel unit. Unlike
    // memory.fill, memory.grow by zero does not charge fuel per byte.
    let b = guest(
        "",
        "i32.const 0",
        "(loop $l i32.const 0 memory.grow drop br $l) i32.const 0",
    );
    let mut l = limits(i32::MAX as u64);
    l.deadline = Instant::now() + Duration::from_secs(1);
    let out = run(&b, l);
    assert_eq!(out.result, Err(Error::DeadlineExceeded));
    assert!(out.compute_used < i32::MAX as u64 / 10, "{out:?}");
    println!(
        "deadline interrupted with compute_used={}",
        out.compute_used
    );
    assert_eq!(
        run(&guest("", "i32.const 0", "i32.const 0"), limits(1000)).result,
        Ok(())
    );
}
#[test]
fn memory_admission_growth_and_stack_bound() {
    let b = guest(
        "",
        "i32.const 0",
        "i32.const 2 memory.grow i32.const -1 i32.ne",
    );
    assert_eq!(run(&b, limits(1000)).result, Ok(()));
    let mut l = limits(1000);
    l.memory_pages = 1;
    assert!(matches!(
        Wasmtime::default().load(&b, &l),
        Err(Error::MemoryExhausted)
    ));
    let recursive=wat::parse_str(r#"(module (memory (export "memory") 1 2) (func $r call $r) (func (export "abi_alloc") (param i32) (result i32) i32.const 0) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) call $r i32.const 0))"#).unwrap();
    assert_eq!(
        run(&recursive, limits(1000000)).result,
        Err(Error::MemoryExhausted)
    );
}
#[test]
fn mode_pointer_return_and_fresh_globals() {
    let b = guest("", "i32.const -1", "i32.const 0");
    assert_eq!(run(&b, limits(1000)).result, Err(Error::InvalidPointer));
    let mut l = limits(1000);
    l.compute_mode = "wamr-2.4.3-fast-cumulative-v1".into();
    assert!(matches!(
        Wasmtime::default().load(&b, &l),
        Err(Error::UnsupportedMode)
    ));
    let b = guest("", "i32.const 0", "i32.const 7");
    assert_eq!(run(&b, limits(1000)).result, Err(Error::GuestReturn(7)));
    let b=wat::parse_str(r#"(module (memory (export "memory") 1 2) (global $g (mut i32) (i32.const 0)) (func (export "abi_alloc") (param i32) (result i32) i32.const 0) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) global.get $g i32.const 1 global.set $g))"#).unwrap();
    let l = limits(1000);
    let m = Wasmtime::default().load(&b, &l).unwrap();
    for _ in 0..3 {
        let out = Wasmtime::default()
            .instantiate(&m, limits(1000))
            .unwrap()
            .handle(
                StepInput {
                    events: vec![0x81, 0xa0],
                    step: 0,
                    logical_time: 0,
                    seed: 0,
                },
                &mut NoHost,
            );
        assert_eq!(out.result, Ok(()));
    }
}
