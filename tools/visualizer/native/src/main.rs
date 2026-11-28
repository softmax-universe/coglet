//! CLI boundary for the shared offline native oracle.
use coglet_runtime::RuntimeService;
use coglet_visualizer_native::{read_archive, parse_request, replay_report};
use std::{fs, io::{self, Read, Write}, path::Path};
type Result<T> = std::result::Result<T, String>;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("export") if args.len() == 4 => {
            // Only an explicitly selected STOPPED TEST WAL, never a live router.
            let setup = read_archive(&args[1])?;
            let mut service = RuntimeService::open(Path::new(&args[2]), &setup, 128)?;
            let last = service.records().last().ok_or("empty capture")?;
            let id = last["trace_id"].clone();
            let time = last["logical_time"].as_u64().ok_or("clock")?;
            let trace = service.export_trace(&id, time)?;
            let mut options = fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options.open(&args[3]).map_err(|e| e.to_string())?;
            file.write_all(&serde_json::to_vec(&trace).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        }
        Some("replay") if args.len() == 2 => {
            let mut raw = Vec::new();
            io::stdin().take(65_537).read_to_end(&mut raw).map_err(|e| e.to_string())?;
            if raw.len() > 65_536 { return Err("request too large".into()); }
            let request = parse_request(&raw)?;
            let trace = read_archive(&args[1])?;
            println!("{}", replay_report(&trace, &request)?);
        }
        _ => return Err("usage: export SETUP STOPPED_TEST_WAL NEW_TRACE | replay TRACE < REQUEST".into()),
    }
    Ok(())
}
