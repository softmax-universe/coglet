//! Offline archive validation is NOT guest re-execution or archive authentication.
//! It calculates host calls/ledgers and checks attributed guest filesystem deltas.
//! Use Runtime::replay with an independently pinned engine/program for replay.
use super::*;

/// An observed transition checker, never a Wasm engine. Only this private path
/// accepts recorded deltas; it cannot be selected by a live runtime service.
struct ObservedTransition<'a>(&'a Value);
impl Engine for ObservedTransition<'_> {
    fn execute(&mut self, _: &Value, host: &mut StepHost) -> Result<EngineReport> {
        let record = self.0;
        for call in record["calls"].as_array().ok_or("missing calls")? {
            let raw = if let Some(raw) = call["argument_hex"].as_str() {
                hex::decode(raw).map_err(|e| e.to_string())?
            } else {
                cbor(&call["argument"])?
            };
            let result = host.import(call["op"].as_str().ok_or("missing op")?, &raw)?;
            if call["result"] != result || host.calls.last() != Some(call) {
                return Err("archive host call result mismatch".into());
            }
        }
        if record["status"] == "committed" {
            let output = Snapshot::new(
                serde_json::from_value(record["output_files"].clone())
                    .map_err(|e| e.to_string())?,
            )?;
            // Host-owned bytes must come only from the calculated imports, not
            // from arbitrary output injection. Code is pinned for the segment.
            for (path, bytes) in output.files() {
                if host.files.files().get(path) != Some(bytes) {
                    host.guest_write(path, &hex::decode(bytes).map_err(|e| e.to_string())?)?;
                }
            }
            let removed: Vec<_> = host
                .files
                .files()
                .keys()
                .filter(|path| !output.files().contains_key(*path))
                .cloned()
                .collect();
            let mut files = host.files.files().clone();
            for path in removed {
                if !path.starts_with("/data/") || crate::filesystem::reserved_collision(&path) {
                    return Err("archive guest deletion violates mount permissions".into());
                }
                files.remove(&path);
            }
            host.files = Snapshot::new(files)?;
            if host.files != output {
                return Err("archive output mismatch".into());
            }
        }
        Ok(EngineReport {
            usage: record["usage"].clone(),
            failure: record.get("failure").cloned(),
        })
    }
}

/// Internal segment cutoffs refer to the same exclusively owned WAL. Old
/// records remain immutable; one state transaction promotes all coordinator
/// metadata without opening a second writable owner or dropping receipts.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompletedSegment {
    initial_setup: Value,
    trace_id: Value,
    end: usize,
}

impl RuntimeService {
    pub(super) fn segment_offset(&self) -> usize {
        self.state.completed_segments.last().map_or(0, |s| s.end)
    }

    pub(super) fn validate_segments(
        state: &ServiceState,
        root: &Value,
        records: &[Value],
    ) -> Result<usize> {
        let mut expected = root.clone();
        let mut offset = 0;
        let mut time_floor = 0;
        let mut source_floor = 0;
        let mut seen = std::collections::BTreeSet::new();
        for segment in &state.completed_segments {
            validate("correlation-id", &segment.trace_id)?;
            if !seen.insert(segment.trace_id.as_str().unwrap())
                || segment.initial_setup != expected
                || segment.end <= offset
                || segment.end > records.len()
            {
                return Err("invalid recovered segment header/cutoff/identity".into());
            }
            let old = Runtime::reconstruct(&segment.initial_setup, &records[offset..segment.end])?;
            if records[offset]["logical_time"]
                .as_u64()
                .ok_or("missing segment clock")?
                < time_floor
            {
                return Err("segment journal clock precedes predecessor".into());
            }
            time_floor = time_floor.max(old.time);
            source_floor = source_floor.max(old.next_source_seq);
            if !old.sealed || old.trace_id.as_ref() != Some(&segment.trace_id) {
                return Err("completed segment lacks matching durable seal".into());
            }
            let next_id = state
                .completed_segments
                .iter()
                .find(|s| s.end > segment.end)
                .map(|s| &s.trace_id)
                .or(state.runtime.trace_id.as_ref())
                .ok_or("missing successor trace identity")?;
            expected = old.start_segment(next_id)?.initial_setup;
            offset = segment.end;
        }
        if state.runtime.initial_setup != expected
            || state.runtime.time < time_floor
            || state.runtime.next_source_seq < source_floor
            || records
                .get(offset)
                .is_some_and(|r| r["logical_time"].as_u64().is_none_or(|t| t < time_floor))
            || state
                .runtime
                .trace_id
                .as_ref()
                .is_some_and(|id| seen.contains(id.as_str().unwrap_or("")))
        {
            return Err(
                "recovered initial snapshot/header differs from configuration/segment chain".into(),
            );
        }
        Ok(offset)
    }

