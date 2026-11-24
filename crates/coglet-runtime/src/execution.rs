use crate::{DurableStore, Router, TrustedExtensionCatalog};
#[path = "supervisor_reads.rs"]
mod supervisor_reads;
use crate::{EffectLedger, Result, Snapshot, StepHost, cbor, digest, profile_value, validate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, VecDeque};
use std::path::Path;

#[path = "clock.rs"]
mod clock;
#[cfg(feature = "native-engine")]
#[path = "database_replay.rs"]
mod database_replay;
#[cfg(feature = "native-engine")]
#[path = "database_service.rs"]
pub mod database_service;
#[path = "dispatch.rs"]
mod dispatch;
#[cfg(test)]
#[path = "heard_deadline_tests.rs"]
mod heard_deadline_tests;
#[path = "live_heard.rs"]
mod live_heard;
#[path = "managed.rs"]
mod managed;
#[cfg(feature = "native-engine")]
#[path = "native_engine.rs"]
pub mod native_engine;
#[path = "playback.rs"]
mod playback;
#[path = "recovery.rs"]
mod recovery;
#[path = "speech_temporal.rs"]
mod speech_temporal;
#[path = "surface_events.rs"]
mod surface_events;
#[path = "trace.rs"]
mod trace;

/// Engine implementations must start a fresh instance for every attempt. This
/// fixture seam also adapts the shared Engine API through opt-in NativeEngine.
pub trait Engine {
    fn metadata(&self) -> Option<Value> {
        None
    }
    /// Refuse an input before guest entry. Must not execute guest code or
    /// mutate host state. Returning Err here leaves the batch safely retryable.
    fn admit(&mut self, _inputs: &Value, _host: &StepHost) -> Result<()> {
        Ok(())
    }
    /// Once entered, Err means unknown outcome/usage, NOT admission refusal.
    /// Known guest failures must return an observed EngineReport instead.
    fn execute(&mut self, inputs: &Value, host: &mut StepHost) -> Result<EngineReport>;
    /// ABI 0.2 hook, called once after `execute`, before the record is built.
    /// `committed` says whether the step will be a committed decision; an
    /// adapter that owns a database unit must abort it when false. Return
    /// `None` when no database call was made: the ordinary 0.1 record applies.
    fn database_observation(&mut self, _committed: bool) -> Result<Option<DatabaseObservation>> {
        Ok(None)
    }
}

/// Native observations that turn an ordinary attempt into an ABI 0.2
/// `db-step-record`. `calls` is the invocation-ordered native call stream
/// (ordinary calls and `db-host-call`s) and is never converted through JSON.
pub struct DatabaseObservation {
    pub calls: Vec<coglet_types::Value>,
    pub db_before: u64,
    pub db_after: u64,
    pub schema_version: u64,
    pub catalog_hash: String,
}

/// Persist entry intent before invoking an engine. A crash or unreportable
/// engine error retains this fence; only the complete atomic attempt commit
/// clears it. No automatic replay or invented zero-usage failure is safe.
struct DurableAttempt<'a, E> {
    engine: &'a mut E,
    store: &'a mut DurableStore,
    before: &'a ServiceState,
    entered: Option<Value>,
}
impl<E: Engine> Engine for DurableAttempt<'_, E> {
    fn metadata(&self) -> Option<Value> {
        self.engine.metadata()
    }
    fn admit(&mut self, inputs: &Value, host: &StepHost) -> Result<()> {
        self.engine.admit(inputs, host)
    }
    fn execute(&mut self, inputs: &Value, host: &mut StepHost) -> Result<EngineReport> {
        let mut intent = self.before.clone();
        intent.runtime.unresolved_attempt = Some(inputs.clone());
        intent.fenced_staged_prefix = Some(
            intent
                .staged_boundary
                .as_ref()
                .map_or(0, |s| s.changes.len()),
        );
        self.store.commit(
            &serde_json::to_value(intent).map_err(|e| e.to_string())?,
            None,
        )?;
        self.entered = Some(inputs.clone());
        self.engine.execute(inputs, host)
    }
    fn database_observation(&mut self, committed: bool) -> Result<Option<DatabaseObservation>> {
        self.engine.database_observation(committed)
    }
}

