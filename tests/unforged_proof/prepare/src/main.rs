//! Supervisor-only stage-1 package preparation. Never included in a hosted program.
use serde_json::json;
use std::{fs, path::Path};

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err("usage: coglet-unforged-prepare PACKAGE OUTPUT".into());
    }
    let setup = coglet_cli::package_setup(Path::new(&args[1])).map_err(|e| e.to_string())?;
    let id = setup["manifest"]["id"].clone();
    let batches: Vec<_> = (0..5).map(|step| json!([{"kind":"runtime.debug_step","source":id,"seq":step,"logical_time":step,"correlation_id":"11111111111111111111111111111111","payload":{}}])).collect();
    let job = coglet_cli::sandbox_job(
        setup,
        json!(batches),
        json!([]),
        json!({"coglet":id,"operations":[],"resources":[]}),
        "11111111111111111111111111111111",
        7,
    ).map_err(|e| e.to_string())?;
    fs::write(&args[2], serde_json::to_vec(&job).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("coglet-unforged-prepare: {error}");
        std::process::exit(1);
    }
}
