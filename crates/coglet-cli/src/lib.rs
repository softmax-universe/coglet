//! Local packaging, sandbox execution and independent native journal replay.
//! These do not sign hub publications or grant remote deployment authority.
use anyhow::{Context, Result, ensure};
use coglet_runtime::{NativeEngine, Runtime, cbor, digest, validate};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::Path,
    process::Command,
};

pub mod enrollment;
pub mod operator;
pub mod process;
#[rustfmt::skip]
#[path = "../../../spikes/cross-engine-hash/host-wasmtime/src/nancanon.rs"]
mod nancanon;

pub const MAX_FILE: u64 = 64 * 1024 * 1024;
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_checked(path, limit, false)
}
pub fn read_private(path: &Path, limit: u64) -> Result<Vec<u8>> {
    read_checked(path, limit, true)
}
fn read_checked(path: &Path, limit: u64, private: bool) -> Result<Vec<u8>> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    let metadata = file.metadata()?;
    ensure!(metadata.is_file(), "not a regular file: {}", path.display());
    if private {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            ensure!(
                metadata.mode() & 0o077 == 0 && metadata.uid() == unsafe { libc::geteuid() },
                "secret file must be owned by this user and private (chmod 600)"
            );
        }
    }
    let mut raw = Vec::new();
    file.take(limit + 1).read_to_end(&mut raw)?;
    ensure!(
        raw.len() as u64 <= limit,
        "file exceeds {limit} bytes: {}",
        path.display()
    );
    Ok(raw)
}
pub fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&read(path, MAX_FILE)?)
        .with_context(|| format!("JSON {}", path.display()))
}
pub fn checked(rule: &str, v: &Value) -> Result<()> {
    validate(rule, v)
        .map_err(anyhow::Error::msg)
        .with_context(|| format!("invalid {rule}"))
}
pub fn private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist_noclobber(path)
        .with_context(|| format!("publish {} (will not overwrite)", path.display()))?;
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}
fn native(code: Vec<u8>, manifest: &Value) -> Result<NativeEngine> {
    NativeEngine::new(code, &manifest["budgets"], 16 * 1024 * 1024, 64 * 1024)
        .map_err(anyhow::Error::msg)
}

/// Publish an unsigned local sandbox package, never a signed hub artifact.
/// source_commit/recipe/policy are mandatory, not synthesized provenance.
pub fn build(
    wasm: &Path,
    manifest: &Path,
    source_commit: &str,
    recipe: &Path,
    policy: &Path,
    output: &Path,
) -> Result<Value> {
    let manifest = read_json(manifest)?;
    checked("manifest", &manifest)?;
    checked("hub-oid", &json!(source_commit))?;
    ensure!(!output.exists(), "output already exists");
    let original = read(wasm, 4 * 1024 * 1024)?;
    let recipe = read(recipe, 1024 * 1024)?;
    let policy = read(policy, 1024 * 1024)?;
    wasmparser::Validator::new()
        .validate_all(&original)
        .context("invalid Wasm")?;
    let (code, _) = nancanon::canonicalize(&original)?;
    ensure!(
        code.len() <= 4 * 1024 * 1024,
        "rewritten module exceeds 4 MiB"
    );
    // Independently validate the rewrite and the actual engine's ABI/budget policy.
    native(code.clone(), &manifest)?;
    let provenance = json!({"version":"0.1", "source_commit":source_commit,
        "recipe_sha256":digest(&recipe),"policy_sha256":digest(&policy),
        "transform_sha256":digest(include_bytes!("../../../spikes/cross-engine-hash/host-wasmtime/src/nancanon.rs")),
        "original_sha256":digest(&original),"rewritten_sha256":digest(&code)});
    checked("hub-build-record", &provenance)?;
    // Stage complete, fsynced pins before reserving a new destination. Errors
    // clean our partial directory. A crash may leave an incomplete package;
    // readers require every pin and never interpret it as successful publication.
    let parent = output
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let stage = tempfile::Builder::new()
        .prefix(".coglet-build-")
        .tempdir_in(parent)?;
    private_file(&stage.path().join("step.wasm"), &code)?;
    private_file(
        &stage.path().join("manifest.json"),
        &serde_json::to_vec(&manifest)?,
    )?;
    private_file(
        &stage.path().join("provenance.cbor"),
        &cbor(&provenance).map_err(anyhow::Error::msg)?,
    )?;
    // persist_noclobber applies to files, not directories. Reserve the destination
    // with create_dir, then rename each entry; readers require all three pins.
    fs::create_dir(output)?;
    let publication = (|| -> Result<()> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(output, fs::Permissions::from_mode(0o700))?;
        }
        for name in ["step.wasm", "manifest.json", "provenance.cbor"] {
            fs::rename(stage.path().join(name), output.join(name))?;
        }
        fs::File::open(output)?.sync_all()?;
        fs::File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if publication.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    publication?;
    Ok(json!({"coglet":manifest["id"],"code_hash":digest(&code),"package":output,"signed":false}))
}

