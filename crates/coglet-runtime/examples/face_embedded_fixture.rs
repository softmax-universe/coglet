//! Actual-runtime Face fixture for the upcoming Swift C ABI probe.
//!
//! ```text
//! cargo run --release -p coglet-runtime --features native-engine \
//!   --example face_embedded_fixture -- prepare OUTPUTDIR QUALIFIED_FACE_WASM
//! cargo run --release -p coglet-runtime --features native-engine \
//!   --example face_embedded_fixture -- verify OUTPUTDIR
//! ```
//!
//! `prepare` writes operator-owned `binding.json` + `runtime.json` only. It does
//! not invent observations, actions, or a WAL. `verify` reopens that WAL after
//! the real FaceHost/C ABI driver ran and checks committed guest journals.
//!
//! Owner `coglet:c*64`, surface `surface:b*64`. Face production wall stays 1s.
//! `native_schedule.logical_time` is NOT Dispatch uptime: C ABI `run_native`
//! stamps `supervised_now()` (BOOTTIME / mach_continuous_time anchored at the
//! first WAL sample onto `runtime.time`, typically 0). Swift FaceHost clocks
//! and grant `expires` stay in Dispatch-uptime microseconds; capabilities
//! mapping converts between the two. Do not enlarge `wall_time_us`.

#[cfg(not(feature = "native-engine"))]
fn main() {
    eprintln!("face_embedded_fixture requires --features native-engine");
    std::process::exit(2);
}

#[cfg(feature = "native-engine")]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(feature = "native-engine")]
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("prepare") => {
            let output = args.next().ok_or("prepare OUTPUTDIR QUALIFIED_FACE_WASM")?;
            let wasm = args.next().ok_or("prepare OUTPUTDIR QUALIFIED_FACE_WASM")?;
            if args.next().is_some() {
                return Err("prepare OUTPUTDIR QUALIFIED_FACE_WASM".into());
            }
            prepare(std::path::Path::new(&output), std::path::Path::new(&wasm))
        }
        Some("verify") => {
            let output = args.next().ok_or("verify OUTPUTDIR")?;
            if args.next().is_some() {
                return Err("verify OUTPUTDIR".into());
            }
            verify(std::path::Path::new(&output))
        }
        _ => Err(
            "usage: face_embedded_fixture prepare OUTPUTDIR QUALIFIED_FACE_WASM | verify OUTPUTDIR"
                .into(),
        ),
    }
}

#[cfg(feature = "native-engine")]
mod actual {
    use super::{OWNER, SURFACE};
    use coglet_runtime::{NativeEngine, Runtime, RuntimeService, cbor, digest};
    use serde_json::{Value, json};
    use std::fs;
    use std::path::Path;

    pub const TRACE: &str = "55555555555555555555555555555555";
    pub const SEED: u64 = 7;
    pub const RUNTIME_BYTES: u32 = 32_000_000;
    pub const STACK_BYTES: u32 = 262_144;

    pub fn root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    pub fn native_schedule() -> Value {
        json!({
            "trace_id": TRACE,
            "seed": SEED,
            "runtime_bytes": RUNTIME_BYTES,
            "stack_bytes": STACK_BYTES
        })
    }

