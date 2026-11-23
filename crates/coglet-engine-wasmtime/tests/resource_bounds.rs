use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wasmtime::{COMPUTE_MODE, Wasmtime};
use std::time::{Duration, Instant};
fn limits() -> Limits {
    Limits {
        compute_mode: COMPUTE_MODE.into(),
        compute: 1000000,
        memory_pages: 32,
        host_bytes: 4096,
        runtime_bytes: 1024 * 1024,
        stack_bytes: 65536,
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
struct Empty;
impl Host for Empty {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        _: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        panic!("unexpected")
    }
}
#[test]
fn runtime_memory_and_table_allocations_are_bounded() {
    for extra in [
        "(memory (export \"memory\") 17 32)",
        "(memory (export \"memory\") 1 32) (table 100000 100000 funcref)",
    ] {
        let b=wat::parse_str(format!(r#"(module {extra} (func (export "abi_alloc") (param i32) (result i32) i32.const 0) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#)).unwrap();
        let l = limits();
        let m = Wasmtime.load(&b, &l).unwrap();
        let i = Wasmtime.instantiate(&m, l).unwrap();
        assert_eq!(
            i.handle(input(), &mut Empty).result,
            Err(Error::MemoryExhausted)
        );
    }
}
#[test]
fn host_compute_charges_share_remaining_store_fuel() {
    struct Charged;
    impl Host for Charged {
        fn call(
            &mut self,
            _: &str,
            _: &str,
            _: &[u64],
            _: &mut dyn GuestMemory,
            b: &mut StepBudget,
        ) -> Result<u32> {
            b.charge_compute(1_000_000)?;
            Ok(0)
        }
    }
    let b=wat::parse_str(r#"(module (import "coglet_v0" "log" (func $h (param i32 i32) (result i32))) (memory (export "memory") 1 32) (func (export "abi_alloc") (param i32) (result i32) i32.const 0 i32.const 0 call $h drop unreachable) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) unreachable))"#).unwrap();
    let l = limits();
    let m = Wasmtime.load(&b, &l).unwrap();
    assert_eq!(
        Wasmtime
            .instantiate(&m, l)
            .unwrap()
            .handle(input(), &mut Charged)
            .result,
        Err(Error::ComputeExhausted)
    );
}
#[test]
fn concurrent_instances_do_not_share_epoch_deadlines() {
    use std::sync::{Arc, Barrier};
    struct Wait(Arc<Barrier>);
    impl Host for Wait {
        fn call(
            &mut self,
            _: &str,
            _: &str,
            _: &[u64],
            _: &mut dyn GuestMemory,
            b: &mut StepBudget,
        ) -> Result<u32> {
            self.0.wait();
            std::thread::sleep(Duration::from_millis(200));
            b.check_deadline()?;
            Ok(0)
        }
    }
    let b=wat::parse_str(r#"(module (import "coglet_v0" "log" (func $h (param i32 i32) (result i32))) (memory (export "memory") 1 32) (func (export "abi_alloc") (param i32) (result i32) i32.const 0) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0 i32.const 0 call $h))"#).unwrap();
    let l = limits();
    let m = Wasmtime.load(&b, &l).unwrap();
    let long = Wasmtime.instantiate(&m, l).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let other = barrier.clone();
    let handle = std::thread::spawn(move || long.handle(input(), &mut Wait(other)));
    barrier.wait();
    let b=wat::parse_str(r#"(module (memory (export "memory") 1 32) (func (export "abi_alloc") (param i32) (result i32) i32.const 0) (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) (loop $l br $l) i32.const 0))"#).unwrap();
    let mut l = limits();
    l.compute = i32::MAX as u64;
    l.deadline = Instant::now() + Duration::from_millis(100);
    let m = Wasmtime.load(&b, &l).unwrap();
    let i = Wasmtime.instantiate(&m, l).unwrap();
    assert_eq!(
        i.handle(input(), &mut Empty).result,
        Err(Error::DeadlineExceeded)
    );
    assert_eq!(handle.join().unwrap().result, Ok(()));
}
