//! Queue-side recovery constraints in addition to journal host reconstruction.
use super::*;
use std::collections::BTreeSet;

impl RuntimeService {
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
        Ok(())
    }
}
