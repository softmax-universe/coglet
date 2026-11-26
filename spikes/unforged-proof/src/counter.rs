//! The one step function both runners share: real `coglets/counter` code, executed
//! by coglet's own `Runtime::step` with a fresh WAMR `NativeEngine`.
//!
//! The hot path calls [`wamr_step`] in process; the cold path calls it inside
//! `coglet-wamr-step`, the helper binary pinned in the unforged resolver tree. Same
//! function, same engine build, so the same input must give the same tree.
use coglet_runtime::{NativeEngine, Runtime};
use gecaos::coglet::{BoundaryChange, CogletEvent, StepContext, sha256_hex};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;

/// `CogletTree` path of the coglet manifest. ADR-0035 snapshots only admit
/// `code/`, `data/` and `logs/`, so the manifest lives under host-owned `code/`.
pub const MANIFEST_PATH: &str = "code/manifest.json";
pub const STEP_WASM_PATH: &str = "code/bin/step.wasm";
/// Event source for the counter's `test.echo` events.
pub const SURFACE: &str =
    "surface:5151515151515151515151515151515151515151515151515151515151515151";
/// Native engine limits, identical to `coglet-cli`'s.
const RUNTIME_BYTES: u32 = 16 * 1024 * 1024;
const STACK_BYTES: u32 = 64 * 1024;

pub type Files = BTreeMap<String, Vec<u8>>;

#[derive(Debug, Clone)]
pub struct StepOutcome {
    pub files: Files,
    pub aborted: bool,
    pub reason: Option<String>,
    pub compute: u64,
}

/// Build an unsigned local package from the real `coglet_counter.wasm` with
/// `coglet_cli::build` (R3 canonicalization + provenance), then load its setup.
pub fn build_package(
    wasm: &Path,
    manifest: &Path,
    source_commit: &str,
    work: &Path,
) -> Result<Value, String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let recipe = work.join("recipe");
    let policy = work.join("policy");
    std::fs::write(
        &recipe,
        "cargo rustc --locked --manifest-path coglets/counter/Cargo.toml \
         --target wasm32-wasip1 --release --lib -- -C link-arg=--max-memory=16777216\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(&policy, "unsigned local proof spike; no dispatch\n")
        .map_err(|e| e.to_string())?;
    let package = work.join("package");
    let _ = std::fs::remove_dir_all(&package);
    coglet_cli::build(wasm, manifest, source_commit, &recipe, &policy, &package)
        .map_err(|e| format!("coglet_cli::build: {e:#}"))?;
    coglet_cli::package_setup(&package).map_err(|e| format!("coglet_cli::package_setup: {e:#}"))
}

/// The coglet's genesis `CogletTree`: the package's initial files plus the manifest.
pub fn genesis_files(setup: &Value) -> Result<Files, String> {
    let mut files = from_hex_map(&setup["initial_files"])?;
    files.insert(
        MANIFEST_PATH.to_string(),
        serde_json::to_vec(&setup["manifest"]).map_err(|e| e.to_string())?,
    );
    Ok(files)
}

fn from_hex_map(v: &Value) -> Result<Files, String> {
    let map = v.as_object().ok_or("expected a path -> hex map")?;
    let mut files = Files::new();
    for (path, hex) in map {
        let bytes =
            hex::decode(hex.as_str().ok_or("non-string hex")?).map_err(|e| e.to_string())?;
        files.insert(path.trim_start_matches('/').to_string(), bytes);
    }
    Ok(files)
}

/// Apply one boundary change to `files` (manage writes before the step).
fn apply_boundary(files: &mut Files, change: &BoundaryChange) -> Result<(), String> {
    let data_only = |p: &str| -> Result<String, String> {
        let clean = p.trim_start_matches('/').to_string();
        if clean.starts_with("data/") {
            Ok(clean)
        } else {
            Err(format!("boundary change outside data/: `{p}`"))
        }
    };
    match change {
        BoundaryChange::Write { path, data } => {
            files.insert(data_only(path)?, data.clone());
        }
        BoundaryChange::Append { path, data } => {
            files
                .entry(data_only(path)?)
                .or_default()
                .extend_from_slice(data);
        }
        BoundaryChange::Delete { path } => {
            files.remove(&data_only(path)?);
        }
    }
    Ok(())
}

