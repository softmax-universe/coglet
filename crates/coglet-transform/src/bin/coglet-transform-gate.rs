//! Finite native publication gate, retained as an executable CI artifact.
//! No filesystem/network imports. This deliberately narrow gate executes two
//! fresh interpreter steps of the exact output; it is not general M3 replay.
use coglet_engine::{Engine, GuestMemory, Host, Instance, Limits, StepBudget, StepInput};
use coglet_engine_wamr::Wamr;
use std::{
    io::{Read, Write},
    time::{Duration, Instant},
};
struct LogOnly;
impl Host for LogOnly {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        _: &mut StepBudget,
    ) -> coglet_engine::Result<u32> {
        if module != "coglet_v0" || name != "log" || args.len() != 2 {
            return Err(coglet_engine::Error::UnqualifiedModule);
        }
        let _ = memory.read(args[0] as u32, args[1] as u32)?;
        Ok(0)
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args_os().len() != 1 {
        return Err("usage: coglet-transform-gate <output.wasm".into());
    }
    let mut output = Vec::new();
    std::io::stdin().take(65537).read_to_end(&mut output)?;
    if output.len() > 65536 {
        return Err("output byte limit".into());
    }
    let engine = Wamr::interpreter();
    let mut computes = Vec::new();
    for seed in [0, 0xfff8_0000_0000_0042] {
        let limits = Limits {
            compute_mode: engine.metadata().compute_mode.into(),
            compute: 10000,
            memory_pages: 2,
            host_bytes: 65536,
            runtime_bytes: 16 * 1024 * 1024,
            stack_bytes: 65536,
            deadline: Instant::now() + Duration::from_secs(1),
        };
        let module = engine.load(&output, &limits)?;
        let result = engine.instantiate(&module, limits)?.handle(
            StepInput {
                events: vec![0x81, 0xa0],
                step: 0,
                logical_time: 0,
                seed,
            },
            &mut LogOnly,
        );
        result.result?;
        computes.push(result.compute_used);
    }
    let report = serde_json::json!({"version":1,"output_sha256":coglet_transform::sha256_hex(&output),"engine":engine.metadata().engine,"compute_mode":engine.metadata().compute_mode,"cases":2,"compute_used":computes});
    std::io::stdout().write_all(&serde_json::to_vec(&report)?)?;
    Ok(())
}
