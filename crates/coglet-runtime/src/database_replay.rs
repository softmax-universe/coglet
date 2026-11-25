//! Offline replay of exact ABI 0.2 database records (ADR-0013).
//!
//! The pinned guest is re-executed against the RECORDED native call stream. The
//! type surface deliberately has no database, catalog, cursor or journal: a
//! `db` import is answered from the next recorded `db-host-call` (exact request
//! bytes, recorded result code, recorded reply bytes), never from live state.
//! Ordinary imports (`emit`, `timer`, `llm`, `log`) are re-executed by the
//! normal host and compared with the recorded native call value. The call
//! order of the mixed stream is enforced; any divergence is sticky and fails
//! the whole replay even when the guest ignores the error it was handed.
//!
//! A recorded `db_recovery_abort` is a host decision, not a guest outcome. It is
//! applied only after the guest reproduced a successful `handle` == 0 with the
//! complete recorded calls and an unpoisoned unit.
use super::*;
use crate::journal::database_projection;
use coglet_engine::{
    Engine as SharedEngine, Error as EngineError, GuestMemory, Host, Instance, Limits, StepBudget,
    StepInput,
};
use coglet_types::Value as V;
use std::time::{Duration, Instant};

const REQUEST_RULE: &str = "database::db-request";
const REPLY_RULE: &str = "database::db-reply";
const RECORD_RULE: &str = "database::db-step-record";

fn get<'a>(value: &'a V, key: &str) -> Option<&'a V> {
    value
        .as_map()?
        .iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
}

fn uint(value: &V, key: &str) -> Result<u64> {
    let integer = get(value, key)
        .and_then(V::as_integer)
        .ok_or_else(|| format!("recorded {key} missing"))?;
    u64::try_from(integer).map_err(|_| format!("recorded {key} out of range"))
}

/// Recorded observation pins. Replay never consults the current schema/catalog.
struct Pins {
    db_before: u64,
    db_after: u64,
    schema_version: u64,
    catalog_hash: String,
}