    pub fn face_setup(wasm: &[u8]) -> Result<Value, String> {
        let root = root();
        let cases: Value = serde_json::from_slice(
            &fs::read(root.join("conformance/abi/cases.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let mut setup = cases
            .as_array()
            .ok_or("cases")?
            .iter()
            .find(|c| c["name"] == "no-effects")
            .ok_or("no-effects")?["steps"][0]["send"]
            .clone();
        setup["manifest"] = serde_json::from_slice(
            &fs::read(root.join("coglets/face/manifest.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if setup["manifest"]["id"] != OWNER {
            return Err("face manifest owner is not coglet:c*64".into());
        }
        if setup["manifest"]["budgets"]["wall_time_us"] != 1_000_000 {
            return Err("refusing to enlarge production Face wall_time_us".into());
        }
        // Production Face lists catalog entries (speech.control / expression.control).
        // C ABI run_native execution_grants matches granted operation names, so the
        // installed grant must name speech.speak_chunk (the emit the guest uses).
        setup["manifest"]["required_capabilities"] = json!(["speech.speak_chunk"]);
        setup["initial_actions"] = json!([]);
        setup["initial_effect_ids"]["actions"] = json!([]);
        setup["initial_timers"] = json!([]);
        setup["initial_effect_ids"]["timers"] = json!([]);
        setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(wasm));
        let raw = hex::decode(
            setup["initial_files"]["/code/build/provenance.cbor"]
                .as_str()
                .ok_or("provenance")?,
        )
        .map_err(|e| e.to_string())?;
        let mut provenance: Value = coglet_types::decode("hub-build-record", &raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        provenance["rewritten_sha256"] = json!(digest(wasm));
        setup["initial_files"]["/code/build/provenance.cbor"] =
            json!(hex::encode(cbor(&provenance)?));
        setup["native_schedule"] = native_schedule();
        // C ABI start ignores inputs/guest_script; keep them out so the installed
        // fixture cannot be mistaken for a mock step script.
        if let Some(object) = setup.as_object_mut() {
            object.remove("operation");
            object.remove("inputs");
            object.remove("guest_script");
        }
        Ok(setup)
    }

    pub fn binding() -> Value {
        json!({"surface": SURFACE, "permitted_coglets": [OWNER]})
    }

    pub fn write_json(path: &Path, value: &Value) -> Result<(), String> {
        fs::write(
            path,
            serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())
    }

    pub fn engine(setup: &Value) -> Result<NativeEngine, String> {
        let code = hex::decode(
            setup["initial_files"]["/code/bin/step.wasm"]
                .as_str()
                .ok_or("step.wasm")?,
        )
        .map_err(|e| e.to_string())?;
        NativeEngine::new(
            code,
            &setup["manifest"]["budgets"],
            RUNTIME_BYTES,
            STACK_BYTES,
        )
    }

    pub fn prepare(output: &Path, wasm_path: &Path) -> Result<(), String> {
        fs::create_dir_all(output).map_err(|e| e.to_string())?;
        let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
        let wal = output.join("state.wal");
        if output.join("runtime.json").exists() || wal.exists() {
            return Err("refusing to replace existing runtime.json or WAL".into());
        }
        let wasm = fs::read(wasm_path).map_err(|e| e.to_string())?;
        if !wasm.starts_with(b"\0asm") {
            return Err("QUALIFIED_FACE_WASM is not core Wasm".into());
        }
        let setup = face_setup(&wasm)?;
        let runtime = json!({
            "wal": wal,
            "setup": setup,
            "native_schedule": native_schedule()
        });
        write_json(&output.join("binding.json"), &binding())?;
        write_json(&output.join("runtime.json"), &runtime)?;
        println!(
            "{}",
            json!({
                "binding": output.join("binding.json"),
                "runtime": output.join("runtime.json"),
                "wal": wal,
                "owner": OWNER,
                "surface": SURFACE,
                "native_schedule": native_schedule(),
                "wall_time_us": 1_000_000
            })
        );
        Ok(())
    }

    pub fn verify(output: &Path) -> Result<(), String> {
        let output = fs::canonicalize(output).map_err(|e| e.to_string())?;
        let runtime: Value = serde_json::from_slice(
            &fs::read(output.join("runtime.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let setup = runtime.get("setup").ok_or("runtime.json setup")?;
        let wal = Path::new(runtime["wal"].as_str().ok_or("runtime.json wal")?);
        if !wal.is_absolute() {
            return Err("runtime.json wal must be absolute".into());
        }
        if !wal.exists() {
            return Err("missing WAL; run the C ABI FaceHost probe before verify".into());
        }
        let service = RuntimeService::open(wal, setup, 128)?;
        let records = service.records().to_vec();
        drop(service);

        let steps: Vec<&Value> = records.iter().filter(|r| r.get("step").is_some()).collect();
        let real: Vec<&Value> = steps
            .iter()
            .copied()
            .filter(|r| {
                r["status"] == "committed" && r["usage"]["compute"].as_u64().is_some_and(|n| n > 0)
            })
            .collect();
        if real.len() < 3 {
            return Err(format!(
                "need at least 3 real guest steps with positive compute, found {}",
                real.len()
            ));
        }

        let spoken = real.iter().find(|record| {
            record["actions"].as_array().is_some_and(|actions| {
                actions.iter().any(|action| {
                    action["coglet"] == OWNER
                        && action["capability"] == "speech.speak_chunk"
                        && action["args"]["text"] == "Hello world"
                })
            })
        });
        let Some(spoken) = spoken else {
            return Err("no committed Face speak_chunk with text Hello world".into());
        };
        let item = spoken["actions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|action| action["args"]["text"] == "Hello world")
            .ok_or("Hello world action")?["id"]
            .clone();

        let barged = real.iter().find(|record| {
            record["events"].as_array().is_some_and(|events| {
                events
                    .iter()
                    .any(|event| event["kind"] == "speech.barge_in")
            })
        });
        let Some(barged) = barged else {
            return Err("no committed guest step consumed speech.barge_in".into());
        };
        if barged["actions"]
            .as_array()
            .is_some_and(|actions| !actions.is_empty())
        {
            return Err("interrupted output must not replay".into());
        }
        if !barged["boundary"].as_array().is_some_and(|changes| {
            changes
                .iter()
                .any(|change| change["method"] == "runtime.heard")
        }) {
            return Err("barge step missing runtime.heard boundary".into());
        }
        let heard_raw = hex::decode(
            barged["output_files"]["/data/runtime/heard.cbor"]
                .as_str()
                .ok_or("runtime must journal heard state before guest consumption")?,
        )
        .map_err(|e| e.to_string())?;
        let heard: Value = coglet_types::decode("heard-state", &heard_raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        let items = heard["items"].as_array().ok_or("heard items")?;
        if items.len() != 1 {
            return Err(format!("expected 1 heard item, found {}", items.len()));
        }
        if items[0]["item"] != item
            || items[0]["heard"] != "Hello"
            || items[0]["played_ms"] != 1200
            || items[0]["stopped"] != true
            || items[0]["spans"] != json!([{"end_ms":1200,"text":"Hello"}])
        {
            return Err(format!("heard mismatch: {}", items[0]));
        }

        let expected: Vec<String> = records
            .iter()
            .map(|r| {
                r["output_hash"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or("output_hash")
            })
            .collect::<Result<_, _>>()?;
        let hashes = Runtime::from_fixture(setup)?.replay(&records, &mut engine(setup)?)?;
        if hashes != expected {
            return Err("independent NativeEngine replay differs from WAL output hashes".into());
        }
        println!(
            "{}",
            json!({
                "guest_steps": real.len(),
                "hello_world_item": item,
                "heard": items[0],
                "output_hashes": expected
            })
        );
        Ok(())
    }
}

#[cfg(feature = "native-engine")]
const OWNER: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
#[cfg(feature = "native-engine")]
const SURFACE: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

#[cfg(feature = "native-engine")]
fn prepare(output: &Path, wasm: &Path) -> Result<(), String> {
    actual::prepare(output, wasm)
}

#[cfg(feature = "native-engine")]
fn verify(output: &Path) -> Result<(), String> {
    actual::verify(output)
}

#[cfg(feature = "native-engine")]
use std::path::Path;