pub struct EngineReport {
    pub usage: Value,
    pub failure: Option<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
struct ServiceState {
    #[serde(default)]
    managed: Option<managed::ManagedState>,
    #[serde(default)]
    catalog_pin: Option<String>,
    #[serde(default)]
    supervisor_reads: BTreeMap<String, supervisor_reads::SupervisorReadRecord>,
    runtime: Runtime,
    router: Router,
    #[serde(default)]
    completed_segments: Vec<trace::CompletedSegment>,
    #[serde(default)]
    clock_anchor: Option<clock::ClockAnchor>,
    #[serde(default)]
    dispatch_order: Vec<String>,
    #[serde(default)]
    dispatches: BTreeMap<String, dispatch::Dispatch>,
    #[serde(default)]
    surface_epochs: BTreeMap<String, u64>,
    #[serde(default)]
    staged_boundary: Option<StagedBoundary>,
    /// Exact staged prefix included in the fenced input, not a value-based guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fenced_staged_prefix: Option<usize>,
    #[serde(default)]
    surface_sequences: BTreeMap<String, (u64, u64)>,
    #[serde(default)]
    private_events: BTreeMap<u64, surface_events::EventLease>,
    #[serde(default)]
    speech_temporal: speech_temporal::SpeechTemporal,
    #[serde(default)]
    heard_pending: BTreeMap<String, live_heard::HeardObservation>,
}

#[derive(Clone, Serialize, Deserialize)]
struct StagedBoundary {
    changes: Vec<Value>,
    runtime: Runtime,
}

/// Serial durable coordinator. Admission and consumption commit host metadata
/// together with files/journal; dispatchers may observe only committed state.
pub struct RuntimeService {
    #[cfg(test)]
    test_clock_sample: Option<clock::TestClockSample>,
    pub(crate) catalog: std::sync::Arc<crate::TrustedExtensionCatalog>,
    state: ServiceState,
    store: DurableStore,
    #[cfg(feature = "native-engine")]
    configured: Runtime,
    #[cfg(feature = "native-engine")]
    native: Option<(native_engine::NativeSchedule, native_engine::NativeEngine)>,
}
impl RuntimeService {
    /// Validate a saved WAL state against the configured pins and the exact
    /// journal. Shared by `open` and by database recovery reload, so a state
    /// published by a collaborator is checked as strictly as a restart.
    fn restore(
        store: &DurableStore,
        configured: &Runtime,
        saved: &Value,
        catalog: &TrustedExtensionCatalog,
        configured_reads: &BTreeMap<String, supervisor_reads::SupervisorReadRecord>,
    ) -> Result<ServiceState> {
        let mut state: ServiceState =
            serde_json::from_value(saved.clone()).map_err(|e| e.to_string())?;
        if state.catalog_pin.as_deref() != catalog.pin() {
            return Err("recovery extension catalog pin differs from configuration".into());
        }
        if state.supervisor_reads.len() != configured_reads.len()
            || state.supervisor_reads.iter().any(|(id, r)| {
                configured_reads
                    .get(id)
                    .is_none_or(|c| c.request != r.request)
            })
        {
            return Err("recovery supervisor read plan differs from configuration".into());
        }
        state.router.validate_recovered()?;
        let offset = Self::validate_segments(&state, &configured.initial_setup, store.records())?;
        state.runtime.records = store.records()[offset..].to_vec();
        let code_files = |runtime: &Runtime| {
            runtime
                .files
                .files()
                .iter()
                .filter(|(path, _)| path.starts_with("/code/"))
                .map(|(p, b)| (p.clone(), b.clone()))
                .collect::<BTreeMap<_, _>>()
        };
        if state.runtime.manifest != configured.manifest
            || code_files(&state.runtime) != code_files(configured)
        {
            return Err("recovery code/manifest pins differ from configuration".into());
        }
        state
            .runtime
            .validate_recovered(&store.records()[offset..])?;
        Self::validate_dispatches(&state, catalog)?;
        state
            .speech_temporal
            .validate(&state.surface_epochs, &state.runtime.manifest["id"])?;
        if let Some(staged) = &state.staged_boundary {
            let mut derived = state.runtime.clone();
            derived.apply_boundary_changes(&staged.changes)?;
            // Router sequences may already have been reserved after apply.
            derived.next_source_seq = staged.runtime.next_source_seq;
            // Attempt intent belongs to the coordinator, not the applied
            // boundary snapshot. It cannot change any boundary bytes.
            derived.unresolved_attempt = staged.runtime.unresolved_attempt.clone();
            if serde_json::to_value(&derived).map_err(|e| e.to_string())?
                != serde_json::to_value(&staged.runtime).map_err(|e| e.to_string())?
            {
                return Err("recovered staged boundary state mismatch".into());
            }
        }
        Self::validate_source_reservations(&state, configured, store.records())?;
        Self::validate_queued_wakes(&state)?;
        Self::validate_private_events(&state)?;
        Self::attempted_ordinals(&state)?;
        Ok(state)
    }

    /// ADR-0014 trusted in-process harness only. Recovery requires explicit
    /// fixture authority again; no daemon, C ABI or artifact enables it.
    pub fn open_echo_fixture(path: &Path, setup: &Value, queue_capacity: usize) -> Result<Self> {
        let catalog = crate::TrustedExtensionCatalog::echo_fixture();
        Self::open_with_catalog(path, setup, queue_capacity, catalog)
    }

    pub fn open(path: &Path, setup: &Value, queue_capacity: usize) -> Result<Self> {
        Self::open_with_catalog(
            path,
            setup,
            queue_capacity,
            crate::TrustedExtensionCatalog::default(),
        )
    }
    pub fn open_with_catalog(
        path: &Path,
        setup: &Value,
        queue_capacity: usize,
        catalog: crate::TrustedExtensionCatalog,
    ) -> Result<Self> {
        Self::open_configured(path, setup, queue_capacity, catalog, None, &|| Ok(0))
    }

    /// Construct and pin the stopped identity in the very first state frame.
    /// Failure/expiry before that frame leaves only an empty WAL, never an
    /// unmanaged service whose code could be rebound to a later install.
    #[cfg(feature = "device")]
    pub(crate) fn open_for_install(
        path: &Path,
        setup: &Value,
        queue_capacity: usize,
        identity: &Value,
        operation_id: &str,
        gate: &dyn Fn() -> Result<u64>,
    ) -> Result<Self> {
        Self::open_configured(
            path,
            setup,
            queue_capacity,
            crate::TrustedExtensionCatalog::default(),
            Some((identity, operation_id)),
            gate,
        )
    }

