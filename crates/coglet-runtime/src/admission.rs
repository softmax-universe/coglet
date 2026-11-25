//! Manifest admission against an explicitly supplied trusted catalog and current
//! authorization snapshot. Transport authentication/lease freshness is the caller's
//! responsibility; this never instantiates a guest.
use crate::{Result, validate};
use serde_json::Value;
#[cfg(feature = "native-engine")]
use serde_json::json;
#[cfg(feature = "native-engine")]
use std::time::{Duration, Instant};

#[cfg(not(feature = "native-engine"))]
pub fn admit_module(_: &Value) -> Result<Value> {
    Err("module admission requires native-engine; no simulated instantiation claim".into())
}

/// Test adapter admission through the actual WAMR loader and instance seam.
/// fixture.interpreter explicitly selects WAMR's interpreter for this admission
/// probe only; no handle/initialization/guest attempt occurs, no input is consumed.
#[cfg(feature = "native-engine")]
pub fn admit_module(request: &Value) -> Result<Value> {
    use coglet_engine::Engine;
    if request["manifest"]["version"] == "1.1" {
        validate("database::db-manifest", &request["manifest"])?;
        return Err(
            "daemon database admission requires authenticated Database/Catalog context; not a deployable boundary"
                .into(),
        );
    }
    let engine = coglet_engine_wamr::Wamr::interpreter();
    validate("manifest", &request["manifest"])?;
    validate("budgets", &request["budgets"])?;
    for field in [
        "compute",
        "memory_pages",
        "wall_time_us",
        "max_events",
        "host_bytes",
    ] {
        if request["budgets"][field].as_u64() > request["manifest"]["budgets"][field].as_u64() {
            return Err("effective budget exceeds manifest".into());
        }
    }
    if request["budgets"]["compute_mode"] != request["manifest"]["budgets"]["compute_mode"] {
        return Err("compute mode changed".into());
    }
    for event in request["pending_events"]
        .as_array()
        .ok_or("pending events")?
    {
        validate("event", event)?;
    }
    let files = crate::Snapshot::new(
        serde_json::from_value(request["initial_files"].clone()).map_err(|e| e.to_string())?,
    )?;
    let code = hex::decode(
        files
            .files()
            .get("/code/bin/step.wasm")
            .ok_or("missing code")?,
    )
    .map_err(|e| e.to_string())?;
    let raw = hex::decode(
        files
            .files()
            .get("/code/build/provenance.cbor")
            .ok_or("missing provenance")?,
    )
    .map_err(|e| e.to_string())?;
    let provenance: Value = coglet_types::decode("hub-build-record", &raw)?
        .deserialized()
        .map_err(|e| e.to_string())?;
    if provenance["rewritten_sha256"] != crate::digest(&code) {
        return Err("provenance code mismatch".into());
    }
    let budget = &request["budgets"];
    let mode = budget["compute_mode"].as_str().unwrap();
    if mode != "fixture.interpreter" && mode != engine.metadata().compute_mode {
        return Err("unsupported compute mode".into());
    }
    let limits = coglet_engine::Limits {
        compute_mode: engine.metadata().compute_mode.into(),
        compute: budget["compute"].as_u64().unwrap(),
        memory_pages: u32::try_from(budget["memory_pages"].as_u64().unwrap())
            .map_err(|_| "memory cap overflow")?,
        host_bytes: budget["host_bytes"].as_u64().unwrap(),
        runtime_bytes: 16 * 1024 * 1024,
        stack_bytes: 64 * 1024,
        deadline: Instant::now()
            .checked_add(Duration::from_micros(
                budget["wall_time_us"].as_u64().unwrap(),
            ))
            .ok_or("deadline overflow")?,
    };
    let admitted = engine.load(&code, &limits);
    let mut instance = None;
    let error = match admitted {
        Ok(module) => {
            instance = Some(
                engine
                    .instantiate(&module, limits.clone())
                    .map_err(|e| e.to_string())?,
            );
            None
        }
        Err(coglet_engine::Error::MemoryExhausted) => Some(
            json!({"code":"resource_exhausted","message":"module memory exceeds granted cap","retryable":false}),
        ),
        Err(error) => return Err(error.to_string()),
    };
    let mut response = json!({"admitted":instance.is_some(),"instantiated":instance.is_some(),"handle_calls":0,
        "effective_memory_pages":limits.memory_pages,"pending_events":request["pending_events"],
        "journal":null,"record_hash":null,"state_hash":files.tree_hash()?});
    if let Some(error) = error {
        response["error"] = error;
    }
    Ok(response)
}

use std::collections::BTreeSet;

pub fn admit_manifest(manifest: &Value, catalog: &Value, grant: &Value) -> Result<()> {
    if manifest.get("version") == Some(&serde_json::json!("1.1")) {
        validate("database::db-manifest", manifest)?;
        return Err(
            "daemon database admission requires authenticated Database/Catalog context; not a deployable boundary"
                .into(),
        );
    }
    validate("manifest", manifest)?;
    let mut events = BTreeSet::new();
    let mut operations = BTreeSet::new();
    let mut resources = BTreeSet::new();
    for entry in catalog.as_array().ok_or("invalid catalog")? {
        for field in ["events", "reads", "actions", "exclusive"] {
            let target = match field {
                "events" => &mut events,
                "exclusive" => &mut resources,
                _ => &mut operations,
            };
            for name in entry[field].as_array().ok_or("invalid catalog entry")? {
                let name = name.as_str().ok_or("invalid catalog name")?;
                validate("capability-name", &Value::String(name.into()))?;
                target.insert(name.to_owned());
            }
        }
    }
    operations.extend(events.iter().cloned());
    let same_owner = grant["coglet"] == manifest["id"];
    if !same_owner {
        return Err("missing operation grant".into());
    }
    for name in manifest["subscriptions"].as_array().unwrap() {
        if !events.contains(name.as_str().unwrap()) {
            return Err("unknown event".into());
        }
    }
    for name in manifest["required_capabilities"].as_array().unwrap() {
        if !operations.contains(name.as_str().unwrap()) {
            return Err("unknown operation".into());
        }
    }
    for name in manifest["exclusive_outputs"].as_array().unwrap() {
        if !resources.contains(name.as_str().unwrap()) {
            return Err("unknown resource".into());
        }
    }
    for field in ["required_capabilities", "subscriptions"] {
        for name in manifest[field].as_array().unwrap() {
            if !same_owner
                || !grant["operations"]
                    .as_array()
                    .is_some_and(|names| names.contains(name))
            {
                return Err("missing operation grant".into());
            }
        }
    }
    for name in manifest["exclusive_outputs"].as_array().unwrap() {
        if !same_owner
            || !grant["resources"]
                .as_array()
                .is_some_and(|names| names.contains(name))
        {
            return Err("missing resource lease".into());
        }
    }
    Ok(())
}
