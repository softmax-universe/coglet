//! Queue-side recovery constraints in addition to journal host reconstruction.
use super::*;
use std::collections::BTreeSet;

impl RuntimeService {
    pub(super) fn attempted_ordinals(state: &ServiceState) -> Result<BTreeSet<u64>> {
        let Some(inputs) = &state.runtime.unresolved_attempt else {
            return Ok(BTreeSet::new());
        };
        let events = inputs["events"]
            .as_array()
            .ok_or("missing attempted batch")?;
        if events.is_empty() || state.router.batch(events.len())? != *events {
            return Err("attempt reservation differs from durable router prefix".into());
        }
        if inputs["coglet"] != state.runtime.manifest["id"]
            || inputs["step"] != state.runtime.step
            || inputs["previous"] != state.runtime.previous
            || state
                .runtime
                .trace_id
                .as_ref()
                .is_some_and(|id| id != &inputs["trace_id"])
            || inputs["logical_time"]
                .as_u64()
                .is_none_or(|t| t < state.runtime.time)
        {
            return Err("attempt reservation differs from durable journal head".into());
        }
        // Run the same no-guest preflight as a live step on a disposable clone.
        // The probe stops at admission; it cannot consume or execute an attempt.
        struct Probe(bool);
        impl Engine for Probe {
            fn admit(&mut self, _: &Value, _: &StepHost) -> Result<()> {
                self.0 = true;
                Err("recovery preflight complete".into())
            }
            fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
                unreachable!("recovery preflight cannot execute guest")
            }
        }
        validate("engine", &inputs["engine"])?;
        inputs["seed"].as_u64().ok_or("invalid attempted seed")?;
        for grant in inputs["grants"]
            .as_array()
            .ok_or("invalid attempted grants")?
        {
            validate("capability-name", grant)?;
        }
        let mut candidate = state.runtime.clone();
        candidate.unresolved_attempt = None;
        let mut probe = Probe(false);
        let result = candidate.step(inputs, &mut probe);
        if !probe.0 {
            return Err(format!("invalid attempted input: {}", result.unwrap_err()));
        }
        Ok(state
            .router
            .entries()
            .take(events.len())
            .map(|(n, _)| *n)
            .collect())
    }
    /// Reconcile a fenced attempt from the original worker's authenticated,
    /// complete journal observation. The caller owns authentication/custody;
    /// this API checks identity and all host transitions, NOT guest execution.
    /// No missing usage is synthesized and no guest is invoked. Without such
    /// evidence the attempt remains fenced indefinitely.
    pub fn recover_attempt(&mut self, record: &Value) -> Result<()> {
        let historical = legacy_version(&self.state.runtime.manifest["version"]);
        historical_scope(historical, || self.recover_attempt_inner(record))?;
        self.upgrade_hash_if_ready()
    }

    fn recover_attempt_inner(&mut self, record: &Value) -> Result<()> {
        if !self.store.is_healthy() {
            return Err("WAL must be reopened before attempt reconciliation".into());
        }
        let next = Self::reconcile_attempt(&self.state, self.store.records(), record)?;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            Some(record),
        )?;
        self.state = next;
        Ok(())
    }

    /// Pure host-state transition for an authenticated attempt observation:
    /// `journal` is the complete WAL record list. Shared by ABI 0.1 recovery and
    /// by the ABI 0.2 journal collaborator so both derive state identically. The
    /// record may be a 0.2 projection, never an unvalidated external value.
    pub(super) fn reconcile_attempt(
        state: &ServiceState,
        journal: &[Value],
        record: &Value,
    ) -> Result<ServiceState> {
        Self::attempted_ordinals(state)?;
        let inputs = state
            .runtime
            .unresolved_attempt
            .as_ref()
            .ok_or("no unresolved engine attempt")?;
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
            if record.get(key) != inputs.get(key) {
                return Err(format!("recovered attempt input mismatch: {key}"));
            }
        }
        let count = inputs["events"]
            .as_array()
            .ok_or("missing attempted events")?
            .len();
        if state.router.batch(count)? != *inputs["events"].as_array().unwrap() {
            return Err("recovered attempt no longer matches retained router prefix".into());
        }
        let offset = state.completed_segments.last().map_or(0, |s| s.end);
        let mut records = journal[offset..].to_vec();
        records.push(record.clone());
        let mut recovered = Runtime::reconstruct(&state.runtime.initial_setup, &records)?;
        recovered.next_source_seq = recovered.next_source_seq.max(state.runtime.next_source_seq);
        recovered.time = recovered.time.max(state.runtime.time);
        recovered.retained_blobs = state.runtime.retained_blobs.clone();
        let mut next = state.clone();
        next.runtime = recovered;
        next.staged_boundary = None;
        next.fenced_staged_prefix = None;
        next.router.consume(count)?;
        next.private_events
            .retain(|_, lease| !inputs["events"].as_array().unwrap().contains(&lease.event));
        for action in record["actions"]
            .as_array()
            .ok_or("missing recovered actions")?
        {
            next.dispatch_order
                .push(action["id"].as_str().ok_or("missing action ID")?.to_owned());
        }
        Ok(next)
    }

    /// Every own-source reservation is retained either in a consumed batch,
    /// the router, or a host-appended inbox line. Segment headers are not a
    /// sequence authority; examine the entire immutable WAL, not just its tail.
    pub(super) fn validate_source_reservations(
        state: &ServiceState,
        root: &Runtime,
        records: &[Value],
    ) -> Result<()> {
        let owner = &state.runtime.manifest["id"];
        let mut sequences = BTreeSet::new();
        let mut observe = |event: &Value| -> Result<()> {
            if &event["source"] == owner {
                let seq = event["seq"].as_u64().ok_or("invalid source sequence")?;
                if !sequences.insert(seq) {
                    return Err("recovered source reservation reused".into());
                }
            }
            Ok(())
        };
        for record in records {
            for event in record["events"].as_array().into_iter().flatten() {
                observe(event)?;
            }
        }
        for event in state.router.events() {
            observe(event)?;
        }
        let runtime = state
            .staged_boundary
            .as_ref()
            .map(|s| &s.runtime)
            .unwrap_or(&state.runtime);
        // Initial inbox contents are supplied state, not reservations made by
        // this coordinator. Later lines cannot be removed or rewritten.
        let initial_count = crate::boundary::inbox(&root.files)?.len();
        let mut external_reservations = BTreeSet::new();
        for message in crate::boundary::inbox(&runtime.files)?
            .iter()
            .skip(initial_count)
        {
            let event = &message["event"];
            observe(event)?;
            if &event["source"] == owner {
                // Authenticated boundary input can advance its producer's
                // source sequence; generated timer/inbox wakes cannot do so.
                external_reservations.insert(event["seq"].as_u64().unwrap());
            }
        }
        let mut next = 0;
        for seq in sequences {
            if seq != next && !external_reservations.contains(&seq) {
                return Err("recovered source reservations have a gap".into());
            }
            next = seq.checked_add(1).ok_or("source sequence exhausted")?;
        }
        if state.runtime.next_source_seq != next || runtime.next_source_seq != next {
            return Err("recovered source highwater differs from retained reservations".into());
        }
        Ok(())
    }

    pub(super) fn validate_queued_wakes(state: &ServiceState) -> Result<()> {
        let runtime = state
            .staged_boundary
            .as_ref()
            .map(|s| &s.runtime)
            .unwrap_or(&state.runtime);
        let inbox = crate::boundary::inbox(&runtime.files)?;
        let mut inbox_indices = Vec::new();
        let mut timers = BTreeSet::new();
        let mut source_sequences = BTreeSet::new();
        for event in state.router.events() {
            let kind = event["kind"].as_str().ok_or("missing event kind")?;
            if matches!(kind, "action.outcome" | "llm.reply") {
                return Err("recovered direct terminal/provider event bypasses inbox".into());
            }
            if event["source"] == runtime.manifest["id"] {
                let seq = event["seq"].as_u64().ok_or("missing wake sequence")?;
                if seq >= runtime.next_source_seq || !source_sequences.insert(seq) {
                    return Err("recovered wake sequence is unreserved/duplicate".into());
                }
            }
            match kind {
                "runtime.inbox" => {
                    let index = event["payload"]["index"]
                        .as_u64()
                        .ok_or("missing inbox index")?;
                    let message = inbox
                        .get(usize::try_from(index).map_err(|_| "inbox index overflow")?)
                        .ok_or("recovered inbox wake has no line")?;
                    if event["source"] != runtime.manifest["id"]
                        || !runtime.inbox_wakes.contains(&index)
                        || event["payload"] != json!({"index":index,"id":message["id"]})
                        || event["correlation_id"] != message["event"]["correlation_id"]
                        || inbox_indices
                            .last()
                            .is_some_and(|previous| *previous >= index)
                    {
                        return Err("recovered inbox wake binding/order mismatch".into());
                    }
                    inbox_indices.push(index);
                }
                "runtime.timer" => {
                    let id = event["payload"]["id"].as_str().ok_or("missing timer ID")?;
                    let timer = runtime
                        .pending_timers
                        .get(id)
                        .ok_or("recovered timer wake has no pending request")?;
                    if event["source"] != runtime.manifest["id"]
                        || event["payload"] != json!({"id":timer["id"],"at":timer["at"]})
                        || event["correlation_id"] != timer["id"]
                        || timer["at"].as_u64().unwrap() > event["logical_time"].as_u64().unwrap()
                        || event["logical_time"].as_u64().unwrap() > runtime.time
                        || !timers.insert(id)
                    {
                        return Err("recovered timer wake binding/time mismatch".into());
                    }
                }
                _ if kind.starts_with("runtime.") => {
                    return Err("unrecognized queued runtime event".into());
                }
                _ => (),
            }
        }
        if inbox_indices != runtime.inbox_wakes.iter().copied().collect::<Vec<_>>() {
            return Err("recovered pending inbox wake missing from router".into());
        }
        Ok(())
    }
}