    fn open_configured(
        path: &Path,
        setup: &Value,
        queue_capacity: usize,
        catalog: crate::TrustedExtensionCatalog,
        install: Option<(&Value, &str)>,
        gate: &dyn Fn() -> Result<u64>,
    ) -> Result<Self> {
        let catalog = std::sync::Arc::new(catalog);
        let mut store = DurableStore::open(path)?;
        let fresh = store.state().is_none();
        let configured = Runtime::from_fixture(setup)?;
        let configured_reads = Self::configured_reads(setup, &catalog)?;
        let mut state: ServiceState = if let Some(saved) = store.state() {
            let saved_state: ServiceState =
                serde_json::from_value(saved.clone()).map_err(|e| e.to_string())?;
            if let Some(managed) = &saved_state.managed {
                managed.validate()?;
                if install.is_some_and(|(identity, _)| managed.identity() != identity) {
                    return Err("conflict: managed install identity differs".into());
                }
            } else if install.is_some() {
                return Err("permission_denied: existing unmanaged WAL cannot be rebound".into());
            }
            Self::restore(&store, &configured, saved, &catalog, &configured_reads)?
        } else {
            let mut state = ServiceState {
                managed: None,
                catalog_pin: catalog.pin().map(str::to_owned),
                supervisor_reads: configured_reads,
                runtime: configured.clone(),
                router: Router::new(queue_capacity)?,
                completed_segments: Vec::new(),
                clock_anchor: None,
                dispatch_order: Vec::new(),
                dispatches: BTreeMap::new(),
                surface_epochs: BTreeMap::new(),
                staged_boundary: None,
                fenced_staged_prefix: None,
                surface_sequences: BTreeMap::new(),
                private_events: BTreeMap::new(),
                speech_temporal: speech_temporal::SpeechTemporal::default(),
                heard_pending: BTreeMap::new(),
            };
            let messages = crate::boundary::inbox(&state.runtime.files)?;
            for index in &state.runtime.inbox_wakes {
                let message = &messages[*index as usize];
                let seq = state.runtime.next_source_seq;
                state.runtime.next_source_seq =
                    seq.checked_add(1).ok_or("source sequence exhausted")?;
                state.router.enqueue(json!({"source":state.runtime.manifest["id"],"kind":"runtime.inbox",
                    "seq":seq,"logical_time":state.runtime.time,"payload":{"index":index,"id":message["id"]},
                    "correlation_id":message["event"]["correlation_id"]}))?;
            }
            state
        };
        // Reconstruct from immutable initial intent order and committed call
        // order, never BTreeMap key order. Upgrade pre-order WALs atomically.
        let mut order = Vec::new();
        for action in setup["initial_actions"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                store
                    .records()
                    .iter()
                    .flat_map(|r| r["actions"].as_array().into_iter().flatten()),
            )
        {
            let id = action["id"]
                .as_str()
                .ok_or("missing ordered action id")?
                .to_owned();
            if order.contains(&id) {
                return Err("duplicate ordered action id".into());
            }
            order.push(id);
        }
        if state
            .runtime
            .pending_actions
            .keys()
            .any(|id| !order.contains(id))
            || state.dispatches.keys().any(|id| !order.contains(id))
            || (!state.dispatch_order.is_empty() && state.dispatch_order != order)
        {
            return Err("durable dispatch call order mismatch".into());
        }
        if state.dispatch_order != order {
            state.dispatch_order = order;
            store.commit(
                &serde_json::to_value(&state).map_err(|e| e.to_string())?,
                None,
            )?;
        }
        // Restart has no live authenticated leases. Keep source highwaters but
        // never deliver queued old-owner observations into a fresh connection.
        if !state.private_events.is_empty() || !state.heard_pending.is_empty() {
            state.heard_pending.clear();
            let revoked = std::mem::take(&mut state.private_events);
            let attempted = Self::attempted_ordinals(&state)?;
            state.router.retain_ordinals(|ordinal| {
                !revoked.contains_key(&ordinal) || attempted.contains(&ordinal)
            });
            store.commit(
                &serde_json::to_value(&state).map_err(|e| e.to_string())?,
                None,
            )?;
        }
        let mut service = Self {
            #[cfg(test)]
            test_clock_sample: None,
            state,
            store,
            catalog,
            #[cfg(feature = "native-engine")]
            configured,
            #[cfg(feature = "native-engine")]
            native: None,
        };
        if let Some((identity, operation_id)) = install {
            gate()?;
            service.managed_install(identity, operation_id)?;
        } else if fresh {
            service.store.commit(
                &serde_json::to_value(&service.state).map_err(|e| e.to_string())?,
                None,
            )?;
        }
        Ok(service)
    }

    /// Number of durably failed guest attempts. Supervisor status, not a wire message.
    pub fn failed_attempts(&self) -> usize {
        self.store
            .records()
            .iter()
            .filter(|r| r["status"] == "aborted")
            .count()
    }

    /// The authenticated surface/router must have authorized external events
    /// before calling this internal coordinator seam.
    pub fn enqueue(&mut self, event: Value) -> Result<u64> {
        if event["kind"].as_str().is_some_and(|kind| {
            kind.starts_with("runtime.")
                || kind == "action.outcome"
                || kind == "llm.reply"
                || self.catalog.single_owner(kind)
        }) {
            return Err("reserved runtime/inbox event cannot enter external event router".into());
        }
        let mut next = self.state.clone();
        let ordinal = next.router.enqueue(event)?;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(ordinal)
    }