/// One deterministic coglet step: `ours` (`CogletTree`) + one input -> next `CogletTree`.
///
/// The guest runs under coglet's real `Runtime::step` + WAMR. Each call is an
/// independent single-step runtime rooted at `ours` (coglet journal step 0); the
/// unforged step number lives in the `StepCommit` trailer, not in the guest.
/// A failed step returns `ours` with `out/` removed and boundary writes undone.
pub fn wamr_step(
    ours: &Files,
    ctx: &StepContext,
    events: &[CogletEvent],
    boundary: &[BoundaryChange],
) -> Result<StepOutcome, String> {
    let ours_wo_out: Files = ours
        .iter()
        .filter(|(p, _)| !p.starts_with("out/"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    let aborted = |reason: String, compute: u64| StepOutcome {
        files: ours_wo_out.clone(),
        aborted: true,
        reason: Some(reason),
        compute,
    };
    let mut working = ours_wo_out.clone();
    for change in boundary {
        apply_boundary(&mut working, change)?;
    }
    let manifest: Value = serde_json::from_slice(
        working
            .get(MANIFEST_PATH)
            .ok_or("CogletTree missing code/manifest.json")?,
    )
    .map_err(|e| e.to_string())?;
    let code = working
        .get(STEP_WASM_PATH)
        .ok_or("CogletTree missing code/bin/step.wasm")?
        .clone();
    let initial: serde_json::Map<String, Value> = working
        .iter()
        .filter(|(p, _)| p.as_str() != MANIFEST_PATH)
        .map(|(p, b)| (format!("/{p}"), json!(hex::encode(b))))
        .collect();
    let setup = json!({"manifest": manifest, "initial_files": initial,
        "initial_effect_ids": {"actions": [], "timers": [], "llm_requests": []},
        "initial_actions": [], "initial_timers": [], "initial_llm_requests": [],
        "initial_inbox_wakes": []});
    let mut runtime = Runtime::from_fixture(&setup)?;
    let mut engine = NativeEngine::new(code, &manifest["budgets"], RUNTIME_BYTES, STACK_BYTES)?;
    let coglet = manifest["id"].clone();
    let trace_id =
        sha256_hex(format!("coglet-unforged-proof:{coglet}").as_bytes())[..32].to_string();
    let batch: Vec<Value> = events
        .iter()
        .enumerate()
        .map(|(i, e)| {
            json!({"source": SURFACE, "kind": e.kind, "seq": ctx.step * 1000 + i as u64,
                "logical_time": ctx.logical_time,
                "correlation_id": &sha256_hex(e.id.as_bytes())[..32], "payload": {}})
        })
        .collect();
    let inputs = json!({"trace_id": trace_id, "coglet": coglet, "step": 0,
        "logical_time": ctx.logical_time, "seed": ctx.seed, "previous": null,
        "budgets": manifest["budgets"], "grants": ctx.grants, "events": batch, "boundary": []});
    let record = match runtime.step(&inputs, &mut engine) {
        Ok(r) => r,
        // Runtime refused the step before guest execution (e.g. empty batch).
        Err(e) => return Ok(aborted(format!("runtime refused step: {e}"), 0)),
    };
    let journal = if record.get("journal").is_some() {
        &record["journal"]
    } else {
        &record
    };
    let compute = journal["usage"]["compute"].as_u64().unwrap_or(0);
    if journal["status"] != "committed" {
        let reason = journal["failure"].to_string();
        return Ok(aborted(reason, compute));
    }
    let state = serde_json::to_value(&runtime).map_err(|e| e.to_string())?;
    let mut files = from_hex_map(&state["files"])?;
    files.insert(MANIFEST_PATH.to_string(), working[MANIFEST_PATH].clone());
    Ok(StepOutcome {
        files,
        aborted: false,
        reason: None,
        compute,
    })
}

/// Decode an unforged `InputTree` (`context.cbor`, `events.cbor`, `boundary.cbor`).
pub fn decode_input(
    theirs: &Files,
) -> Result<(StepContext, Vec<CogletEvent>, Vec<BoundaryChange>), String> {
    let ctx = StepContext::from_bytes(
        theirs
            .get("context.cbor")
            .ok_or("InputTree missing context.cbor")?,
    )?;
    let events = match theirs.get("events.cbor") {
        Some(b) => CogletEvent::decode_batch(b)?,
        None => Vec::new(),
    };
    let boundary = match theirs.get("boundary.cbor") {
        Some(b) => BoundaryChange::decode_batch(b)?,
        None => Vec::new(),
    };
    Ok((ctx, events, boundary))
}

/// Encode an unforged `InputTree` for one step.
pub fn encode_input(
    ctx: &StepContext,
    events: &[CogletEvent],
    boundary: &[BoundaryChange],
) -> Files {
    let mut files = Files::new();
    files.insert("context.cbor".into(), ctx.to_cbor_bytes());
    files.insert("events.cbor".into(), CogletEvent::encode_batch_cbor(events));
    if !boundary.is_empty() {
        files.insert(
            "boundary.cbor".into(),
            BoundaryChange::encode_batch_cbor(boundary),
        );
    }
    files
}

/// `StepContext` for step `n` of the proof (logical time = step, fixed seed rule).
pub fn context(step: u64) -> StepContext {
    StepContext {
        step,
        logical_time: step,
        seed: step.wrapping_mul(0x9e37_79b9_7f4a_7c15),
        grants: Vec::new(),
        fuel_limit: 10_000_000,
        mem_limit_bytes: 16 * 1024 * 1024,
    }
}

/// `n` `test.echo` events for step `step`.
pub fn echo_events(step: u64, n: usize) -> Vec<CogletEvent> {
    (0..n)
        .map(|i| CogletEvent::new(format!("evt-{step}-{i}"), "test.echo", Vec::new()))
        .collect()
}