pub fn package_setup(package: &Path) -> Result<Value> {
    let manifest = read_json(&package.join("manifest.json"))?;
    checked("manifest", &manifest)?;
    let code = read(&package.join("step.wasm"), 4 * 1024 * 1024)?;
    let provenance = read(&package.join("provenance.cbor"), 1024 * 1024)?;
    let setup = json!({"manifest":manifest,"initial_files":{
        "/code/bin/step.wasm":hex::encode(&code),
        "/code/build/provenance.cbor":hex::encode(provenance),
        "/data/inbox.jsonl":""},
        "initial_effect_ids":{"actions":[],"timers":[],"llm_requests":[]},
        "initial_actions":[],"initial_timers":[],"initial_llm_requests":[],"initial_inbox_wakes":[]});
    Runtime::from_fixture(&setup).map_err(anyhow::Error::msg)?;
    native(code, &setup["manifest"])?;
    Ok(setup)
}

/// Explicit local supervisor authorization. Missing catalog/grants deny all.
pub fn sandbox_job(
    setup: Value,
    batches: Value,
    catalog: Value,
    grant: Value,
    trace_id: &str,
    seed: u64,
) -> Result<Value> {
    checked("manifest", &setup["manifest"])?;
    // Local sandbox grants are explicit timeless snapshots, not network leases.
    // Unknown fields (including an accidentally supplied expires) fail closed.
    let fields = grant.as_object().context("invalid sandbox grant")?;
    ensure!(
        fields.len() == 3
            && fields
                .keys()
                .all(|k| ["coglet", "operations", "resources"].contains(&k.as_str())),
        "grant requires exactly coglet, operations, resources"
    );
    for (field, manifest_fields) in [
        (
            "operations",
            &["subscriptions", "required_capabilities"][..],
        ),
        ("resources", &["exclusive_outputs"][..]),
    ] {
        let mut seen = std::collections::BTreeSet::new();
        for name in grant[field]
            .as_array()
            .context("invalid sandbox grant names")?
        {
            checked("capability-name", name)?;
            ensure!(
                seen.insert(name.as_str().unwrap()),
                "duplicate sandbox grant"
            );
            ensure!(
                manifest_fields.iter().any(|f| setup["manifest"][*f]
                    .as_array()
                    .is_some_and(|a| a.contains(name))),
                "sandbox grant exceeds manifest"
            );
        }
    }
    coglet_runtime::admit_manifest(&setup["manifest"], &catalog, &grant)
        .map_err(anyhow::Error::msg)?;
    checked("correlation-id", &json!(trace_id))?;
    let batches = batches
        .as_array()
        .context("events must be an array of nonempty event batches")?;
    ensure!(
        !batches.is_empty() && batches.len() <= 1024,
        "requires 1..1024 batches"
    );
    let mut time = 0;
    let mut inputs = Vec::new();
    for (step, events) in batches.iter().enumerate() {
        let events = events.as_array().context("event batch must be an array")?;
        ensure!(
            !events.is_empty()
                && events.len() as u64
                    <= setup["manifest"]["budgets"]["max_events"].as_u64().unwrap(),
            "invalid event batch size"
        );
        for event in events {
            checked("event", event)?;
            let kind = event["kind"].as_str().unwrap();
            // Runtime-owned wakes need retained effects; only explicit debug_step
            // may be synthesized in this fresh offline sandbox.
            ensure!(
                kind == "runtime.debug_step"
                    || (!kind.starts_with("runtime.") && kind != "action.outcome"),
                "runtime/provider boundary wakes require a full archived coordinator"
            );
            if kind == "runtime.debug_step" {
                ensure!(
                    event["source"] == setup["manifest"]["id"] && event["payload"] == json!({}),
                    "invalid sandbox debug wake"
                );
            } else {
                ensure!(
                    setup["manifest"]["subscriptions"]
                        .as_array()
                        .unwrap()
                        .contains(&event["kind"]),
                    "unsubscribed sandbox event"
                );
                ensure!(
                    grant["operations"]
                        .as_array()
                        .is_some_and(|g| g.contains(&event["kind"])),
                    "ungranted sandbox event"
                );
            }
            time = time.max(event["logical_time"].as_u64().unwrap());
        }
        inputs.push(json!({"trace_id":trace_id,"coglet":setup["manifest"]["id"],"step":step,"logical_time":time,"seed":seed.wrapping_add(step as u64),
            "previous":null,"budgets":setup["manifest"]["budgets"],"grants":grant["operations"],"events":events,"boundary":[]}));
    }
    Ok(json!({"setup":setup,"inputs":inputs}))
}