/// The mixed ordinary/database call stream, consumed strictly in order.
struct ReplayHost<'h> {
    ordinary: &'h mut StepHost,
    expected: &'h [V],
    next: usize,
    poisoned: bool,
    diverged: Option<String>,
    wasi: crate::wasi::Wasi,
}
impl ReplayHost<'_> {
    fn diverge(&mut self, why: impl Into<String>) -> EngineError {
        let why = why.into();
        let error = EngineError::Trap(format!("replay divergence: {why}"));
        self.diverged.get_or_insert(why);
        error
    }
    fn recorded(&mut self, op: &str) -> coglet_engine::Result<&V> {
        let index = self.next;
        let Some(call) = self.expected.get(index) else {
            return Err(self.diverge(format!("extra {op} call beyond the recorded stream")));
        };
        if get(call, "op").and_then(V::as_text) != Some(op) {
            return Err(self.diverge(format!("call {index} is not recorded as {op}")));
        }
        self.next += 1;
        Ok(&self.expected[index])
    }
    fn database(
        &mut self,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> coglet_engine::Result<u32> {
        let args: [u32; 4] = args
            .iter()
            .map(|v| u32::try_from(*v).map_err(|_| EngineError::InvalidPointer))
            .collect::<coglet_engine::Result<Vec<_>>>()?
            .try_into()
            .map_err(|_| EngineError::InvalidArgument)?;
        let [ptr, len, out_ptr, out_cap] = args;
        // Live refuses overlapping/out-of-range pointers with ABI invalid_argument
        // and does not journal a call. Consume a recorded slot only after that.
        if coglet_engine::disjoint_guest_ranges(ptr, len, out_ptr, out_cap, memory.memory_len())
            .is_err()
        {
            return Err(EngineError::InvalidPointer);
        }
        let call = self.recorded("db")?.clone();
        let sent = memory.read(ptr, len)?;
        let wanted = if let Some(argument) = get(&call, "argument") {
            coglet_types::encode(REQUEST_RULE, argument)
        } else {
            get(&call, "argument_hex")
                .and_then(V::as_text)
                .ok_or_else(|| "recorded database argument missing".to_owned())
                .and_then(|hex| hex::decode(hex).map_err(|e| e.to_string()))
        };
        if wanted.as_deref() != Ok(sent.as_slice()) {
            return Err(self.diverge("database request bytes differ from the recorded call"));
        }
        let result = get(&call, "result")
            .and_then(V::as_integer)
            .and_then(|i| u32::try_from(i).ok())
            .ok_or_else(|| self.diverge("recorded database result missing"))?;
        if result != 0 {
            self.poisoned = true;
            return Ok(result);
        }
        let response = get(&call, "response")
            .ok_or_else(|| self.diverge("recorded success carries no response"))?;
        let bytes = coglet_types::encode(REPLY_RULE, response)
            .map_err(|e| self.diverge(format!("recorded response is not encodable: {e}")))?;
        if get(response, "status").and_then(V::as_text) == Some("failed") {
            self.poisoned = true;
        }
        if out_cap < 65540 || bytes.len() > 65536 {
            return Err(self.diverge("recorded response does not fit the guest buffer"));
        }
        memory.write(out_ptr, &(bytes.len() as u32).to_be_bytes())?;
        memory.write(
            out_ptr.checked_add(4).ok_or(EngineError::InvalidPointer)?,
            &bytes,
        )?;
        budget.charge_host_bytes(bytes.len() as u64)?;
        Ok(0)
    }
}
impl Host for ReplayHost<'_> {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> coglet_engine::Result<u32> {
        budget.check_deadline()?;
        if self.diverged.is_some() {
            // Sticky: a guest that ignored the first error cannot continue.
            return Err(EngineError::Trap("replay divergence".into()));
        }
        if module == "wasi_snapshot_preview1" {
            return self.wasi.call(name, args, memory, budget);
        }
        if module != "coglet_v0" {
            return Err(EngineError::UnqualifiedModule);
        }
        if name == "db" {
            return self.database(args, memory, budget);
        }
        if !["emit", "timer", "llm", "log"].contains(&name) || args.len() != 2 {
            return Err(EngineError::UnqualifiedModule);
        }
        let ptr = u32::try_from(args[0]).map_err(|_| EngineError::InvalidPointer)?;
        let len = u32::try_from(args[1]).map_err(|_| EngineError::InvalidPointer)?;
        let raw = memory.read(ptr, len)?;
        // Live `import` may refuse without appending. Do not consume a recorded
        // call, and do not turn that refusal into sticky extra-call divergence.
        let result = match self.ordinary.import(name, &raw) {
            Ok(result) => result,
            Err(error) => return Err(EngineError::Trap(error)),
        };
        if name == "log" && result == 0 {
            self.wasi
                .sync_events_log(&self.ordinary.files)
                .map_err(EngineError::Trap)?;
        }
        let expected = self.recorded(name)?.clone();
        let actual = self
            .ordinary
            .calls
            .last()
            .ok_or(EngineError::InvalidArgument)
            .and_then(|call| profile_value(call, 0).map_err(EngineError::Trap))?;
        // Compare canonical profile encodings: map key order is not meaning.
        let canonical = |call: &V| coglet_types::encode_profile(call).map_err(EngineError::Trap);
        if canonical(&actual)? != canonical(&expected)? {
            return Err(self.diverge(format!("ordinary {name} call differs from the record")));
        }
        budget.check_deadline()?;
        Ok(result as u32)
    }
}