    /// Gate shared by every live delivery path (ordinary and database).
    fn require_deliverable(&self) -> Result<()> {
        self.require_managed_running()?;
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err(
                "unresolved engine attempt; authoritative outcome recovery required".into(),
            );
        }
        if !self.store.is_healthy() {
            return Err("runtime requires recovery before guest delivery".into());
        }
        Ok(())
    }

    /// Everything before guest entry: timers, router batch and the exact step
    /// inputs. Touches no durable state except due-timer wakes.
    fn begin_attempt(
        &mut self,
        context: &Value,
    ) -> Result<Option<(Value, Vec<Value>, ServiceState)>> {
        self.expire_private_events(
            context["logical_time"]
                .as_u64()
                .ok_or("missing logical_time")?,
        )?;
        if !self.state.heard_pending.is_empty() {
            return Ok(None);
        }
        self.queue_due_timers(
            context["logical_time"]
                .as_u64()
                .ok_or("missing logical_time")?,
        )?;
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
        if let Some(staged) = next.staged_boundary.take() {
            let mut changes = staged.changes;
            changes.extend(
                inputs["boundary"]
                    .as_array()
                    .ok_or("missing boundary")?
                    .iter()
                    .cloned(),
            );
            inputs["boundary"] = json!(changes);
        }
        inputs["coglet"] = next.runtime.manifest["id"].clone();
        inputs["step"] = json!(next.runtime.step);
        inputs["previous"] = next.runtime.previous.clone();
        inputs["events"] = json!(events);
        Ok(Some((inputs, events, next)))
    }

    /// Host-side consequences of a recorded step, shared by all commit paths.
    fn consume_attempt(next: &mut ServiceState, events: &[Value], result: &Value) -> Result<()> {
        for action in result["journal"]["actions"]
            .as_array()
            .ok_or("missing committed actions")?
        {
            next.dispatch_order
                .push(action["id"].as_str().ok_or("missing action id")?.to_owned());
        }
        next.router.consume(events.len())?;
        next.private_events
            .retain(|_, lease| !events.iter().any(|event| event == &lease.event));
        Ok(())
    }

    pub fn run_next(&mut self, context: &Value, engine: &mut impl Engine) -> Result<Option<Value>> {
        self.require_deliverable()?;
        let Some((inputs, events, mut next)) = self.begin_attempt(context)? else {
            return Ok(None);
        };
        let mut attempt = DurableAttempt {
            engine,
            store: &mut self.store,
            before: &self.state,
            entered: None,
        };
        let result = next.runtime.step(&inputs, &mut attempt);
        let entered = attempt.entered;
        if let Some(inputs) = entered {
            // Also retain the fence on post-engine validation/commit failures.
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
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            Some(&result["journal"]),
        )?;
        self.state = next;
        Ok(Some(result))
    }