pub fn run_daemon(daemon: &Path, job: &Value) -> Result<Value> {
    let raw = serde_json::to_vec(job)?;
    ensure!(
        raw.len() <= 32 * 1024 * 1024,
        "sandbox request exceeds 32 MiB"
    );
    let mut command = Command::new(daemon);
    command.arg("--sandbox-stdio");
    let raw = process::run(
        command,
        raw,
        std::time::Duration::from_secs(300),
        MAX_FILE as usize,
    )?;
    let trace: Value = serde_json::from_slice(&raw).context("cogletd trace JSON")?;
    Runtime::import_trace(&trace).map_err(anyhow::Error::msg)?;
    ensure!(
        trace["trace_id"] == job["inputs"][0]["trace_id"]
            && trace["coglet"] == job["setup"]["manifest"]["id"],
        "daemon trace identity mismatch"
    );
    ensure!(
        trace["manifest_hash"]
            == digest(&cbor(&job["setup"]["manifest"]).map_err(anyhow::Error::msg)?),
        "daemon manifest pin mismatch"
    );
    let code = hex::decode(
        job["setup"]["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .context("job code")?,
    )?;
    ensure!(
        trace["code_hash"] == digest(&code),
        "daemon code pin mismatch"
    );
    for key in [
        "initial_files",
        "initial_effect_ids",
        "initial_actions",
        "initial_timers",
        "initial_llm_requests",
        "initial_inbox_wakes",
    ] {
        ensure!(
            trace[key] == job["setup"][key],
            "daemon initial state mismatch: {key}"
        );
    }
    let records = trace["records"].as_array().context("trace records")?;
    let inputs = job["inputs"].as_array().context("job inputs")?;
    ensure!(
        records.len() == inputs.len() && trace["trailing_boundary"].is_null(),
        "daemon trace length/boundary mismatch"
    );
    for (record, input) in records.iter().zip(inputs) {
        for key in [
            "trace_id",
            "coglet",
            "step",
            "logical_time",
            "seed",
            "budgets",
            "grants",
            "events",
            "boundary",
        ] {
            ensure!(
                record[key] == input[key],
                "daemon recorded input mismatch: {key}"
            );
        }
    }
    Ok(trace)
}

/// Replay uses independently supplied package code, not archived outputs.
pub fn replay(package: &Path, trace: &Value) -> Result<Value> {
    let setup = package_setup(package)?;
    Runtime::import_trace(trace).map_err(anyhow::Error::msg)?;
    let code = hex::decode(
        setup["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .unwrap(),
    )?;
    ensure!(
        trace["code_hash"] == digest(&code),
        "replay package code hash mismatch"
    );
    ensure!(
        trace["manifest_hash"] == digest(&cbor(&setup["manifest"]).map_err(anyhow::Error::msg)?),
        "replay package manifest mismatch"
    );
    let mut initial = trace.clone();
    initial["manifest"] = setup["manifest"].clone();
    let mut records = trace["records"]
        .as_array()
        .context("trace records")?
        .clone();
    if !trace["trailing_boundary"].is_null() {
        records.push(trace["trailing_boundary"].clone());
    }
    let hashes = Runtime::from_fixture(&initial)
        .map_err(anyhow::Error::msg)?
        .replay(&records, &mut native(code, &setup["manifest"])?)
        .map_err(anyhow::Error::msg)?;
    let last = hashes
        .last()
        .cloned()
        .unwrap_or_else(|| trace["initial_hash"].as_str().unwrap().into());
    ensure!(trace["final_hash"] == last, "replay final hash mismatch");
    Ok(
        json!({"replayed":true,"steps":trace["records"].as_array().unwrap().len(),"state_hashes":hashes,"final_hash":last}),
    )
}

pub fn inspect(trace: &Value) -> Result<Value> {
    Runtime::import_trace(trace).map_err(anyhow::Error::msg)?;
    Ok(
        json!({"trace_id":trace["trace_id"],"coglet":trace["coglet"],"code_hash":trace["code_hash"],"steps":trace["records"].as_array().unwrap().len(),"final_hash":trace["final_hash"],"state":trace["records"].as_array().unwrap().last().map(|r| &r["status"])}),
    )
}