/// Engine adapter for `Runtime::step`: runs the pinned guest through the
/// replay host and reports the recorded observation. No database exists here.
struct ReplayAttempt<'a, E: SharedEngine> {
    engine: &'a E,
    code: &'a [u8],
    runtime_bytes: u32,
    stack_bytes: u32,
    calls: &'a [V],
    pins: Pins,
    recovery_abort: Option<Value>,
    prepared: Option<(E::Instance, StepInput, Instant, u32)>,
}
impl<E: SharedEngine> Engine for ReplayAttempt<'_, E> {
    fn metadata(&self) -> Option<Value> {
        let metadata = self.engine.metadata();
        Some(json!({"build":metadata.build,"policy":metadata.compute_mode}))
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> Result<()> {
        self.prepared = None;
        let started = Instant::now();
        if host.files.files().get("/code/bin/step.wasm") != Some(&hex::encode(self.code)) {
            return Err("replay engine code pin mismatch".into());
        }
        let budgets = &inputs["budgets"];
        validate("budgets", budgets)?;
        let limits = Limits {
            compute_mode: budgets["compute_mode"]
                .as_str()
                .ok_or("compute mode")?
                .into(),
            compute: budgets["compute"].as_u64().ok_or("compute")?,
            memory_pages: u32::try_from(budgets["memory_pages"].as_u64().ok_or("memory pages")?)
                .map_err(|_| "memory pages overflow")?,
            host_bytes: budgets["host_bytes"].as_u64().ok_or("host bytes")?,
            runtime_bytes: self.runtime_bytes,
            stack_bytes: self.stack_bytes,
            deadline: started
                .checked_add(Duration::from_micros(
                    budgets["wall_time_us"].as_u64().ok_or("wall time")?,
                ))
                .ok_or("deadline overflow")?,
        };
        let pages = limits.memory_pages;
        let module = self
            .engine
            .load(self.code, &limits)
            .map_err(|e| e.to_string())?;
        let instance = self
            .engine
            .instantiate(&module, limits)
            .map_err(|e| e.to_string())?;
        self.prepared = Some((
            instance,
            StepInput {
                events: cbor(&inputs["events"])?,
                step: inputs["step"].as_u64().ok_or("step")?,
                logical_time: inputs["logical_time"].as_u64().ok_or("logical time")?,
                seed: inputs["seed"].as_u64().ok_or("seed")?,
            },
            started,
            pages,
        ));
        Ok(())
    }
    fn execute(&mut self, _inputs: &Value, host: &mut StepHost) -> Result<EngineReport> {
        let (instance, input, started, pages) = self
            .prepared
            .take()
            .ok_or("replay execution requires successful admission")?;
        let wasi = crate::wasi::Wasi::new(&host.files)?;
        let mut replay = ReplayHost {
            wasi,
            ordinary: host,
            expected: self.calls,
            next: 0,
            poisoned: false,
            diverged: None,
        };
        let mut outcome = instance.handle(input, &mut replay);
        // Divergence outranks every guest outcome, including success.
        if let Some(why) = &replay.diverged {
            return Err(format!("replay divergence: {why}"));
        }
        if replay.next != self.calls.len() {
            return Err(format!(
                "replay divergence: {} recorded call(s) were never made",
                self.calls.len() - replay.next
            ));
        }
        if outcome.result.is_ok() && !replay.poisoned {
            replay.wasi.commit(&mut replay.ordinary.files)?;
        }
        let poisoned_return = outcome.result.is_ok() && replay.poisoned;
        if poisoned_return {
            outcome.result = Err(EngineError::Trap("database unit conflict".into()));
        }
        let failure = if let Some(abort) = &self.recovery_abort {
            // Host-decided abort overlay: only on a reproduced clean success.
            if outcome.result.is_err() || replay.poisoned {
                return Err(
                    "replay divergence: recovery abort recorded for a guest that did not \
                     return success with an unpoisoned unit"
                        .into(),
                );
            }
            Some(abort.clone())
        } else if poisoned_return {
            Some(json!({"kind":"guest_return","error":{"code":"conflict",
                "message":"database unit poisoned by a failed call","retryable":false}}))
        } else {
            outcome.result.err().map(native_engine::engine_failure)
        };
        Ok(EngineReport {
            usage: json!({"compute":outcome.compute_used,"host_bytes":outcome.host_bytes_used,
                "memory_pages":pages,"wall_time_us":u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)}),
            failure,
        })
    }
    fn database_observation(&mut self, committed: bool) -> Result<Option<DatabaseObservation>> {
        Ok(Some(DatabaseObservation {
            calls: self.calls.to_vec(),
            db_before: self.pins.db_before,
            db_after: if committed {
                self.pins.db_after
            } else {
                self.pins.db_before
            },
            schema_version: self.pins.schema_version,
            catalog_hash: self.pins.catalog_hash.clone(),
        }))
    }
}

