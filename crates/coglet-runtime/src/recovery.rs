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
        Self::attempted_ordinals(&self.state)?;
        if !self.store.is_healthy() {
            return Err("WAL must be reopened before attempt reconciliation".into());
        }
        let inputs = self
            .state
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
        if self.state.router.batch(count)? != *inputs["events"].as_array().unwrap() {
            return Err("recovered attempt no longer matches retained router prefix".into());
        }
        let mut records = self.store.records()[self.segment_offset()..].to_vec();
        records.push(record.clone());
        let mut recovered = Runtime::reconstruct(&self.state.runtime.initial_setup, &records)?;
        recovered.next_source_seq = recovered
            .next_source_seq
            .max(self.state.runtime.next_source_seq);
        recovered.time = recovered.time.max(self.state.runtime.time);
        recovered.retained_blobs = self.state.runtime.retained_blobs.clone();
        let mut next = self.state.clone();
        next.runtime = recovered;
        next.staged_boundary = None;
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
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            Some(record),
        )?;
        self.state = next;
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