    /// Atomically start a fresh trace-local chain after a durable seal. Keeps
    /// router ordinals, pending wakes, dispatch receipts, leases, lifetime IDs,
    /// source reservations and epoch clock in the same exclusively locked WAL.
    pub fn start_segment(&mut self, trace_id: &Value) -> Result<()> {
        if !self.store.is_healthy() || self.state.staged_boundary.is_some() {
            return Err("segment transfer requires healthy WAL and no staged boundary".into());
        }
        if self
            .state
            .completed_segments
            .iter()
            .any(|s| &s.trace_id == trace_id)
        {
            return Err("segment trace identity already used".into());
        }
        let mut next = self.state.clone();
        let runtime = next.runtime.start_segment(trace_id)?;
        next.completed_segments.push(CompletedSegment {
            initial_setup: next.runtime.initial_setup.clone(),
            trace_id: next
                .runtime
                .trace_id
                .clone()
                .ok_or("missing predecessor trace ID")?,
            end: self.store.records().len(),
        });
        next.runtime = runtime;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        #[cfg(feature = "native-engine")]
        {
            self.native = None;
        }
        Ok(())
    }

    /// Export a retained sealed predecessor. Caller separately authorizes
    /// private-data release and authenticates the cutoff and complete header.
    pub fn export_segment(&self, trace_id: &Value) -> Result<Value> {
        if !self.store.is_healthy() {
            return Err("WAL requires recovery before segment export".into());
        }
        let mut offset = 0;
        for segment in &self.state.completed_segments {
            if &segment.trace_id == trace_id {
                let mut runtime = Runtime::reconstruct(
                    &segment.initial_setup,
                    &self.store.records()[offset..segment.end],
                )?;
                runtime.retained_blobs = self.state.runtime.retained_blobs.clone();
                return runtime.export_trace(trace_id);
            }
            offset = segment.end;
        }
        Err("unknown completed segment".into())
    }

    /// Export all durable state through this serialized cutoff. Caller must
    /// authorize private-data release and authenticate cutoff/completeness. Any
    /// staged applied boundary is first sealed in the WAL, without a guest wake.
    /// A resulting sealed trace cannot resume until explicitly segmented.
    pub fn export_trace(&mut self, trace_id: &Value, logical_time: u64) -> Result<Value> {
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("unresolved attempt prevents complete export".into());
        }
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery before export".into());
        }
        validate("correlation-id", trace_id)?;
        if logical_time < self.state.runtime.time {
            return Err("export cutoff clock rollback".into());
        }
        if self.state.staged_boundary.is_some() {
            self.seal_boundary(trace_id, logical_time, &[])?;
        }
        self.state.runtime.export_trace(trace_id)
    }
}

impl Runtime {
    pub(super) fn reconstruct(initial: &Value, records: &[Value]) -> Result<Self> {
        let mut runtime = Self::from_fixture(initial)?;
        for record in records {
            if record.get("step").is_some() {
                let actual = runtime.step(record, &mut ObservedTransition(record))?;
                if actual["journal"] != *record {
                    return Err("recovered step mismatch".into());
                }
            } else {
                let actual = runtime.seal_boundary(
                    &record["trace_id"],
                    record["logical_time"].as_u64().ok_or("boundary time")?,
                    record["boundary"].as_array().ok_or("boundary changes")?,
                )?;
                if actual != *record {
                    return Err("recovered boundary mismatch".into());
                }
            }
        }
        Ok(runtime)
    }

