//! Opt-in local proof driver. No unforged library is linked into Coglet.
use coglet_runtime::{NativeEngine, Runtime};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};

fn files(runtime: &Runtime) -> BTreeMap<String, String> {
    // Runtime's existing WAL serialization exposes the private snapshot without
    // adding a production API or changing journal/trace formats.
    serde_json::from_value(serde_json::to_value(runtime).unwrap()["files"].clone()).unwrap()
}
fn read(path: &str) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("record") | Some("fork-trace") if args.len() == 3 => {
            let trace = coglet_visualizer_native::read_archive(&args[2])?;
            let archive = if args[1] == "record" {
                coglet_visualizer_native::record(&trace)?
            } else {
                use std::io::Read;
                let mut raw = Vec::new();
                std::io::stdin().take(65_537).read_to_end(&mut raw).map_err(|e| e.to_string())?;
                if raw.len() > 65_536 { return Err("request too large".into()); }
                let request = coglet_visualizer_native::parse_request(&raw)?;
                coglet_visualizer_native::fork_trace(&trace, &request)?
            };
            println!("{}", serde_json::to_string(&archive).map_err(|e| e.to_string())?);
            return Ok(());
        }
        _ => {}
    }
    if args.get(1).map(String::as_str) == Some("prepare") {
        let setup = coglet_cli::package_setup(Path::new(&args[2])).map_err(|e| e.to_string())?;
        let id = setup["manifest"]["id"].clone();
        let batches: Vec<_> = (0..5).map(|step| json!([{"kind":"runtime.debug_step","source":id,"seq":step,"logical_time":step,"correlation_id":"11111111111111111111111111111111","payload":{}}])).collect();
        let job = coglet_cli::sandbox_job(
            setup,
            json!(batches),
            json!([]),
            json!({"coglet":id,"operations":[],"resources":[]}),
            "11111111111111111111111111111111",
            7,
        )
        .map_err(|e| e.to_string())?;
        fs::write(&args[3], serde_json::to_vec(&job).unwrap()).map_err(|e| e.to_string())?;
        return Ok(());
    }
    if args.get(1).map(String::as_str) == Some("snapshot") {
        let runtime = Runtime::import_trace(&read(&args[2]))?;
        println!("{}", serde_json::to_string(&files(&runtime)).unwrap());
        return Ok(());
    }
    let job = read("proof-input.json");
    let mut runtime = Runtime::import_trace(&job["prefix"])?;
    let before = files(&runtime);
    for (path, bytes) in &before {
        let actual = fs::read(&path[1..]).map_err(|e| format!("prior state {path}: {e}"))?;
        if hex::encode(actual) != *bytes {
            return Err(format!("prior state differs: {path}"));
        }
    }
    let setup = &job["setup"];
    let mut engine = NativeEngine::new(
        hex::decode(&before["/code/bin/step.wasm"]).map_err(|e| e.to_string())?,
        &setup["manifest"]["budgets"],
        16 * 1024 * 1024,
        64 * 1024,
    )?;
    let result = runtime.step(&job["input"], &mut engine)?;
    if result["journal"]["status"] != "committed" {
        return Err(format!("step failed: {result}"));
    }
    let after = files(&runtime);
    for path in before.keys().filter(|p| !after.contains_key(*p)) {
        fs::remove_file(&path[1..]).map_err(|e| e.to_string())?;
    }
    for (path, bytes) in after {
        let path = Path::new(&path[1..]);
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(path, hex::decode(bytes).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    }
    fs::remove_file("proof-input.json").map_err(|e| e.to_string())?;
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("coglet-unforged-proof: {error}");
        std::process::exit(1);
    }
}
