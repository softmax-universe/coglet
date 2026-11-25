//! Durable ABI 0.2 database path of `RuntimeService` (ADR-0013).
//!
//! One guest attempt flows: intent fence (WAL) -> guest through the shared
//! engine with native DB calls -> the COMPLETE next `ServiceState` (router
//! consumption, boundary, files, effects, dispatch order) is prepared as the
//! decision's `artifacts` -> `Unit::complete` (prepare, marker commit, journal)
//! -> the `WalJournal` collaborator atomically writes that exact state and the
//! exact record bytes into the WAL. Restart/uncertain-commit recovery calls the
//! SAME `WalJournal` through `Database::recover`; it never executes a guest.
//!
//! Lossless storage: the record is the deterministic CBOR byte string produced
//! once at step time. It is stored verbatim in the WAL (`commit_database`) and
//! exported verbatim. JSON appears only as a derived projection for chain
//! checks and is recomputed from the bytes on open. Artifacts are the
//! deterministic CBOR of the JSON `ServiceState`, the same value the WAL stores.
use super::*;
use crate::database::{DatabaseHost, Deployment, admit};
use crate::journal::database_projection;
use coglet_db::{Catalog, Database, Decision, Journal, Recovered, StepId, Unit};
use coglet_engine::{Engine as SharedEngine, Limits, StepInput};
use coglet_types::Value as V;
use std::time::{Duration, Instant};

/// Which journal publication a host-supplied availability gate is guarding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Publication {
    Committed,
    Aborted,
}

/// Optional host seam consulted before every journal publication (live and
/// recovery). Returning an error models an unavailable journal: the decision
/// stays held for recovery; nothing is aborted or lost.
pub type JournalGate<'a> = &'a mut dyn FnMut(Publication, &StepId) -> Result<()>;

/// Everything a database-bearing attempt needs beyond the shared context.
pub struct DatabaseStep<'a, 'g, E: SharedEngine> {
    pub engine: &'a E,
    pub code: &'a [u8],
    pub deployment: Deployment<'a>,
    pub db: &'a mut Database,
    pub catalog: &'a Catalog,
    pub runtime_bytes: u32,
    pub stack_bytes: u32,
    pub gate: Option<JournalGate<'g>>,
}

struct DbAttempt<'a, E: SharedEngine> {
    engine: &'a E,
    code: &'a [u8],
    runtime_bytes: u32,
    stack_bytes: u32,
    deployment: Option<Deployment<'a>>,
    db: Option<&'a mut Database>,
    catalog: &'a Catalog,
    schema_version: u64,
    catalog_hash: String,
    unit: Option<Unit<'a>>,
    prepared: Option<(E::Instance, StepInput, Instant, u32)>,
    db_before: u64,
    calls: Vec<V>,
}