    /// Atomically retain timer wake and shared source sequence before execution.
    /// A full router defers firing without discarding or reordering admitted input.
    pub fn queue_due_timers(&mut self, now: u64) -> Result<()> {
        self.require_managed_running()?;
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("timers deferred until attempt reconciliation".into());
        }
        if now < self.state.runtime.time {
            return Err("runtime clock rollback".into());
        }
        let mut next = self.state.clone();
        let mut due: Vec<_> = next
            .runtime
            .pending_timers
            .values()
            .filter(|timer| timer["at"].as_u64().unwrap() <= now)
            .cloned()
            .collect();
        due.sort_by_key(|timer| {
            (
                timer["at"].as_u64().unwrap(),
                timer["id"].as_str().unwrap().to_owned(),
            )
        });
        let mut changed = false;
        for timer in due {
            if next.router.events().any(|event| {
                event["kind"] == "runtime.timer" && event["payload"]["id"] == timer["id"]
            }) {
                continue;
            }
            if next.router.is_full() {
                break;
            }
            let seq = next.runtime.next_source_seq;
            next.runtime.next_source_seq =
                seq.checked_add(1).ok_or("runtime source seq exhausted")?;
            next.router
                .enqueue(json!({"source":next.runtime.manifest["id"],
                "kind":"runtime.timer","seq":seq,"logical_time":now,
                "payload":{"id":timer["id"],"at":timer["at"]},"correlation_id":timer["id"]}))?;
            changed = true;
        }
        if changed {
            next.runtime.time = now;
            if let Some(staged) = &mut next.staged_boundary {
                staged.runtime.time = now;
                staged.runtime.next_source_seq = next.runtime.next_source_seq;
            }
            self.store.commit(
                &serde_json::to_value(&next).map_err(|e| e.to_string())?,
                None,
            )?;
            self.state = next;
        }
        Ok(())
    }

    /// Persist a trailing boundary and any new inbox wakes in one transaction.
    /// Seals this trace segment: promotion/new recording must carry host state.
    pub fn seal_boundary(
        &mut self,
        trace_id: &Value,
        logical_time: u64,
        changes: &[Value],
    ) -> Result<Value> {
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("unresolved attempt prevents sealing".into());
        }
        let old_count = crate::boundary::inbox(&self.state.runtime.files)?.len();
        let mut next = self.state.clone();
        let staged_count = next
            .staged_boundary
            .as_ref()
            .map(|s| {
                s.changes
                    .iter()
                    .filter(|c| c["method"] == "runtime.receipt")
                    .count()
            })
            .unwrap_or(0);
        let mut all_changes = next
            .staged_boundary
            .take()
            .map(|s| s.changes)
            .unwrap_or_default();
        all_changes.extend_from_slice(changes);
        let record = next
            .runtime
            .seal_boundary(trace_id, logical_time, &all_changes)?;
        let messages = crate::boundary::inbox(&next.runtime.files)?;
        for message in &messages[old_count + staged_count..] {
            let seq = next.runtime.next_source_seq;
            next.runtime.next_source_seq =
                seq.checked_add(1).ok_or("runtime source seq exhausted")?;
            next.router.enqueue(json!({"source":next.runtime.manifest["id"],"kind":"runtime.inbox",
                "seq":seq,"logical_time":logical_time,"payload":{"index":message["index"],"id":message["id"]},
                "correlation_id":message["event"]["correlation_id"]}))?;
        }
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            Some(&record),
        )?;
        self.state = next;
        Ok(record)
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
    initial_setup: Value,
    // Archive observation only; never authority to admit a live database.
    #[serde(default)]
    db_revision: Option<u64>,
    #[serde(default)]
    unresolved_attempt: Option<Value>,
    #[serde(default)]
    retained_blobs: serde_json::Map<String, Value>,
    files: Snapshot,
    manifest: Value,
    step: u64,
    time: u64,
    previous: Value,
    // The WAL owns journal history. Repeating it inside every state frame would
    // make durable storage quadratic and eventually exceed transaction bounds.
    #[serde(skip)]
    records: Vec<Value>,
    ledger: EffectLedger,
    inbox_wakes: VecDeque<u64>,
    pending_llm: BTreeMap<String, Value>,
    pending_timers: BTreeMap<String, Value>,
    pending_actions: BTreeMap<String, Value>,
    next_source_seq: u64,
    sealed: bool,
    trace_id: Option<Value>,
}
impl Runtime {
    pub fn from_fixture(setup: &Value) -> Result<Self> {
        // A database deployment pins the explicit 0.2 manifest; 0.1 is unchanged.
        if setup["manifest"]["version"] == "0.2" {
            validate("database::db-manifest", &setup["manifest"])?;
        } else {
            validate("manifest", &setup["manifest"])?;
        }
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
        for kind in ["actions", "timers", "llm_requests"] {
            let ids = setup["initial_effect_ids"][kind].as_array().unwrap();
            if ids
                .windows(2)
                .any(|pair| pair[0].as_str().unwrap() >= pair[1].as_str().unwrap())
            {
                return Err("effect ledger must be sorted and duplicate-free".into());
            }
        }
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
        let mut pending_timers = BTreeMap::new();
        for timer in setup["initial_timers"]
            .as_array()
            .ok_or("missing initial_timers")?
        {
            validate("timer-request", timer)?;
            let id = timer["id"].as_str().unwrap();
            if !ledger["timers"].contains(id)
                || pending_timers.insert(id.into(), timer.clone()).is_some()
            {
                return Err("invalid initial timer ledger".into());
            }
        }
        let mut pending_actions = BTreeMap::new();
        // abi.step fixture setup predates trace headers and omits this field.
        // Trace import must require it explicitly before using this setup seam.
        let empty_actions = json!([]);
        for action in setup
            .get("initial_actions")
            .unwrap_or(&empty_actions)
            .as_array()
            .ok_or("invalid initial_actions")?
        {
            validate("action", action)?;
            let id = action["id"].as_str().unwrap();
            if action["coglet"] != setup["manifest"]["id"]
                || !ledger["actions"].contains(id)
                || pending_actions.insert(id.into(), action.clone()).is_some()
                || inbox.iter().any(|message| {
                    matches!(
                        message["event"]["kind"].as_str(),
                        Some("action.outcome" | "runtime.action_rejected")
                    ) && message["event"]["payload"]["id"] == id
                })
            {
                return Err("invalid initial pending action ledger".into());
            }
        }
        let db_revision = setup
            .get("initial_db_revision")
            .map(|value| value.as_u64().ok_or("invalid initial database revision"))
            .transpose()?;
        let mut runtime = Self {
            db_revision,
            initial_setup: json!({"manifest":setup["manifest"],"initial_files":setup["initial_files"],
                "initial_effect_ids":setup["initial_effect_ids"],"initial_actions":setup.get("initial_actions").unwrap_or(&empty_actions),
                "initial_inbox_wakes":setup["initial_inbox_wakes"],"initial_timers":setup["initial_timers"],
                "initial_llm_requests":setup["initial_llm_requests"]}),
            retained_blobs: serde_json::Map::new(),
            files,
            manifest: setup["manifest"].clone(),
            step: 0,
            time: 0,
            previous: Value::Null,
            records: Vec::new(),
            ledger,
            inbox_wakes,
            pending_llm,
            pending_timers,
            pending_actions,
            unresolved_attempt: None,
            next_source_seq: 0,
            sealed: false,
            trace_id: None,
        };
        if let Some(revision) = db_revision {
            runtime.initial_setup["initial_db_revision"] = json!(revision);
        }
        Ok(runtime)
    }

