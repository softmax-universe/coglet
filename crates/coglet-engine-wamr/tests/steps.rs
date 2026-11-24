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
fn limits(compute: u64) -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute,
        memory_pages: 2,
        host_bytes: 4096,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        // Non-deadline cases must tolerate the serialized runaway test.
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn bytes(init: &str, alloc: &str, handle: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(memory (export "memory") 1 2)
(func (export "_initialize") {init})
(func (export "abi_alloc") (param i32) (result i32) {alloc})
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {handle}))"#
    ))
    .unwrap()
}
fn run(bytes: &[u8], limits: Limits) -> coglet_engine::StepOutcome {
    let e = Wamr::interpreter();
    let module = e.load(bytes, &limits).unwrap();
    e.instantiate(&module, limits).unwrap().handle(
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
fn cumulative_init_alloc_handle_budget() {
    let b = bytes("nop nop", "i32.const 0", "i32.const 0");
    let first = run(&b, limits(1000));
    assert_eq!(first.result, Ok(()));
    let used = first.compute_used;
    assert!(used > 0 && used < 1000);
    let exact = run(&b, limits(used));
    assert_eq!(exact.result, Ok(()));
    assert_eq!(exact.compute_used, used);
    assert_eq!(
        run(&b, limits(used - 1)).result,
        Err(Error::ComputeExhausted)
    );
}
#[test]
fn exhausted_init_never_calls_alloc_or_handle() {
    let b = bytes("(loop $l br $l)", "unreachable", "unreachable");
    let out = run(&b, limits(1000));
    assert_eq!(out.result, Err(Error::ComputeExhausted));
    assert_eq!(out.compute_used, 1000);
}
#[test]
fn exhausted_alloc_never_calls_handle() {
    let b = bytes("", "(loop $l br $l) i32.const 0", "unreachable");
    let out = run(&b, limits(1000));
    assert_eq!(out.result, Err(Error::ComputeExhausted));
    assert_eq!(out.compute_used, 1000);
}
#[test]
fn constructors_share_the_step_allowance() {
    let b = wat::parse_str(
        r#"(module
    (memory (export "memory") 1 2)
    (func (export "__wasm_call_ctors") (loop $l br $l))
    (func (export "abi_alloc") (param i32) (result i32) unreachable)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) unreachable))"#,
    )
    .unwrap();
    assert_eq!(run(&b, limits(1000)).result, Err(Error::ComputeExhausted));
}

#[test]
fn host_stack_and_memory_bound() {
    let mut l = limits(100_000);
    l.stack_bytes = 16_384;
    let recursive=wat::parse_str(r#"(module
    (memory (export "memory") 1 2)
    (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
    (func $recursive call $recursive)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) call $recursive i32.const 0))"#).unwrap();
    assert_eq!(run(&recursive, l).result, Err(Error::MemoryExhausted));
    let mut l = limits(1000);
    l.runtime_bytes = 1024 * 1024;
    // Linear memory is independently capped; exhaust WAMR's metadata pool with
    // many tiny functions, not mmap-backed linear pages outside that pool.
    let mut source = String::from("(module (memory (export \"memory\") 1 2)");
    source.push_str(&"(func)".repeat(15_000));
    source.push_str(
        r#"(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#,
    );
    let large = wat::parse_str(source).unwrap();
    // Fixture construction is not part of this admitted attempt; allow slow
    // QEMU validation to reach the allocator boundary (not a latency assertion).
    l.deadline = Instant::now() + Duration::from_secs(10);
    assert_eq!(run(&large, l).result, Err(Error::MemoryExhausted));
    assert_eq!(
        run(&bytes("", "i32.const 0", "i32.const 0"), limits(1000)).result,
        Ok(())
    );
}

#[test]
fn runaway_deadline_then_next_fresh_call_succeeds() {
    // Keep wall-clock work per compute unit high, as in the Wasmtime fixture:
    // a bare branch races the finite compute allowance against the watchdog.
    let b = bytes(
        "",
        "i32.const 0",
        "(loop $l i32.const 0 memory.grow drop br $l) i32.const 0",
    );
    let mut l = limits(i32::MAX as u64);
    // Setup can wait behind the runtime lock while the memory-bound fixture
    // validates thousands of functions; 20ms races that unrelated work.
    l.deadline = Instant::now() + Duration::from_secs(1);
    let out = run(&b, l);
    assert_eq!(out.result, Err(Error::DeadlineExceeded));
    assert!(out.compute_used < i32::MAX as u64 / 10, "{out:?}");
    println!(
        "deadline interrupted with compute_used={}",
        out.compute_used
    );
    assert_eq!(
        run(&bytes("", "i32.const 0", "i32.const 0"), limits(1000)).result,
        Ok(())
    );
}
