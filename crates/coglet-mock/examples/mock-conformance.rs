//! Test-only native simulations bound to pinned exact Wasm fixture inputs.
//! The fixture name NEVER chooses execution: only registered bytes do. This does
//! not validate/interpret Wasm or qualify real interpreter/AOT enforcement.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput, StepOutcome,
};
use coglet_mock::engine::{COMPUTE_MODE, Context, MockEngine, NativeGuest, NativeModule};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    op: String,
    fixture: String,
    module_hex: String,
    compute_mode: String,
    compute: u64,
    memory_pages: u32,
    host_bytes: u64,
    runtime_bytes: u32,
    stack_bytes: u32,
    wall_time_us: u64,
    events_hex: String,
    step: u64,
    logical_time: u64,
    seed: u64,
}
struct DenyHost;
impl Host for DenyHost {
    fn call(
        &mut self,
        _: &str,
        _: &str,
        _: &[u64],
        _: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> Result<u32> {
        Err(Error::UnqualifiedModule)
    }
}
fn decode(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err(Error::InvalidArgument);
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| {
            u8::from_str_radix(
                std::str::from_utf8(b).map_err(|_| Error::InvalidArgument)?,
                16,
            )
            .map_err(|_| Error::InvalidArgument)
        })
        .collect()
}
#[derive(Clone, Copy)]
enum Program {
    Success,
    Fresh,
    Runaway,
    AllocationRunaway,
    Growth,
    BadPointer,
}
struct Guest {
    program: Program,
    global: u32,
}
fn spin(cx: &mut Context<'_>) -> Result<u32> {
    loop {
        cx.charge(1)?;
    }
}
impl NativeGuest for Guest {
    fn allocate(&mut self, len: u32, cx: &mut Context<'_>) -> Result<u32> {
        if matches!(self.program, Program::AllocationRunaway) {
            return spin(cx);
        }
        cx.charge(1)?;
        if matches!(self.program, Program::BadPointer) {
            return Ok(u32::MAX);
        }
        cx.memory.read(0, len)?;
        Ok(0)
    }
    fn handle(&mut self, _: u32, _: &StepInput, cx: &mut Context<'_>) -> Result<u32> {
        cx.charge(1)?;
        match self.program {
            Program::Runaway => spin(cx),
            Program::Fresh => {
                let old = self.global;
                self.global += 1;
                Ok(old)
            }
            Program::Growth => {
                let first = cx.memory.grow(1);
                let second = cx.memory.grow(1);
                Ok(u32::from(
                    first != 1 || second != -1 || cx.memory.pages() != 2,
                ))
            }
            _ => Ok(0),
        }
    }
}
fn engine() -> MockEngine {
    // Separate pinned INPUT catalog. No expected responses/case output is loaded.
    let catalog: BTreeMap<String, String> =
        serde_json::from_str(include_str!("native-fixtures.json")).unwrap();
    let mut e = MockEngine::default();
    for (name, program, maximum) in [
        ("success", Program::Success, 2),
        ("fresh-global", Program::Fresh, 2),
        ("runaway", Program::Runaway, 2),
        ("allocation-runaway", Program::AllocationRunaway, 2),
        ("memory-growth", Program::Growth, 2),
        ("invalid-allocation", Program::BadPointer, 2),
        ("memory-too-large", Program::Success, 3),
    ] {
        e.register(
            decode(&catalog[name]).unwrap(),
            NativeModule::new(1, maximum, move || Box::new(Guest { program, global: 0 })).unwrap(),
        )
        .unwrap();
    }
    // Forbidden imports/start/signatures have NO native implementation registered.
    e
}
fn failed(error: Error) -> StepOutcome {
    StepOutcome {
        result: Err(error),
        compute_used: 0,
        host_bytes_used: 0,
    }
}
fn run(e: &MockEngine, r: Request) -> StepOutcome {
    let attempt = || -> Result<StepOutcome> {
        if r.op != "step"
            || r.fixture.is_empty()
            || r.wall_time_us == 0
            || r.wall_time_us > 300_000_000
        {
            return Err(Error::InvalidArgument);
        }
        let limits = Limits {
            compute_mode: if r.compute_mode == "qualified" {
                COMPUTE_MODE.into()
            } else {
                r.compute_mode
            },
            compute: r.compute,
            memory_pages: r.memory_pages,
            host_bytes: r.host_bytes,
            runtime_bytes: r.runtime_bytes,
            stack_bytes: r.stack_bytes,
            deadline: Instant::now()
                .checked_add(Duration::from_micros(r.wall_time_us))
                .ok_or(Error::InvalidArgument)?,
        };
        let m = e.load(&decode(&r.module_hex)?, &limits)?;
        let instance = e.instantiate(&m, limits)?;
        Ok(instance.handle(
            StepInput {
                events: decode(&r.events_hex)?,
                step: r.step,
                logical_time: r.logical_time,
                seed: r.seed,
            },
            &mut DenyHost,
        ))
    };
    attempt().unwrap_or_else(failed)
}
fn code(e: &Error) -> &'static str {
    match e {
        Error::InvalidArgument => "invalid_argument",
        Error::UnsupportedMode => "unsupported_mode",
        Error::UnqualifiedModule => "unqualified_module",
        Error::ComputeExhausted => "compute_exhausted",
        Error::MemoryExhausted => "memory_exhausted",
        Error::HostBytesExhausted => "host_bytes_exhausted",
        Error::DeadlineExceeded => "deadline_exceeded",
        Error::InvalidPointer => "invalid_pointer",
        Error::GuestExit(_) => "guest_exit",
        Error::GuestReturn(_) => "guest_return",
        Error::Trap(_) => "trap",
        Error::ReplayDiverged => "replay_diverged",
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let e = engine();
    let mut out = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.len() > 4 * 1024 * 1024 {
            return Err("oversize request".into());
        }
        let outcome = match serde_json::from_str(&line) {
            Ok(r) => run(&e, r),
            Err(_) => failed(Error::InvalidArgument),
        };
        serde_json::to_writer(
            &mut out,
            &json!({"ok": outcome.result.is_ok(), "error": outcome.result.as_ref().err().map(code), "compute_used": outcome.compute_used, "host_bytes_used": outcome.host_bytes_used, "effective_compute_mode": COMPUTE_MODE, "simulation": true, "deadline_interrupt": false}),
        )?;
        writeln!(out)?;
        out.flush()?;
    }
    Ok(())
}