    /// Validate all required header fields/blobs, replay host transitions and
    /// reconstruct pending work. No guest execution, dispatch or live lookup.
    /// Caller separately verifies authorization, admission signatures and cutoff.
    pub fn import_trace(trace: &Value) -> Result<Self> {
        validate("trace", trace)?;
        let blobs = trace["blobs"].as_object().ok_or("missing blobs")?;
        for (hash, bytes) in blobs {
            let raw =
                hex::decode(bytes.as_str().ok_or("invalid blob")?).map_err(|e| e.to_string())?;
            if hash != &digest(&raw) {
                return Err("trace blob digest mismatch".into());
            }
        }
        let manifest_raw = hex::decode(
            blobs
                .get(trace["manifest_hash"].as_str().ok_or("manifest pin")?)
                .and_then(Value::as_str)
                .ok_or("missing manifest blob")?,
        )
        .map_err(|e| e.to_string())?;
        let manifest: Value = coglet_types::decode("manifest", &manifest_raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        if manifest["id"] != trace["coglet"] {
            return Err("trace manifest identity mismatch".into());
        }
        let mut setup = json!({"manifest":manifest});
        for key in [
            "initial_files",
            "initial_effect_ids",
            "initial_actions",
            "initial_inbox_wakes",
            "initial_timers",
            "initial_llm_requests",
        ] {
            setup[key] = trace
                .get(key)
                .ok_or_else(|| format!("missing trace {key}"))?
                .clone();
        }
        let mut runtime = Self::from_fixture(&setup)?;
        let code = runtime
            .files
            .files()
            .get("/code/bin/step.wasm")
            .ok_or("missing trace code")?;
        if trace["code_hash"] != digest(&hex::decode(code).map_err(|e| e.to_string())?)
            || blobs.get(trace["code_hash"].as_str().ok_or("code pin")?) != Some(&json!(code))
            || trace["initial_hash"] != runtime.files.tree_hash()?
        {
            return Err("trace initial snapshot/pins mismatch".into());
        }
        runtime.retained_blobs = blobs.clone();
        runtime.trace_id = Some(trace["trace_id"].clone());
        for record in trace["records"].as_array().ok_or("missing records")? {
            // step() independently computes host returns, ledgers, boundary
            // mutations and hashes. Observed guest deltas are mount-validated;
            // they are NOT evidence an independently pinned guest produced them.
            let actual = runtime.step(record, &mut ObservedTransition(record))?;
            if actual["journal"] != *record {
                return Err("archive step mismatch".into());
            }
        }
        if !trace["trailing_boundary"].is_null() {
            let record = &trace["trailing_boundary"];
            let actual = runtime.seal_boundary(
                &trace["trace_id"],
                record["logical_time"].as_u64().ok_or("boundary time")?,
                record["boundary"].as_array().ok_or("boundary changes")?,
            )?;
            if actual != *record {
                return Err("archive trailing boundary mismatch".into());
            }
        }
        if trace["final_hash"] != runtime.files.tree_hash()? {
            return Err("trace final hash mismatch".into());
        }
        Ok(runtime)
    }

    /// Export the complete atomic header and exact journal bytes' JSON view.
    /// The caller must authorize release of private data and bind completeness.
    pub fn export_trace(&self, trace_id: &Value) -> Result<Value> {
        if self.unresolved_attempt.is_some() {
            return Err("unresolved attempt prevents complete export".into());
        }
        validate("correlation-id", trace_id)?;
        if self.trace_id.as_ref().is_some_and(|id| id != trace_id) {
            return Err("trace identity changed".into());
        }
        let initial = Self::from_fixture(&self.initial_setup)?;
        let code = &initial.files.files()["/code/bin/step.wasm"];
        let code_hash = digest(&hex::decode(code).map_err(|e| e.to_string())?);
        let manifest = cbor(&initial.manifest)?;
        let manifest_hash = digest(&manifest);
        let mut blobs = self.retained_blobs.clone();
        blobs.insert(code_hash.clone(), json!(code));
        blobs.insert(manifest_hash.clone(), json!(hex::encode(manifest)));
        let records: Vec<_> = self
            .records
            .iter()
            .filter(|r| r.get("step").is_some())
            .cloned()
            .collect();
        let trailing = self
            .records
            .last()
            .filter(|r| r.get("step").is_none())
            .cloned();
        let mut trace = json!({"version":"0.1","trace_id":trace_id,"coglet":self.manifest["id"],
            "initial_hash":initial.files.tree_hash()?,"code_hash":code_hash,"manifest_hash":manifest_hash,
            "final_hash":self.files.tree_hash()?,"records":records,"trailing_boundary":trailing,"blobs":blobs});
        for key in [
            "initial_files",
            "initial_effect_ids",
            "initial_actions",
            "initial_inbox_wakes",
            "initial_timers",
            "initial_llm_requests",
        ] {
            trace[key] = self.initial_setup[key].clone();
        }
        validate("trace", &trace)?;
        Ok(trace)
    }

    /// Fork a new trace-local segment from a sealed state, carrying lifetime
    /// effects, pending work, source sequence and runtime clock unchanged. This
    /// is an in-memory candidate: a durable coordinator must atomically persist
    /// it with its router/dispatch metadata before admitting or exporting work.
    pub fn start_segment(&self, trace_id: &Value) -> Result<Self> {
        if self.unresolved_attempt.is_some() {
            return Err("unresolved attempt prevents segmentation".into());
        }
        validate("correlation-id", trace_id)?;
        if !self.sealed || self.trace_id.as_ref() == Some(trace_id) {
            return Err("new segment requires sealed predecessor and fresh trace identity".into());
        }
        let setup = json!({"manifest":self.manifest,"initial_files":self.files.files(),
            "initial_effect_ids":self.ledger,"initial_actions":self.pending_actions.values().collect::<Vec<_>>(),
            "initial_inbox_wakes":self.inbox_wakes,"initial_timers":self.pending_timers.values().collect::<Vec<_>>(),
            "initial_llm_requests":self.pending_llm.values().collect::<Vec<_>>()});
        let mut next = Self::from_fixture(&setup)?;
        next.trace_id = Some(trace_id.clone());
        next.retained_blobs = self.retained_blobs.clone();
        next.next_source_seq = self.next_source_seq;
        next.time = self.time;
        Ok(next)
    }

    pub fn archive_state(&self) -> Result<Value> {
        Ok(
            json!({"final_hash":self.files.tree_hash()?,"pending_actions":self.pending_actions.keys().collect::<Vec<_>>(),
            "pending_inbox_wakes":self.inbox_wakes,"pending_timers":self.pending_timers.keys().collect::<Vec<_>>(),
            "pending_llm_requests":self.pending_llm.keys().collect::<Vec<_>>() }),
        )
    }
}
