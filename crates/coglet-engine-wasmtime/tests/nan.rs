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
        panic!()
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
        deadline: Instant::now() + Duration::from_secs(1),
    }
}
fn guest(scrub: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32) (local $t f64)
 f64.const nan:0x8000000000042 f64.const 1 f64.add
 {scrub}
 i64.reinterpret_f64 i64.const 9221120237041090560 i64.ne))"#
    ))
    .unwrap()
}
#[test]
fn production_nan_validator_rejects_unrewritten_computation() {
    assert!(matches!(
        Wasmtime.load(&guest(""), &limits()),
        Err(Error::UnqualifiedModule)
    ));
}
#[test]
fn float_nan_repeated_execution() {
    let b = guest(
        "local.set $t local.get $t f64.const nan:0x8000000000000 local.get $t local.get $t f64.eq select",
    );
    for _ in 0..5 {
        let e = Wasmtime;
        let l = limits();
        let m = e.load(&b, &l).unwrap();
        let out = e.instantiate(&m, l).unwrap().handle(
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
#[test]
fn every_r3_operator_requires_and_executes_the_exact_rewrite() {
    for width in [32, 64] {
        let float = if width == 32 { "f32" } else { "f64" };
        let integer = if width == 32 { "i32" } else { "i64" };
        let canonical = if width == 32 {
            "nan:0x400000"
        } else {
            "nan:0x8000000000000"
        };
        let bits = if width == 32 {
            "0x7fc00000"
        } else {
            "0x7ff8000000000000"
        };
        let mut operations = vec![
            "add", "sub", "mul", "div", "sqrt", "ceil", "floor", "trunc", "nearest", "min", "max",
        ];
        operations.push(if width == 32 {
            "demote_f64"
        } else {
            "promote_f32"
        });
        for op in operations {
            let unary = [
                "sqrt",
                "ceil",
                "floor",
                "trunc",
                "nearest",
                "demote_f64",
                "promote_f32",
            ]
            .contains(&op);
            let input_type = if op == "demote_f64" {
                "f64"
            } else if op == "promote_f32" {
                "f32"
            } else {
                float
            };
            let input = if input_type == "f32" {
                "nan:0x400042"
            } else {
                "nan:0x8000000000042"
            };
            let second = if unary {
                String::new()
            } else {
                format!("{float}.const 1")
            };
            let prefix = format!("{input_type}.const {input} {second} {float}.{op}");
            let scrub = format!(
                "local.set $t local.get $t {float}.const {canonical} local.get $t local.get $t {float}.eq select"
            );
            let build = |scrub: &str| {
                wat::parse_str(format!(r#"(module
                (memory (export "memory") 1 2)
                (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
                (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) (local $t {float})
                {prefix} {scrub} {integer}.reinterpret_{float} {integer}.const {bits} {integer}.ne))"#)).unwrap()
            };
            let e = Wasmtime;
            let l = limits();
            assert!(
                matches!(e.load(&build(""), &l), Err(Error::UnqualifiedModule)),
                "{float}.{op}"
            );
            let m = e.load(&build(&scrub), &l).unwrap();
            let out = e.instantiate(&m, l).unwrap().handle(
                StepInput {
                    events: vec![0x81, 0xa0],
                    step: 0,
                    logical_time: 0,
                    seed: 0,
                },
                &mut NoHost,
            );
            assert_eq!(out.result, Ok(()), "{float}.{op}");
        }
    }
}

#[test]
fn wrong_nan_payload_is_rejected() {
    let wrong = guest(
        "local.set $t local.get $t f64.const nan:0x8000000000042 local.get $t local.get $t f64.eq select",
    );
    assert!(matches!(
        Wasmtime.load(&wrong, &limits()),
        Err(Error::UnqualifiedModule)
    ));
}