impl<E: SharedEngine> Engine for DbAttempt<'_, E> {
    fn metadata(&self) -> Option<Value> {
        let metadata = self.engine.metadata();
        Some(json!({"build":metadata.build,"policy":metadata.compute_mode}))
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> Result<()> {
        self.unit = None;
        self.prepared = None;
        self.calls.clear();
        let started = Instant::now();
        if host.files.files().get("/code/bin/step.wasm") != Some(&hex::encode(self.code)) {
            return Err("database engine code pin mismatch".into());
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
        let db = self.db.take().ok_or("database already admitted")?;
        let deployment = self
            .deployment
            .take()
            .ok_or("deployment already admitted")?;
        // Admission failures happen before guest entry and before any intent
        // fence: the batch stays retryable.
        let (unit, instance) = admit(self.engine, self.code, deployment, db, self.catalog, limits)?;
        self.db_before = unit.revision();
        self.unit = Some(unit);
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
            .ok_or("database execution requires successful admission")?;
        let mut unit = self.unit.take().ok_or("missing database unit")?;
        let (outcome, calls, poisoned_return) = {
            let mut native = DatabaseHost::new(&mut unit, host);
            let outcome = native.execute(instance, input);
            (outcome, native.calls().to_vec(), native.poisoned_return())
        };
        self.unit = Some(unit);
        self.calls = calls;
        let failure = if poisoned_return {
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
        let used = self
            .calls
            .iter()
            .any(|call| matches!(call, V::Map(m) if m.iter().any(|(k, v)| matches!((k, v), (V::Text(k), V::Text(v)) if k == "op" && v == "db"))));
        if !used {
            // No database call: ordinary ABI 0.1 commit/abort. Dropping the unit
            // rolls back nothing; it never held a transaction.
            self.unit = None;
            return Ok(None);
        }
        let db_after = if committed {
            self.unit
                .as_ref()
                .ok_or("missing database unit")?
                .resulting_revision()?
        } else {
            // Report-level failure (budgets) after a clean guest return.
            self.unit.as_mut().ok_or("missing database unit")?.abort();
            self.db_before
        };
        Ok(Some(DatabaseObservation {
            calls: self.calls.clone(),
            db_before: self.db_before,
            db_after,
            schema_version: self.schema_version,
            catalog_hash: self.catalog_hash.clone(),
        }))
    }
}

/// The single journal collaborator: live `Unit::complete` and
/// `Database::recover` both publish through it, idempotently, into the WAL.
struct WalJournal<'s, 'g> {
    store: &'s mut DurableStore,
    gate: Option<JournalGate<'g>>,
}
impl WalJournal<'_, '_> {
    fn check(&mut self, kind: Publication, id: &StepId) -> Result<()> {
        match &mut self.gate {
            Some(gate) => historical_scope(false, || gate(kind, id)),
            None => Ok(()),
        }
    }
    /// Exact record AND artifact receipt must match. Legacy committed records
    /// without a receipt cannot establish identical artifacts; fail closed.
    fn already(&self, id: &StepId, record: &[u8], artifacts_hash: Option<&str>) -> Result<bool> {
        for (index, existing) in self.store.records().iter().enumerate().rev() {
            if existing.get("step") == Some(&json!(id.step))
                && existing["trace_id"] == id.trace_id
                && existing["coglet"] == id.coglet
            {
                return match self.store.database_record_bytes(index) {
                    Some(bytes)
                        if bytes == record
                            && self.store.database_artifacts_hash(index) == artifacts_hash =>
                    {
                        Ok(true)
                    }
                    _ => Err(format!(
                        "conflicting journal publication for step {}",
                        id.step
                    )),
                };
            }
        }
        Ok(false)
    }
    fn current(&self) -> Result<ServiceState> {
        serde_json::from_value(self.store.state().ok_or("missing WAL state")?.clone())
            .map_err(|e| e.to_string())
    }
}
impl Journal for WalJournal<'_, '_> {
    fn publish_committed(&mut self, id: &StepId, record: &[u8], artifacts: &[u8]) -> Result<()> {
        self.check(Publication::Committed, id)?;
        if self.already(id, record, Some(&digest(artifacts)))? {
            return Ok(());
        }
        let projection = database_projection(record)?;
        if projection["status"] != "committed" {
            return Err("committed publication carries a non-committed record".into());
        }
        let saved: Value = coglet_types::decode_profile(artifacts)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        // Snapshot's hash mode is deliberately not serialized. Rehydrate it
        // against the validated prepared record's ABI, not an ambient caller's
        // mode: old PREPARED artifacts carry CBOR snapshot hashes, never Git ids.
        let state: ServiceState = historical_scope(legacy_version(&projection["version"]), || {
            serde_json::from_value(saved).map_err(|e| e.to_string())
        })?;
        let runtime = &state.runtime;
        if legacy_version(&runtime.manifest["version"]) != legacy_version(&projection["version"])
            || runtime.unresolved_attempt.is_some()
            || runtime.previous != projection["record_hash"]
            || json!(runtime.step) != json!(projection["step"].as_u64().ok_or("step")? + 1)
            || json!(runtime.time) != projection["logical_time"]
            || json!(runtime.files.tree_hash()?) != projection["output_hash"]
        {
            return Err("prepared state is not the successor of the prepared record".into());
        }
        self.check_fence(&projection)?;
        // Host metadata (router, clock anchor, surface epochs, heard barriers,
        // dispatches, private events, staged boundary) may legitimately have
        // changed after the fence. Derive the successor from the CURRENT WAL
        // state exactly as ABI 0.1 recovery does, then require the prepared
        // runtime and dispatch order to agree with that derivation.
        let current = self.current()?;
        let mut derived =
            RuntimeService::reconcile_attempt(&current, self.store.records(), &projection)?;
        Self::retain_later_boundary(&current, &mut derived)?;
        let mut prepared = state.runtime.clone();
        prepared.next_source_seq = prepared
            .next_source_seq
            .max(current.runtime.next_source_seq);
        prepared.time = prepared.time.max(current.runtime.time);
        prepared.retained_blobs = current.runtime.retained_blobs.clone();
        let json_of = |v: &Runtime| serde_json::to_value(v).map_err(|e| e.to_string());
        if json_of(&prepared)? != json_of(&derived.runtime)?
            || state.dispatch_order != derived.dispatch_order
        {
            return Err("prepared state differs from state derived from the journal".into());
        }
        let saved = serde_json::to_value(&derived).map_err(|e| e.to_string())?;
        self.store
            .commit_database_artifacts(&saved, record, artifacts)
    }
    fn publish_abort(&mut self, id: &StepId, record: &[u8]) -> Result<()> {
        self.check(Publication::Aborted, id)?;
        if self.already(id, record, None)? {
            return Ok(());
        }
        let projection = database_projection(record)?;
        if projection["status"] != "aborted" || projection["failure"]["kind"] != "db_recovery_abort"
        {
            return Err("abort publication carries a non-recovery-abort record".into());
        }
        // The abort carries no artifacts. Its host state is derived exactly as
        // for any durable ABI abort: the fenced attempt input in the WAL plus
        // the recorded observation, with wakes consumed and boundary kept.
        let current = self.current()?;
        let mut next =
            RuntimeService::reconcile_attempt(&current, self.store.records(), &projection)?;
        Self::retain_later_boundary(&current, &mut next)?;
        self.store.commit_database(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            record,
        )
    }
}
impl WalJournal<'_, '_> {
    /// Preserve post-fence boundary appends for either authoritative decision.
    fn retain_later_boundary(current: &ServiceState, derived: &mut ServiceState) -> Result<()> {
        let consumed = current
            .runtime
            .unresolved_attempt
            .as_ref()
            .and_then(|a| a["boundary"].as_array())
            .cloned()
            .unwrap_or_default();
        let used = current
            .fenced_staged_prefix
            .ok_or("missing fenced staged prefix receipt")?;
        if let Some(staged) = &current.staged_boundary {
            if used > staged.changes.len()
                || used > consumed.len()
                || staged.changes[..used] != consumed[..used]
            {
                return Err("fenced staged prefix differs from prepared boundary".into());
            }
            let rest = staged.changes[used..].to_vec();
            if !rest.is_empty() {
                let mut runtime = derived.runtime.clone();
                runtime.apply_boundary_changes(&rest)?;
                runtime.next_source_seq = derived.runtime.next_source_seq;
                derived.staged_boundary = Some(StagedBoundary {
                    changes: rest,
                    runtime,
                });
            }
        } else if used != 0 {
            return Err("fenced staged prefix missing from current state".into());
        }
        Ok(())
    }
    /// The WAL must hold the entry fence of exactly this attempt.
    fn check_fence(&self, projection: &Value) -> Result<()> {
        let current = self.current()?;
        let inputs = current
            .runtime
            .unresolved_attempt
            .as_ref()
            .ok_or("no fenced attempt for this publication")?;
        for key in [
            "trace_id",
            "coglet",
            "step",
            "previous",
            "logical_time",
            "seed",
            "engine",
            "budgets",
            "grants",
            "events",
            "boundary",
        ] {
            if projection.get(key) != inputs.get(key) {
                return Err(format!("publication differs from fenced attempt: {key}"));
            }
        }
        Ok(())
    }
}

impl RuntimeService {
    /// Run the next batch through a real or mock shared engine with the ABI 0.2
    /// database import. Mirrors `run_next`: Ok(None) when nothing is queued. Any
    /// error after guest entry leaves the attempt fenced; call
    /// `recover_database` (never rerun the guest).
    pub fn run_next_database<E: SharedEngine>(
        &mut self,
        context: &Value,
        step: DatabaseStep<'_, '_, E>,
    ) -> Result<Option<Value>> {
        let DatabaseStep {
            engine,
            code,
            deployment,
            db,
            catalog,
            runtime_bytes,
            stack_bytes,
            gate,
        } = step;
        self.require_deliverable()?;
        // Previous decisions must be reconciled before a new step (held).
        db.reconciled()?;
        // Authenticated deployment pins must be exactly what the durable
        // runtime and this attempt's recorded grants are (before any fence).
        if deployment.manifest != &self.state.runtime.manifest {
            return Err("deployment manifest differs from the runtime manifest".into());
        }
        let mut granted: Vec<&str> = context["grants"]
            .as_array()
            .ok_or("missing context grants")?
            .iter()
            .map(|g| g.as_str().ok_or("invalid context grant"))
            .collect::<std::result::Result<_, _>>()?;
        let mut pinned: Vec<&str> = deployment.grants.iter().map(String::as_str).collect();
        granted.sort_unstable();
        pinned.sort_unstable();
        if granted != pinned {
            return Err("deployment grants differ from the context grants".into());
        }
        let Some((inputs, events, mut next)) = self.begin_attempt(context)? else {
            return Ok(None);
        };
        let mut adapter = DbAttempt {
            engine,
            code,
            runtime_bytes,
            stack_bytes,
            schema_version: deployment.pins.schema_version,
            catalog_hash: deployment.pins.catalog_hash.clone(),
            deployment: Some(deployment),
            db: Some(db),
            catalog,
            unit: None,
            prepared: None,
            db_before: 0,
            calls: Vec::new(),
        };
        let mut attempt = DurableAttempt {
            engine: &mut adapter,
            store: &mut self.store,
            before: &self.state,
            entered: None,
        };
        let result = next.runtime.step(&inputs, &mut attempt);
        let entered = attempt.entered;
        if let Some(inputs) = entered {
            self.state.runtime.unresolved_attempt = Some(inputs);
            self.state.fenced_staged_prefix = Some(
                self.state
                    .staged_boundary
                    .as_ref()
                    .map_or(0, |s| s.changes.len()),
            );
        }
        let result = result?;
        Self::consume_attempt(&mut next, &events, &result)?;
        let state = serde_json::to_value(&next).map_err(|e| e.to_string())?;
        let Some(raw) = result["record_raw"].as_str() else {
            // No database call happened: ordinary ABI commit of a 0.1 record.
            drop(adapter);
            self.store.commit(&state, Some(&result["journal"]))?;
            self.state = next;
            return Ok(Some(result));
        };
        let raw = hex::decode(raw).map_err(|e| e.to_string())?;
        if result["journal"]["status"] == "aborted" {
            // The unit was rolled back inside the adapter; an aborted attempt has
            // no marker and nothing to prepare. Persist it with the same exact
            // bytes through the WAL.
            drop(adapter);
            self.store.commit_database(&state, &raw)?;
            self.state = next;
            return Ok(Some(result));
        }
        let unit = adapter.unit.take().ok_or("missing database unit")?;
        let decision = Decision {
            record: raw,
            artifacts: cbor(&state)?,
        };
        let mut journal = WalJournal {
            store: &mut self.store,
            gate,
        };
        unit.complete(&decision, &mut journal)?;
        self.state = next;
        Ok(Some(result))
    }

    /// Authoritative recovery of an uncertain/crashed database decision, with
    /// NO guest execution: marker present rolls the exact prepared decision
    /// forward; marker absent publishes the host-decided `db_recovery_abort`;
    /// a DB or journal error is `held:` and keeps the preparation and the fence.
    /// The in-memory state is then reloaded from the WAL through the same full
    /// validation as `open`, whatever the outcome.
    pub fn recover_database(
        &mut self,
        db: &mut Database,
        gate: Option<JournalGate<'_>>,
    ) -> Result<Recovered> {
        let result = {
            let mut journal = WalJournal {
                store: &mut self.store,
                gate,
            };
            historical_scope(
                legacy_version(&self.state.runtime.manifest["version"]),
                || db.recover(&mut journal),
            )
        };
        if self.store.is_healthy() {
            let saved = self.store.state().ok_or("missing WAL state")?.clone();
            self.state = Self::restore(
                &self.store,
                &self.configured,
                &saved,
                &self.catalog,
                &self.state.supervisor_reads,
            )?;
        }
        if result.is_ok() {
            self.upgrade_hash_if_ready()?;
        }
        result
    }

    /// Whether an entered attempt is still fenced (needs `recover_database`).
    pub fn has_unresolved_attempt(&self) -> bool {
        self.state.runtime.unresolved_attempt.is_some()
    }

    /// Exact ABI journal bytes (u32be framing; ABI 0.2 records verbatim).
    pub fn export_journal(&self, output: impl std::io::Write) -> Result<()> {
        self.store.export_journal(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unit-level journal fixture, not an engine/DB integration claim.
    fn decision_with_staged(
        staged: bool,
    ) -> (tempfile::TempDir, DurableStore, StepId, Vec<u8>, Vec<u8>) {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
            .unwrap()["steps"][0]["send"]
            .clone();
        let dir = tempfile::tempdir().unwrap();
        let mut service = RuntimeService::open(&dir.path().join("journal.wal"), &setup, 4).unwrap();
        service
            .enqueue(setup["inputs"]["events"][0].clone())
            .unwrap();
        if staged {
            let change = json!({"actor":format!("hub:{}", "a".repeat(64)),"audit_id":"0".repeat(32),"ticket":null,"method":"debug.write","target":setup["manifest"]["id"],"path":"/data/repeated","before":null,"after":null});
            let mut runtime = service.state.runtime.clone();
            runtime
                .apply_boundary_changes(std::slice::from_ref(&change))
                .unwrap();
            setup["inputs"]["boundary"] = json!([change.clone()]);
            service.state.staged_boundary = Some(StagedBoundary {
                changes: vec![change],
                runtime,
            });
        }
        let (inputs, events, mut next) = service.begin_attempt(&setup["inputs"]).unwrap().unwrap();
        struct Observation;
        impl Engine for Observation {
            fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
                Ok(EngineReport {
                    usage: json!({"compute":0,"host_bytes":0,"memory_pages":0,"wall_time_us":0}),
                    failure: None,
                })
            }
            fn database_observation(&mut self, _: bool) -> Result<Option<DatabaseObservation>> {
                Ok(Some(DatabaseObservation {
                    calls: vec![],
                    db_before: 0,
                    db_after: 0,
                    schema_version: 1,
                    catalog_hash: "0".repeat(64),
                }))
            }
        }
        let mut engine = Observation;
        let mut attempt = DurableAttempt {
            engine: &mut engine,
            store: &mut service.store,
            before: &service.state,
            entered: None,
        };
        let result = next.runtime.step(&inputs, &mut attempt).unwrap();
        RuntimeService::consume_attempt(&mut next, &events, &result).unwrap();
        let raw = hex::decode(result["record_raw"].as_str().unwrap()).unwrap();
        let artifacts = cbor(&serde_json::to_value(next).unwrap()).unwrap();
        let id = StepId {
            trace_id: inputs["trace_id"].as_str().unwrap().into(),
            coglet: inputs["coglet"].as_str().unwrap().into(),
            step: inputs["step"].as_u64().unwrap(),
        };
        (dir, service.store, id, raw, artifacts)
    }

    #[test]
    fn committed_duplicate_requires_exact_artifacts_across_wal_reopen() {
        let (dir, mut store, id, record, artifacts) = decision_with_staged(false);
        {
            let mut journal = WalJournal {
                store: &mut store,
                gate: None,
            };
            journal.publish_committed(&id, &record, &artifacts).unwrap();
            journal.publish_committed(&id, &record, &artifacts).unwrap();
            let mut changed = artifacts.clone();
            changed.push(0);
            assert!(
                journal
                    .publish_committed(&id, &record, &changed)
                    .unwrap_err()
                    .contains("conflicting")
            );
        }
        assert_eq!(store.records().len(), 1);
        assert_eq!(
            store.database_artifacts_hash(0),
            Some(digest(&artifacts).as_str())
        );
        drop(store);
        let mut store = DurableStore::open(&dir.path().join("journal.wal")).unwrap();
        let before = store.state().cloned();
        let mut journal = WalJournal {
            store: &mut store,
            gate: None,
        };
        journal.publish_committed(&id, &record, &artifacts).unwrap();
        let mut changed: Value = coglet_types::decode_profile(&artifacts)
            .unwrap()
            .deserialized()
            .unwrap();
        changed["surface_epochs"] = json!({"different":99});
        assert!(
            journal
                .publish_committed(&id, &record, &cbor(&changed).unwrap())
                .unwrap_err()
                .contains("conflicting")
        );
        assert_eq!(store.records().len(), 1);
        assert_eq!(store.state().cloned(), before);
    }

    #[test]
    fn exact_fenced_staged_count_preserves_identical_post_fence_append() {
        let (dir, mut store, id, record, artifacts) = decision_with_staged(true);
        let mut current = store.state().unwrap().clone();
        assert_eq!(current["fenced_staged_prefix"], 1);
        let duplicate = current["staged_boundary"]["changes"][0].clone();
        current["staged_boundary"]["changes"]
            .as_array_mut()
            .unwrap()
            .push(duplicate.clone());
        store.commit(&current, None).unwrap();
        drop(store);
        let mut store = DurableStore::open(&dir.path().join("journal.wal")).unwrap();
        WalJournal {
            store: &mut store,
            gate: None,
        }
        .publish_committed(&id, &record, &artifacts)
        .unwrap();
        assert_eq!(
            store.state().unwrap()["staged_boundary"]["changes"],
            json!([duplicate])
        );
        assert!(store.state().unwrap().get("fenced_staged_prefix").is_none());
    }

    #[test]
    fn recovery_abort_preserves_identical_post_fence_boundary() {
        let (dir, mut store, id, record, _) = decision_with_staged(true);
        let mut current = store.state().unwrap().clone();
        let duplicate = current["staged_boundary"]["changes"][0].clone();
        current["staged_boundary"]["changes"]
            .as_array_mut()
            .unwrap()
            .push(duplicate.clone());
        store.commit(&current, None).unwrap();
        drop(store);
        let mut store = DurableStore::open(&dir.path().join("journal.wal")).unwrap();
        let abort = coglet_db::recovery_abort_record(&record).unwrap();
        WalJournal {
            store: &mut store,
            gate: None,
        }
        .publish_abort(&id, &abort)
        .unwrap();
        assert_eq!(
            store.state().unwrap()["staged_boundary"]["changes"],
            json!([duplicate])
        );
        assert_eq!(store.records()[0]["failure"]["kind"], "db_recovery_abort");
    }

    #[test]
    fn legacy_committed_frame_without_artifact_receipt_refuses_duplicate() {
        let (dir, mut store, id, record, artifacts) = decision_with_staged(false);
        store.commit_database(&json!({}), &record).unwrap();
        drop(store);
        let mut store = DurableStore::open(&dir.path().join("journal.wal")).unwrap();
        assert_eq!(store.database_artifacts_hash(0), None);
        assert!(
            WalJournal {
                store: &mut store,
                gate: None
            }
            .publish_committed(&id, &record, &artifacts)
            .unwrap_err()
            .contains("conflicting")
        );
        assert_eq!(store.records().len(), 1);
    }

    #[test]
    fn missing_or_invalid_fenced_staged_receipt_refuses_publication() {
        for receipt in [None, Some(json!(2))] {
            let (_dir, mut store, id, record, artifacts) = decision_with_staged(true);
            let mut current = store.state().unwrap().clone();
            match receipt {
                Some(value) => current["fenced_staged_prefix"] = value,
                None => {
                    current
                        .as_object_mut()
                        .unwrap()
                        .remove("fenced_staged_prefix");
                }
            }
            store.commit(&current, None).unwrap();
            assert!(
                WalJournal {
                    store: &mut store,
                    gate: None
                }
                .publish_committed(&id, &record, &artifacts)
                .unwrap_err()
                .contains("staged prefix")
            );
            assert!(store.records().is_empty());
            assert_eq!(store.state(), Some(&current));
        }
    }

    #[test]
    fn committed_publication_preserves_surface_epochs_and_heard_pending() {
        let (_dir, mut store, id, record, artifacts) = decision_with_staged(false);
        let mut current = store.state().unwrap().clone();
        current["surface_epochs"] = json!({"surface:test":42});
        current["heard_pending"] = json!({"pending":{"surface":"surface:test","epoch":42,"item":"item","played":8,"stopped":false,"expires":1000}});
        store.commit(&current, None).unwrap();
        WalJournal {
            store: &mut store,
            gate: None,
        }
        .publish_committed(&id, &record, &artifacts)
        .unwrap();
        for field in ["surface_epochs", "heard_pending"] {
            assert_eq!(store.state().unwrap()[field], current[field]);
        }
    }
}