    /// Compute an attempted step. No dispatch occurs here. Integration with the
    /// durable store must commit returned state + record before external effects.
    pub fn step(&mut self, inputs: &Value, engine: &mut impl Engine) -> Result<Value> {
        if self.unresolved_attempt.is_some() {
            return Err(
                "unresolved engine attempt; authoritative outcome recovery required".into(),
            );
        }
        let mut pinned_inputs = inputs.clone();
        if let Some(metadata) = engine.metadata() {
            pinned_inputs["engine"] = metadata;
        }
        let inputs = &pinned_inputs;
        if self.sealed {
            return Err("trace is sealed; start a new segment".into());
        }
        validate("correlation-id", &inputs["trace_id"])?;
        if self
            .trace_id
            .as_ref()
            .is_some_and(|id| id != &inputs["trace_id"])
        {
            return Err("trace identity changed".into());
        }
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
        let mut boundary_state = self.clone();
        boundary_state
            .apply_boundary_changes(inputs["boundary"].as_array().ok_or("missing boundary")?)?;
        let input = boundary_state.files;
        let mut inbox_wakes = boundary_state.inbox_wakes;
        let mut pending_llm = boundary_state.pending_llm;
        let mut pending_actions = boundary_state.pending_actions;
        let inbox = crate::boundary::inbox(&input)?;
        let mut pending_timers = self.pending_timers.clone();
        let mut next_source_seq = boundary_state.next_source_seq;
        for event in events {
            if event["source"] == inputs["coglet"] {
                next_source_seq = next_source_seq.max(
                    event["seq"]
                        .as_u64()
                        .unwrap()
                        .checked_add(1)
                        .ok_or("runtime source seq exhausted")?,
                );
            }
            if event["kind"] == "runtime.timer" {
                let id = event["payload"]["id"].as_str().ok_or("missing timer ID")?;
                let timer = pending_timers
                    .remove(id)
                    .ok_or("timer wake has no committed pending request")?;
                if event["source"] != inputs["coglet"]
                    || event["correlation_id"] != timer["id"]
                    || event["payload"] != json!({"id":timer["id"],"at":timer["at"]})
                    || timer["at"].as_u64().unwrap() > time
                    || event["logical_time"].as_u64().unwrap() > time
                {
                    return Err("timer wake mismatch or not due".into());
                }
            }
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
        engine.admit(inputs, &host)?;
        self.unresolved_attempt = Some(inputs.clone());
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
        // A recorded ABI 0.2 record re-enters here only to be re-derived from
        // its ordinary host calls (offline/durable reconstruction).
        let recorded_database = inputs.get("version") == Some(&json!("0.2"));
        let observation = if recorded_database {
            None
        } else {
            engine.database_observation(committed)?
        };
        let code_hash = digest(
            &hex::decode(&self.files.files()["/code/bin/step.wasm"]).map_err(|e| e.to_string())?,
        );
        let mut record = json!({
            "version":if observation.is_some() { "0.2" } else { "0.1" },
            "trace_id":inputs["trace_id"], "coglet":inputs["coglet"],
            "step":self.step,"logical_time":time,"seed":inputs["seed"],"previous":self.previous,
            "code_hash":code_hash,
            "manifest_hash":digest(&cbor(&self.manifest)?),"before_hash":before_hash,
            "input_hash":input.tree_hash()?,"output_hash":output.tree_hash()?,
            "engine":inputs["engine"],"budgets":budgets,"grants":inputs["grants"],
            "events":events,"boundary":inputs["boundary"],"calls":if observation.is_some() { vec![] } else { host.calls.clone() },"actions":if committed { host.actions.clone() } else { vec![] },
            "timers":if committed { host.timers.clone() } else { vec![] },
            "llm_requests":if committed { host.llm_requests.clone() } else { vec![] },
            "logs":if committed { host.logs.clone() } else { vec![] },
            "input_files":input.files(),"output_files":output.files(),"usage":report.usage,
            "status":if report.failure.is_some() { "aborted" } else { "committed" }
        });
        if let Some(failure) = report.failure {
            record["failure"] = failure;
        }
        let mut raw_record = None;
        if let Some(observation) = observation {
            // Native 0.2 record: calls stay native values; the JSON record is
            // only the projection derived from the final exact bytes.
            for (key, value) in [
                ("db_before", observation.db_before),
                ("db_after", observation.db_after),
                ("db_schema_version", observation.schema_version),
            ] {
                record[key] = json!(value);
            }
            record["db_catalog_hash"] = json!(observation.catalog_hash);
            let mut native = profile_value(&record, 0)?;
            let coglet_types::Value::Map(entries) = &mut native else {
                return Err("database record is not a map".into());
            };
            let calls = entries
                .iter_mut()
                .find(|(k, _)| matches!(k, coglet_types::Value::Text(t) if t == "calls"))
                .ok_or("missing calls")?;
            calls.1 = coglet_types::Value::Array(observation.calls);
            let hash = digest(&coglet_types::encode_profile(&coglet_types::Value::Map(
                entries.clone(),
            ))?);
            entries.push((
                coglet_types::Value::Text("record_hash".into()),
                coglet_types::Value::Text(hash),
            ));
            let raw = coglet_types::encode("database::db-step-record", &native)?;
            record = crate::journal::database_projection(&raw)?;
            raw_record = Some(raw);
        } else if recorded_database {
            // Host-owned state was re-derived from the recorded ordinary calls.
            // Everything derivable must agree with the (already hash-checked)
            // projection before it is adopted as this runtime's record.
            let recorded = inputs;
            let mut derived = record.clone();
            derived["version"] = json!("0.2");
            for key in [
                "version",
                "trace_id",
                "coglet",
                "step",
                "logical_time",
                "seed",
                "previous",
                "code_hash",
                "manifest_hash",
                "before_hash",
                "input_hash",
                "output_hash",
                "engine",
                "budgets",
                "grants",
                "events",
                "boundary",
                "actions",
                "timers",
                "llm_requests",
                "logs",
                "input_files",
                "output_files",
                "usage",
                "status",
            ] {
                if recorded.get(key) != derived.get(key) {
                    return Err(format!(
                        "recorded database step differs from host state: {key}"
                    ));
                }
            }
            if recorded.get("failure") != derived.get("failure") {
                return Err("recorded database failure differs".into());
            }
            record = recorded.clone();
        } else {
            record["record_hash"] = Value::String(digest(&cbor(&record)?));
            validate("step-record", &record)?;
        }
        let mut result = json!({"state_hash":record["output_hash"],"record_hash":record["record_hash"],"journal":record});
        if let Some(raw) = raw_record {
            result["record_raw"] = json!(hex::encode(raw));
        }
        self.previous = record["record_hash"].clone();
        self.trace_id = Some(inputs["trace_id"].clone());
        self.files = output;
        self.inbox_wakes = inbox_wakes;
        if committed {
            self.ledger = host.ledger;
            for request in host.llm_requests {
                pending_llm.insert(request["id"].as_str().unwrap().into(), request);
            }
            for timer in host.timers {
                pending_timers.insert(timer["id"].as_str().unwrap().into(), timer);
            }
            for action in host.actions {
                pending_actions.insert(action["id"].as_str().unwrap().into(), action);
            }
        }
        self.pending_actions = pending_actions;
        self.pending_timers = pending_timers;
        self.next_source_seq = next_source_seq;
        self.pending_llm = pending_llm;
        self.time = time;
        if record["version"] == "0.2" {
            self.db_revision = Some(record["db_after"].as_u64().ok_or("missing DB revision")?);
        }
        self.step = self.step.checked_add(1).ok_or("step counter exhausted")?;
        self.records.push(record);
        self.unresolved_attempt = None;
        Ok(result)
    }

    fn validate_recovered(&self, records: &[Value]) -> Result<()> {
        for (hash, bytes) in &self.retained_blobs {
            let raw = hex::decode(bytes.as_str().ok_or("invalid retained blob")?)
                .map_err(|e| e.to_string())?;
            if &digest(&raw) != hash {
                return Err("retained blob digest mismatch".into());
            }
        }
        let derived = Self::reconstruct(&self.initial_setup, records)?;
        if self.files != derived.files
            || self
                .db_revision
                .is_some_and(|revision| Some(revision) != derived.db_revision)
            || self.manifest != derived.manifest
            || self.ledger != derived.ledger
            || self.inbox_wakes != derived.inbox_wakes
            || self.pending_actions != derived.pending_actions
            || self.pending_llm != derived.pending_llm
            || self.pending_timers != derived.pending_timers
            || self.next_source_seq < derived.next_source_seq
        {
            return Err("recovered host state differs from journal reconstruction".into());
        }
        // Revalidate semantic host-state constraints; a valid WAL checksum does
        // not establish that decoded pending work or pins are self-consistent.
        Self::from_fixture(
            &json!({"manifest":self.manifest,"initial_files":self.files.files(),
            "initial_effect_ids":self.ledger,"initial_inbox_wakes":self.inbox_wakes,
            "initial_timers":self.pending_timers.values().collect::<Vec<_>>(),
            "initial_actions":self.pending_actions.values().collect::<Vec<_>>(),
            "initial_llm_requests":self.pending_llm.values().collect::<Vec<_>>() }),
        )?;
        let mut previous = Value::Null;
        let mut count = 0u64;
        let mut output_hash = None;
        let mut time = 0;
        let mut sealed = false;
        for record in records {
            if self.trace_id.as_ref() != Some(&record["trace_id"]) {
                return Err("recovered trace identity mismatch".into());
            }
            if sealed
                || record["coglet"] != self.manifest["id"]
                || record["previous"] != previous
                || record["logical_time"].as_u64().unwrap() < time
                || output_hash
                    .as_ref()
                    .is_some_and(|hash| hash != &record["before_hash"])
            {
                return Err("recovered journal chain mismatch".into());
            }
            time = record["logical_time"].as_u64().unwrap();
            output_hash = Some(record["output_hash"].clone());
            if record.get("step").is_some() {
                if record["step"] != count {
                    return Err("recovered step counter mismatch".into());
                }
                count = count.checked_add(1).ok_or("step counter exhausted")?;
                previous = record["record_hash"].clone();
            } else {
                sealed = true;
            }
        }
        if self.step != count
            || self.previous != previous
            || self.time < time
            || self.sealed != sealed
            || output_hash.is_some_and(|hash| hash != self.files.tree_hash().unwrap_or_default())
        {
            return Err("recovered runtime/journal head mismatch".into());
        }
        Ok(())
    }

    // Apply only to a private candidate; callers publish it after validation and
    // persistence. This seam assumes authenticated hub/surface admission.
    fn apply_boundary_changes(&mut self, changes: &[Value]) -> Result<()> {
        for change in changes {
            let old_count = crate::boundary::inbox(&self.files)?.len();
            self.files = crate::boundary::apply(&self.files, change, &self.manifest["id"])?;
            let messages = crate::boundary::inbox(&self.files)?;
            for message in &messages[old_count..] {
                let event = &message["event"];
                if event["source"] == self.manifest["id"] {
                    self.next_source_seq = self.next_source_seq.max(
                        event["seq"]
                            .as_u64()
                            .unwrap()
                            .checked_add(1)
                            .ok_or("runtime source seq exhausted")?,
                    );
                }
                if matches!(
                    event["kind"].as_str(),
                    Some("runtime.action_rejected" | "action.outcome")
                ) {
                    let id = event["payload"]["id"].as_str().unwrap();
                    let action = self
                        .pending_actions
                        .remove(id)
                        .ok_or("terminal message has no pending action")?;
                    if event["correlation_id"] != action["correlation_id"]
                        || event["payload"]["correlation_id"] != action["correlation_id"]
                        || (event["kind"] == "runtime.action_rejected"
                            && event["source"] != self.manifest["id"])
                        || (event["kind"] == "action.outcome"
                            && event["source"] != self.manifest["id"]
                            && !event["source"].as_str().unwrap().starts_with("surface:"))
                    {
                        return Err("terminal message correlation/source mismatch".into());
                    }
                }
                if event["kind"] == "llm.reply" {
                    let id = event["payload"]["request_id"].as_str().unwrap();
                    let request = self
                        .pending_llm
                        .remove(id)
                        .ok_or("LLM reply has no committed pending request")?;
                    if request["correlation_id"] != event["correlation_id"] {
                        return Err("LLM reply correlation mismatch".into());
                    }
                }
            }
            self.inbox_wakes
                .extend((old_count as u64)..(messages.len() as u64));
        }
        Ok(())
    }

    /// Seal an archive segment with durable mutations but NO guest attempt. The
    /// coordinator must commit this returned state and record before ack/export.
    pub fn seal_boundary(
        &mut self,
        trace_id: &Value,
        logical_time: u64,
        changes: &[Value],
    ) -> Result<Value> {
        validate("correlation-id", trace_id)?;
        if self.unresolved_attempt.is_some() {
            return Err("unresolved attempt prevents sealing".into());
        }
        if self.trace_id.as_ref().is_some_and(|id| id != trace_id) {
            return Err("trace identity changed".into());
        }
        if self.sealed || changes.is_empty() || logical_time < self.time {
            return Err("sealed/empty/nonmonotonic trailing boundary".into());
        }
        let mut next = self.clone();
        next.apply_boundary_changes(changes)?;
        let database = self.manifest["version"] == "0.2";
        let mut record = json!({"version":if database { "0.2" } else { "0.1" },"trace_id":trace_id,"coglet":self.manifest["id"],
            "previous":self.previous,"logical_time":logical_time,"boundary":changes,
            "before_hash":self.files.tree_hash()?,"output_hash":next.files.tree_hash()?,"output_files":next.files.files()});
        record["record_hash"] = json!(digest(&cbor(&record)?));
        validate(
            if database {
                "database::db-boundary-record"
            } else {
                "boundary-record"
            },
            &record,
        )?;
        next.sealed = true;
        next.trace_id = Some(trace_id.clone());
        next.time = logical_time;
        next.records.push(record.clone());
        *self = next;
        Ok(record)
    }

    pub fn records(&self) -> &[Value] {
        &self.records
    }

    /// Re-execute a journal with a pinned engine/program. Only recorded step
    /// inputs are passed to the engine, NEVER output files or recorded returns.
    /// This has no dispatch or live-provider/surface callback path.
    pub fn replay(&mut self, records: &[Value], engine: &mut impl Engine) -> Result<Vec<String>> {
        let mut candidate = self.clone();
        let mut hashes = Vec::new();
        for record in records {
            let is_step = record.get("step").is_some();
            validate(
                if is_step {
                    "step-record"
                } else {
                    "boundary-record"
                },
                record,
            )?;
            let mut unhashed = record.clone();
            unhashed.as_object_mut().unwrap().remove("record_hash");
            if record["record_hash"] != digest(&cbor(&unhashed)?) {
                return Err("replay record checksum mismatch".into());
            }
            if is_step && record["status"] == "committed" {
                for field in ["compute", "memory_pages", "host_bytes", "wall_time_us"] {
                    if record["usage"][field].as_u64() > record["budgets"][field].as_u64() {
                        return Err("replay committed record exceeds observed budget".into());
                    }
                }
            }
            if !is_step {
                let actual = candidate.seal_boundary(
                    &record["trace_id"],
                    record["logical_time"].as_u64().unwrap(),
                    record["boundary"].as_array().unwrap(),
                )?;
                if actual != *record {
                    return Err("replay boundary mismatch".into());
                }
                hashes.push(actual["output_hash"].as_str().unwrap().into());
                continue;
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
            let result = candidate.step(&Value::Object(inputs), engine)?;
            // Wall duration is a host observation, not deterministic guest
            // state. Both executions independently enforced their wall limits.
            // Compare every other field, including deterministic usage counters,
            // effects, failure and snapshots; validate the ORIGINAL checksum
            // above and retain its identity for the next recorded chain link.
            let mut actual = result["journal"].clone();
            let mut expected = record.clone();
            for value in [&mut actual, &mut expected] {
                value.as_object_mut().unwrap().remove("record_hash");
                value["usage"]
                    .as_object_mut()
                    .unwrap()
                    .remove("wall_time_us");
            }
            if actual != expected {
                return Err(format!("replay mismatch at step {}", record["step"]));
            }
            candidate.previous = record["record_hash"].clone();
            hashes.push(result["state_hash"].as_str().unwrap().into());
        }
        Ok(hashes)
    }
}