impl Runtime {
    /// Re-execute exact ABI 0.2 `db-step-record` byte strings (the WAL's
    /// `database_record_bytes`, or `record_raw` hex-decoded) with a pinned
    /// engine. The guest sees only recorded results; there is no database,
    /// catalog, cursor, journal or dispatch path. Returns the state hashes.
    ///
    /// Nothing is adopted into `self` (like `replay`): failure of any record
    /// fails the whole replay. Wall duration is a host observation and is the
    /// only field not compared; every other field, including the byte-exact
    /// native calls and deterministic usage counters, must be reproduced.
    pub fn replay_database<E: SharedEngine>(
        &mut self,
        records: &[&[u8]],
        engine: &E,
        runtime_bytes: u32,
        stack_bytes: u32,
    ) -> Result<Vec<String>> {
        let mut candidate = self.clone();
        let mut hashes = Vec::new();
        for raw in records {
            // Checks the schema and the record hash over the exact bytes.
            let record = database_projection(raw)?;
            let native = coglet_types::decode(RECORD_RULE, raw)?;
            if coglet_types::encode(RECORD_RULE, &native)? != *raw {
                return Err("replay record bytes are not canonical".into());
            }
            let calls = get(&native, "calls")
                .and_then(V::as_array)
                .ok_or("recorded calls missing")?;
            // A database record exists only because the guest called `db`.
            if !calls
                .iter()
                .any(|call| get(call, "op").and_then(V::as_text) == Some("db"))
            {
                return Err("database record carries no database call".into());
            }
            let pins = Pins {
                db_before: uint(&native, "db_before")?,
                db_after: uint(&native, "db_after")?,
                schema_version: uint(&native, "db_schema_version")?,
                catalog_hash: get(&native, "db_catalog_hash")
                    .and_then(V::as_text)
                    .ok_or("recorded catalog hash missing")?
                    .to_owned(),
            };
            let aborted = record["status"] == "aborted";
            if aborted && pins.db_after != pins.db_before {
                return Err("aborted database record changes the revision".into());
            }
            if !aborted
                && pins.db_after != pins.db_before
                && Some(pins.db_after) != pins.db_before.checked_add(1)
            {
                return Err("committed database record skips a revision".into());
            }
            let recovery_abort = (record["failure"]["kind"] == "db_recovery_abort")
                .then(|| record["failure"].clone());
            if recovery_abort.is_some() && !aborted {
                return Err("recovery abort attached to a committed record".into());
            }
            let code = hex::decode(
                candidate
                    .files
                    .files()
                    .get("/code/bin/step.wasm")
                    .ok_or("missing step.wasm")?,
            )
            .map_err(|e| e.to_string())?;
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
            let mut attempt = ReplayAttempt {
                engine,
                code: &code,
                runtime_bytes,
                stack_bytes,
                calls,
                pins,
                recovery_abort,
                prepared: None,
            };
            let result = candidate.step(&Value::Object(inputs), &mut attempt)?;
            let (mut actual, mut expected) = (result["journal"].clone(), record.clone());
            for value in [&mut actual, &mut expected] {
                let object = value.as_object_mut().ok_or("record is not an object")?;
                object.remove("record_hash");
                object
                    .get_mut("usage")
                    .and_then(Value::as_object_mut)
                    .ok_or("usage missing")?
                    .remove("wall_time_us");
            }
            // Whole record, including failure/status/calls. A sticky extra call
            // or a dropped failure cannot hide behind a partial field list.
            if actual.get("failure") != expected.get("failure")
                || actual.get("status") != expected.get("status")
                || actual.get("calls") != expected.get("calls")
            {
                return Err(format!(
                    "replay mismatch at step {}: failure/status/calls",
                    record["step"]
                ));
            }
            if actual != expected {
                let differing: Vec<_> = actual
                    .as_object()
                    .into_iter()
                    .flatten()
                    .filter(|(key, value)| expected.get(key.as_str()) != Some(*value))
                    .chain(
                        expected
                            .as_object()
                            .into_iter()
                            .flatten()
                            .filter(|(key, _)| actual.get(key.as_str()).is_none()),
                    )
                    .map(|(key, _)| key.as_str())
                    .collect();
                return Err(format!(
                    "replay mismatch at step {}: differing {differing:?}",
                    record["step"]
                ));
            }
            candidate.previous = record["record_hash"].clone();
            hashes.push(
                result["state_hash"]
                    .as_str()
                    .ok_or("missing state hash")?
                    .to_owned(),
            );
        }
        Ok(hashes)
    }
}
