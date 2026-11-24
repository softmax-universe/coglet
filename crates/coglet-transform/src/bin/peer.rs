//! Aarch64/x86_64 peer evaluator. Not a publication credential.
use coglet_engine::{Engine, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput};
use coglet_engine_wamr::Wamr;
use coglet_engine_wasmtime::Wasmtime;
use std::io::{Read, Write};
use std::time::{Duration, Instant};

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
        self.0 = memory.read(args[0] as u32, args[1] as u32)?;
        Ok(0)
    }
}

fn limits<E: Engine>(e: &E) -> Limits {
    Limits {
        compute_mode: e.metadata().compute_mode.into(),
        compute: 1_000_000,
        memory_pages: 2,
        host_bytes: 8192,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}

fn run<E: Engine>(e: &E, wasm: &[u8], seed: u64) -> u64 {
    let l = limits(e);
    let m = e.load(wasm, &l).expect("load");
    let mut host = Capture(Vec::new());
    let out = e.instantiate(&m, l).expect("inst").handle(
        StepInput {
            events: vec![0x81, 0xa0],
            step: 0,
            logical_time: 0,
            seed,
        },
        &mut host,
    );
    assert_eq!(out.result, Ok(()));
    assert_eq!(host.0.len(), 8);
    u64::from_le_bytes(host.0.try_into().unwrap())
}

fn main() {
    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input).unwrap();
    assert!(input.len() >= 8);
    let wasm_len = u32::from_le_bytes(input[0..4].try_into().unwrap()) as usize;
    let module = &input[4..4 + wasm_len];
    let rest = &input[4 + wasm_len..];
    assert_eq!(rest.len() % 8, 0);
    let wamr = Wamr::interpreter();
    let wasmtime = Wasmtime::default();
    let mut out = Vec::new();
    out.extend(std::env::consts::ARCH.as_bytes());
    out.push(0);
    for seed in rest.as_chunks::<8>().0 {
        let seed = u64::from_le_bytes(*seed);
        out.extend(run(&wamr, module, seed).to_le_bytes());
        out.extend(run(&wasmtime, module, seed).to_le_bytes());
    }
    std::io::stdout().write_all(&out).unwrap();
}
