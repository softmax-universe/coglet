//! Differential real-engine execution, not a mock runtime/journal proof.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::Wamr;
use coglet_engine_wasmtime::Wasmtime;
use coglet_fs::Snapshot;
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
#[derive(Default)]
struct Capture(Vec<u8>);
impl Host for Capture {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        assert_eq!((module, name), ("coglet_v0", "log"));
        // Test-only host captures raw state bytes, not a production log decoder.
        self.0 = memory.read(args[0] as u32, args[1] as u32)?;
        Ok(0)
    }
}
fn limits<E: Engine>(engine: &E) -> Limits {
    Limits {
        compute_mode: engine.metadata().compute_mode.into(),
        compute: 1_000_000,
        memory_pages: 2,
        host_bytes: 8192,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn rewrite(width: u32) -> String {
    format!(
        "local.set $t{width} local.get $t{width} f{width}.const nan:0x{} local.get $t{width} local.get $t{width} f{width}.eq select",
        if width == 32 {
            "400000"
        } else {
            "8000000000000"
        }
    )
}
fn guest(body: &str) -> Vec<u8> {
    wat::parse_str(format!(r#"(module
(import "coglet_v0" "log" (func $save (param i32 i32) (result i32)))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param $p i32) (param i32) (param $step i64) (param $time i64) (param $seed i64) (result i32)
(local $t32 f32) (local $t64 f64)
{body}
i32.const 256 i32.const 64 call $save drop i32.const 0))"#)).unwrap()
}
fn execute<E: Engine>(e: &E, b: &[u8], state: &[u8], step: u64) -> Vec<u8> {
    let mut events = vec![0x81, 0x58, 64];
    events.extend_from_slice(state); // deterministic CBOR test input
    let l = limits(e);
    let m = e.load(b, &l).unwrap();
    let mut host = Capture::default();
    let out = e.instantiate(&m, l).unwrap().handle(
        StepInput {
            events,
            step,
            logical_time: step * 1000,
            seed: step.wrapping_mul(0x9e3779b97f4a7c15),
        },
        &mut host,
    );
    assert_eq!(out.result, Ok(()), "{} step {step}", e.metadata().engine);
    assert_eq!(host.0.len(), 64);
    host.0
}
fn snapshot(b: &[u8], state: &[u8]) -> Snapshot {
    Snapshot::from_files(BTreeMap::from([
        ("/code/bin/step.wasm".into(), b.to_vec()),
        ("/data/state.bin".into(), state.to_vec()),
    ]))
    .unwrap()
}
#[test]
fn cross_engine_state_hash() {
    let mut bodies=vec!["i32.const 256 local.get $p i32.const 3 i32.add i32.const 64 memory.copy i32.const 256 local.get $seed i64.store i32.const 264 local.get $step i64.store".to_string()];
    // Every R3 operator, both widths; every step feeds prior raw bits back in.
    for width in [32, 64] {
        let load = if width == 32 {
            "i32.const 3 f32.load"
        } else {
            "i32.const 3 f64.load"
        };
        let mut body = String::new();
        for (index, op) in [
            "add", "sub", "mul", "div", "sqrt", "ceil", "floor", "trunc", "nearest", "min", "max",
        ]
        .iter()
        .enumerate()
        {
            body.push_str(&format!("i32.const {} {load} ", 256 + (index % 8) * 8));
            if matches!(*op, "add" | "sub" | "mul" | "div" | "min" | "max") {
                body.push_str(&format!("f{width}.const -0.5 "));
            }
            body.push_str(&format!("f{width}.{op} {} f{width}.store ", rewrite(width)));
        }
        // promote and demote with exact per-operation R3 suffixes.
        body.push_str(&format!("i32.const 312 i32.const 3 f32.load f64.promote_f32 {} f64.store i32.const 316 i32.const 3 f64.load f32.demote_f64 {} f32.store",rewrite(64),rewrite(32)));
        bodies.push(body);
    }
    // Bit-exact ops must preserve payloads, including signed zeros and signaling NaNs.
    bodies.push("i32.const 256 i32.const 3 f64.load f64.neg f64.store i32.const 264 i32.const 3 f64.load f64.abs f64.store i32.const 272 i32.const 3 f32.load f32.const -1 f32.copysign f32.store".into());
    for (case, body) in bodies.iter().enumerate() {
        let b = guest(body);
        for bits in [
            0u64,
            1,
            0x8000000000000000,
            0x7ff0000000000000,
            0xfff0000000000000,
            0x7ff82b108398f114,
            0x7ff0000000000001,
            0x3ff123457fc00001,
            0x3ff0000000000000,
        ] {
            let mut a = bits.to_le_bytes().repeat(8);
            let mut c = a.clone();
            for step in 0..32 {
                let prev = a.clone();
                a = execute(&Wamr::interpreter(), &b, &a, step);
                c = execute(&Wasmtime, &b, &c, step);
                assert_eq!(a, c, "raw state case {case} bits {bits:x} step {step}");
                assert_eq!(
                    snapshot(&b, &a).tree_hash(),
                    snapshot(&b, &c).tree_hash(),
                    "tree hash case {case} step {step}"
                );
                if case == 0 && step > 0 {
                    assert_ne!(a, prev, "negative control: state must evolve");
                }
            }
        }
    }
}
#[test]
fn unrewritten_float_guest_is_rejected_by_both_engines() {
    let b = guest("i32.const 256 f64.const 0 f64.const 0 f64.div f64.store");
    assert!(matches!(
        Wamr::interpreter().load(&b, &limits(&Wamr::interpreter())),
        Err(Error::UnqualifiedModule)
    ));
    assert!(matches!(
        Wasmtime.load(&b, &limits(&Wasmtime)),
        Err(Error::UnqualifiedModule)
    ));
}
