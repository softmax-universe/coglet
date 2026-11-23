use crate::{DurableStore, Router};
use crate::{EffectLedger, Result, Snapshot, StepHost, cbor, digest, validate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

/// Engine implementations must start a fresh instance for every attempt. This
/// runtime-owned seam is temporary while coglet-engine's public API is empty.
pub trait Engine {
    fn execute(&mut self, inputs: &Value, host: &mut StepHost) -> Result<EngineReport>;
}

pub struct EngineReport {
    pub usage: Value,
    pub failure: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ServiceState {
    runtime: Runtime,
    router: Router,
}

/// Serial durable coordinator. Admission and consumption commit host metadata
/// together with files/journal; dispatchers may observe only committed state.
pub struct RuntimeService {
    state: ServiceState,
    store: DurableStore,
}
impl RuntimeService {
    pub fn open(path: &Path, setup: &Value, queue_capacity: usize) -> Result<Self> {
        let mut store = DurableStore::open(path)?;
        let configured = Runtime::from_fixture(setup)?;
        let state: ServiceState = if let Some(saved) = store.state() {
            let state: ServiceState =
                serde_json::from_value(saved.clone()).map_err(|e| e.to_string())?;
            if state.runtime.manifest != configured.manifest
                || state.runtime.files.files().get("/code/bin/step.wasm")
                    != configured.files.files().get("/code/bin/step.wasm")
                || state
                    .runtime
                    .files
                    .files()
                    .get("/code/build/provenance.cbor")
                    != configured.files.files().get("/code/build/provenance.cbor")
            {
                return Err("recovery code/manifest pins differ from configuration".into());
            }
            state
        } else {
            let state = ServiceState {
                runtime: configured,
                router: Router::new(queue_capacity)?,
            };
            store.commit(
                &serde_json::to_value(&state).map_err(|e| e.to_string())?,
                None,
            )?;
            state
        };
        Ok(Self { state, store })
    }

    /// The authenticated surface/router must have authorized external events
    /// before calling this internal coordinator seam.
    pub fn enqueue(&mut self, event: Value) -> Result<u64> {
        let mut next = self.state.clone();
        let ordinal = next.router.enqueue(event)?;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(ordinal)
    }

    pub fn run_next(&mut self, context: &Value, engine: &mut impl Engine) -> Result<Option<Value>> {
        let max_events = context["budgets"]["max_events"]
            .as_u64()
            .ok_or("missing max_events")?;
        let events = self
            .state
            .router
            .batch(usize::try_from(max_events).map_err(|_| "max_events overflow")?)?;
        if events.is_empty() {
            return Ok(None);
        }
        let mut next = self.state.clone();
        let mut inputs = context.clone();
        inputs["coglet"] = next.runtime.manifest["id"].clone();
        inputs["step"] = json!(next.runtime.step);
        inputs["previous"] = next.runtime.previous.clone();
        inputs["events"] = json!(events);
        let result = next.runtime.step(&inputs, engine)?;
        next.router.consume(events.len())?;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            Some(&result["journal"]),
        )?;
        self.state = next;
        Ok(Some(result))
    }

    pub fn queued(&self) -> usize {
        self.state.router.len()
    }
    pub fn records(&self) -> &[Value] {
        self.store.records()
    }
}

/// Data-only conformance engine; usage is scripted, NOT real Wasm measurement.
pub struct MockEngine {
    script: Value,
}
impl MockEngine {
    pub fn new(script: Value) -> Self {
        Self { script }
    }
}
impl Engine for MockEngine {
    fn execute(&mut self, _inputs: &Value, host: &mut StepHost) -> Result<EngineReport> {
        for call in self.script["calls"].as_array().ok_or("missing calls")? {
            let raw = if let Some(raw) = call["argument_hex"].as_str() {
                hex::decode(raw).map_err(|e| e.to_string())?
            } else {
                cbor(&call["argument"])?
            };
            let result = host.import(call["op"].as_str().ok_or("missing op")?, &raw)?;
            if call["result"] != result {
                return Err(format!(
                    "script host result mismatch: got {result}, expected {}",
                    call["result"]
                ));
            }
        }
        for (path, bytes) in self.script["writes"].as_object().ok_or("missing writes")? {
            host.guest_write(
                path,
                &hex::decode(bytes.as_str().ok_or("invalid write bytes")?)
                    .map_err(|e| e.to_string())?,
            )?;
        }
        validate("usage", &self.script["usage"])?;
        let failure = match self.script["termination"].as_str() {
            Some("committed") => None,
            Some("aborted") => {
                validate("failure", &self.script["failure"])?;
                Some(self.script["failure"].clone())
            }
            _ => return Err("invalid mock termination".into()),
        };
        Ok(EngineReport {
            usage: self.script["usage"].clone(),
            failure,
        })
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Runtime {
    files: Snapshot,
    manifest: Value,
    step: u64,
    time: u64,
    previous: Value,
    records: Vec<Value>,
    ledger: EffectLedger,
    inbox_wakes: VecDeque<u64>,
    pending_llm: BTreeMap<String, Value>,
}
impl Runtime {
    pub fn from_fixture(setup: &Value) -> Result<Self> {
        validate("manifest", &setup["manifest"])?;
        let files = Snapshot::new(
            serde_json::from_value(setup["initial_files"].clone()).map_err(|e| e.to_string())?,
        )?;
        let code = hex::decode(
            files
                .files()
                .get("/code/bin/step.wasm")
                .ok_or("missing step.wasm")?,
        )
        .map_err(|e| e.to_string())?;
        let provenance = hex::decode(
            files
                .files()
                .get("/code/build/provenance.cbor")
                .ok_or("missing build provenance")?,
        )
        .map_err(|e| e.to_string())?;
        let provenance: Value = coglet_types::decode("hub-build-record", &provenance)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        if provenance["rewritten_sha256"] != digest(&code) {
            return Err("provenance code mismatch".into());
        }
        validate("effect-ledger", &setup["initial_effect_ids"])?;
        let ledger: EffectLedger = serde_json::from_value(setup["initial_effect_ids"].clone())
            .map_err(|e| e.to_string())?;
        let mut pending_llm = BTreeMap::new();
        for request in setup["initial_llm_requests"]
            .as_array()
            .ok_or("missing initial_llm_requests")?
        {
            validate("llm-request", request)?;
            let id = request["id"].as_str().unwrap();
            if !ledger["llm_requests"].contains(id)
                || pending_llm.insert(id.into(), request.clone()).is_some()
            {
                return Err("invalid initial LLM request ledger".into());
            }
        }
        let inbox = crate::boundary::inbox(&files)?;
        let inbox_wakes: VecDeque<u64> =
            serde_json::from_value(setup["initial_inbox_wakes"].clone())
                .map_err(|e| e.to_string())?;
        let mut previous_index = None;
        for index in &inbox_wakes {
            if *index >= inbox.len() as u64
                || previous_index.is_some_and(|previous| previous >= *index)
            {
                return Err("invalid initial inbox wakes".into());
            }
            previous_index = Some(*index);
        }
        Ok(Self {
            files,
            manifest: setup["manifest"].clone(),
            step: 0,
            time: 0,
            previous: Value::Null,
            records: Vec::new(),
            ledger,
            inbox_wakes,
            pending_llm,
        })
    }

    /// Compute an attempted step. No dispatch occurs here. Integration with the
    /// durable store must commit returned state + record before external effects.
    pub fn step(&mut self, inputs: &Value, engine: &mut impl Engine) -> Result<Value> {
        if inputs["coglet"] != self.manifest["id"]
            || inputs["step"] != self.step
            || inputs["previous"] != self.previous
        {
            return Err("step identity/chain mismatch".into());
        }
        let time = inputs["logical_time"]
            .as_u64()
            .ok_or("missing logical_time")?;
        if time < self.time {
            return Err("runtime clock rollback".into());
        }
        validate("budgets", &inputs["budgets"])?;
        let budgets = &inputs["budgets"];
        for key in [
            "compute",
            "memory_pages",
            "wall_time_us",
            "max_events",
            "host_bytes",
        ] {
            if budgets[key].as_u64() > self.manifest["budgets"][key].as_u64() {
                return Err("effective budget exceeds manifest".into());
            }
        }
        if budgets["compute_mode"] != self.manifest["budgets"]["compute_mode"] {
            return Err("compute mode changed".into());
        }
        let events = inputs["events"].as_array().ok_or("missing events")?;
        if events.is_empty() || events.len() as u64 > budgets["max_events"].as_u64().unwrap() {
            return Err("invalid event batch size".into());
        }
        for event in events {
            validate("event", event)?;
        }
        let before_hash = self.files.tree_hash()?;
        let mut input = self.files.clone();
        let mut inbox_wakes = self.inbox_wakes.clone();
        let mut pending_llm = self.pending_llm.clone();
        for change in inputs["boundary"].as_array().ok_or("missing boundary")? {
            let old_count = crate::boundary::inbox(&input)?.len();
            input = crate::boundary::apply(&input, change, &inputs["coglet"])?;
            let messages = crate::boundary::inbox(&input)?;
            let new_count = messages.len();
            for message in &messages[old_count..] {
                if message["event"]["kind"] == "llm.reply" {
                    let id = message["event"]["payload"]["request_id"].as_str().unwrap();
                    let request = pending_llm
                        .remove(id)
                        .ok_or("LLM reply has no committed pending request")?;
                    if request["correlation_id"] != message["event"]["correlation_id"] {
                        return Err("LLM reply correlation mismatch".into());
                    }
                }
            }
            inbox_wakes.extend((old_count as u64)..(new_count as u64));
        }
        let inbox = crate::boundary::inbox(&input)?;
        for event in events {
            if event["kind"] == "runtime.inbox" {
                let index = event["payload"]["index"]
                    .as_u64()
                    .ok_or("invalid inbox wake index")?;
                if inbox_wakes.pop_front() != Some(index) {
                    return Err("inbox wake absent or out of order".into());
                }
                let message = &inbox[index as usize];
                if event["source"] != inputs["coglet"]
                    || event["payload"] != json!({"index":index,"id":message["id"]})
                    || event["correlation_id"] != message["event"]["correlation_id"]
                {
                    return Err("inbox wake mismatch".into());
                }
            }
        }
        let mut host = StepHost::new(input.clone(), self.ledger.clone(), inputs);
        let mut report = engine.execute(inputs, &mut host)?;
        validate("usage", &report.usage)?;
        if report.failure.is_none() {
            for (field, kind) in [
                ("compute", "compute"),
                ("memory_pages", "memory"),
                ("wall_time_us", "wall"),
                ("host_bytes", "host_bytes"),
            ] {
                if report.usage[field].as_u64().unwrap() > budgets[field].as_u64().unwrap() {
                    report.failure = Some(json!({"kind":kind,"error":{
                        "code":if kind == "wall" { "deadline_exceeded" } else { "resource_exhausted" },
                        "message":format!("step {field} budget exhausted"),"retryable":false}}));
                    break;
                }
            }
        }
        let committed = report.failure.is_none();
        let output = if committed {
            host.files.clone()
        } else {
            input.clone()
        };
        let mut record = json!({
            "version":"0.1", "trace_id":inputs["trace_id"], "coglet":inputs["coglet"],
            "step":self.step,"logical_time":time,"seed":inputs["seed"],"previous":self.previous,
            "code_hash":digest(&hex::decode(&self.files.files()["/code/bin/step.wasm"]).map_err(|e| e.to_string())?),
            "manifest_hash":digest(&cbor(&self.manifest)?),"before_hash":before_hash,
            "input_hash":input.tree_hash()?,"output_hash":output.tree_hash()?,
            "engine":inputs["engine"],"budgets":budgets,"grants":inputs["grants"],
            "events":events,"boundary":inputs["boundary"],"calls":host.calls,"actions":if committed { host.actions.clone() } else { vec![] },
            "timers":if committed { host.timers.clone() } else { vec![] },
            "llm_requests":if committed { host.llm_requests.clone() } else { vec![] },
            "logs":if committed { host.logs.clone() } else { vec![] },
            "input_files":input.files(),"output_files":output.files(),"usage":report.usage,
            "status":if report.failure.is_some() { "aborted" } else { "committed" }
        });
        if let Some(failure) = report.failure {
            record["failure"] = failure;
        }
        record["record_hash"] = Value::String(digest(&cbor(&record)?));
        validate("step-record", &record)?;
        let result = json!({"state_hash":record["output_hash"],"record_hash":record["record_hash"],"journal":record});
        self.previous = record["record_hash"].clone();
        self.files = output;
        self.inbox_wakes = inbox_wakes;
        if committed {
            self.ledger = host.ledger;
            for request in host.llm_requests {
                pending_llm.insert(request["id"].as_str().unwrap().into(), request);
            }
        }
        self.pending_llm = pending_llm;
        self.time = time;
        self.step = self.step.checked_add(1).ok_or("step counter exhausted")?;
        self.records.push(record);
        Ok(result)
    }

    pub fn records(&self) -> &[Value] {
        &self.records
    }

    /// Re-execute a journal with a pinned engine/program. Only recorded step
    /// inputs are passed to the engine, NEVER output files or recorded returns.
    /// This has no dispatch or live-provider/surface callback path.
    pub fn replay(&mut self, records: &[Value], engine: &mut impl Engine) -> Result<Vec<String>> {
        let mut hashes = Vec::new();
        for record in records {
            validate("step-record", record)?;
            let mut unhashed = record.clone();
            unhashed.as_object_mut().unwrap().remove("record_hash");
            if record["record_hash"] != digest(&cbor(&unhashed)?) {
                return Err("replay record checksum mismatch".into());
            }
            let mut inputs = serde_json::Map::new();
            for key in [
                "trace_id",
                "coglet",
                "step",
                "logical_time",
                "seed",
                "previous",
                "engine",
                "budgets",
                "grants",
                "events",
                "boundary",
            ] {
                inputs.insert(key.into(), record[key].clone());
            }
            let result = self.step(&Value::Object(inputs), engine)?;
            if result["journal"] != *record {
                return Err(format!("replay mismatch at step {}", record["step"]));
            }
            hashes.push(result["state_hash"].as_str().unwrap().into());
        }
        Ok(hashes)
    }
}
