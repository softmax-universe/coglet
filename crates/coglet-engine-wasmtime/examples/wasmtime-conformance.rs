//! Test-only JSON-line adapter; the shared CBOR bridge supplies harness framing.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput, StepOutcome,
};
use coglet_engine_wasmtime::{COMPUTE_MODE, Wasmtime};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
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
struct FixtureHost;
impl Host for FixtureHost {
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
fn unhex(value: &str) -> Result<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return Err(Error::InvalidArgument);
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| {
            let text = std::str::from_utf8(b).map_err(|_| Error::InvalidArgument)?;
            u8::from_str_radix(text, 16).map_err(|_| Error::InvalidArgument)
        })
        .collect()
}
fn code(error: &Error) -> &'static str {
    match error {
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
fn run(request: Request) -> StepOutcome {
    let attempt = || -> Result<StepOutcome> {
        if request.op != "step"
            || request.fixture.is_empty()
            || request.wall_time_us == 0
            || request.wall_time_us > 300_000_000
        {
            return Err(Error::InvalidArgument);
        }
        // 'qualified' is a test fixture alias, resolved explicitly and reported;
        // never a production compute-mode fallback. Wasmtime checks exact mode.
        let compute_mode = if request.compute_mode == "qualified" {
            COMPUTE_MODE.into()
        } else {
            request.compute_mode.clone()
        };
        let l = Limits {
            compute_mode,
            compute: request.compute,
            memory_pages: request.memory_pages,
            host_bytes: request.host_bytes,
            runtime_bytes: request.runtime_bytes,
            stack_bytes: request.stack_bytes,
            deadline: Instant::now()
                .checked_add(Duration::from_micros(request.wall_time_us))
                .ok_or(Error::InvalidArgument)?,
        };
        let e = Wasmtime::default();
        let m = e.load(&unhex(&request.module_hex)?, &l)?;
        let input = StepInput {
            events: unhex(&request.events_hex)?,
            step: request.step,
            logical_time: request.logical_time,
            seed: request.seed,
        };
        Ok(e.instantiate(&m, l)?.handle(input, &mut FixtureHost))
    };
    match attempt() {
        Ok(v) => v,
        Err(e) => StepOutcome {
            result: Err(e),
            compute_used: 0,
            host_bytes_used: 0,
        },
    }
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.len() > 4 * 1024 * 1024 {
            return Err("oversize request".into());
        }
        let outcome = match serde_json::from_str::<Request>(&line) {
            Ok(r) => run(r),
            Err(_) => StepOutcome {
                result: Err(Error::InvalidArgument),
                compute_used: 0,
                host_bytes_used: 0,
            },
        };
        let error: Value = outcome
            .result
            .as_ref()
            .err()
            .map(|e| json!(code(e)))
            .unwrap_or(Value::Null);
        let response = json!({"ok":outcome.result.is_ok(),"error":error,"compute_used":outcome.compute_used,"host_bytes_used":outcome.host_bytes_used,"effective_compute_mode":COMPUTE_MODE});
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    Ok(())
}
