//! Durable translated envelopes and authenticated receipt reconciliation.
//!
//! This is dispatch metadata, not a replacement for the immutable ABI intent.
use super::*;
use crate::{SurfaceSession, catalog};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Dispatch {
    intent: Value,
    surface: String,
    envelope_hex: String,
    prepared_at: u64,
    terminal_at: Option<u64>,
    accept: Option<Value>,
    outcome: Option<Value>,
}
impl Dispatch {
    fn validate(&self, id: &str, runtime: &Runtime) -> Result<()> {
        validate("action", &self.intent)?;
        validate("surface-id", &json!(self.surface))?;
        if self.intent["id"] != id
            || self.intent["coglet"] != runtime.manifest["id"]
            || !runtime.ledger["actions"].contains(id)
        {
            return Err("invalid recovered dispatch identity".into());
        }
        if runtime
            .pending_actions
            .get(id)
            .is_some_and(|action| action != &self.intent)
        {
            return Err("recovered dispatch differs from committed ABI intent".into());
        }
        let raw = hex::decode(&self.envelope_hex).map_err(|e| e.to_string())?;
        let envelope: Value = coglet_types::decode("surface-message", &raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        let mut original = envelope["body"]["action"].clone();
        original["deadline"] = self.intent["deadline"].clone();
        if envelope["type"] != "act" || original != self.intent || cbor(&envelope)? != raw {
            return Err("recovered translated envelope changed intent".into());
        }
        if let Some(accept) = &self.accept {
            self.check_accept(accept)?;
        }
        if let Some(outcome) = &self.outcome {
            self.check_outcome(outcome)?;
        }
        Ok(())
    }
    fn check_accept(&self, body: &Value) -> Result<()> {
        validate("action-accept", body)?;
        if body["id"] != self.intent["id"]
            || body["correlation_id"] != self.intent["correlation_id"]
        {
            return Err("accept identity/correlation mismatch".into());
        }
        Ok(())
    }
    fn check_outcome(&self, body: &Value) -> Result<()> {
        validate("action-outcome", body)?;
        if self.accept.as_ref().is_none_or(|a| a["accepted"] != true)
            || body["id"] != self.intent["id"]
            || body["correlation_id"] != self.intent["correlation_id"]
        {
            return Err("outcome has no authenticated admission".into());
        }
        if body["status"] == "succeeded" {
            let op = self.intent["capability"].as_str().unwrap();
            catalog::check(
                &catalog::operation(op, "action")?["output"],
                &body["result"],
            )?;
        }
        Ok(())
    }
}
impl RuntimeService {
    fn stage_terminal(next: &mut ServiceState, event: Value, now: u64) -> Result<()> {
        if next.runtime.sealed || now < next.runtime.time {
            return Err("cannot materialize terminal in sealed/past segment".into());
        }
        let mut staged = next
            .staged_boundary
            .take()
            .unwrap_or_else(|| StagedBoundary {
                changes: Vec::new(),
                runtime: next.runtime.clone(),
            });
        let runtime = &mut staged.runtime;
        let index = crate::boundary::inbox(&runtime.files)?.len();
        let identity = digest(&cbor(&json!([
            "runtime.receipt",
            runtime.manifest["id"],
            event["payload"]["id"]
        ]))?)[..32]
            .to_owned();
        let message = json!({"id":identity,"index":index,"sender":event["source"],"event":event});
        let before = runtime.files.files().get("/data/inbox.jsonl").cloned();
        let mut bytes = hex::decode(before.as_deref().unwrap_or("")).map_err(|e| e.to_string())?;
        bytes.extend(serde_json::to_vec(&message).map_err(|e| e.to_string())?);
        bytes.push(b'\n');
        let change = json!({"actor":runtime.manifest["id"],"target":runtime.manifest["id"],"audit_id":identity,"ticket":null,"method":"runtime.receipt","path":"/data/inbox.jsonl","before":before,"after":hex::encode(bytes)});
        runtime.apply_boundary_changes(std::slice::from_ref(&change))?;
        let seq = runtime.next_source_seq;
        runtime.next_source_seq = seq.checked_add(1).ok_or("source sequence exhausted")?;
        next.router.enqueue(json!({"source":runtime.manifest["id"],"kind":"runtime.inbox","seq":seq,"logical_time":now,"payload":{"index":index,"id":identity},"correlation_id":event["correlation_id"]}))?;
        runtime.time = now;
        next.runtime.next_source_seq = runtime.next_source_seq;
        next.runtime.time = now;
        staged.changes.push(change);
        next.staged_boundary = Some(staged);
        Ok(())
    }
    pub(crate) fn surface_epoch(&self, surface: &str) -> u64 {
        self.state.surface_epochs.get(surface).copied().unwrap_or(0)
    }
    pub(crate) fn record_surface_epoch(&mut self, surface: &str, epoch: u64) -> Result<()> {
        if epoch <= self.surface_epoch(surface) {
            return Err("surface epoch did not advance".into());
        }
        let mut next = self.state.clone();
        next.surface_epochs.insert(surface.to_owned(), epoch);
        self.commit_dispatch(next)
    }
    /// Persist BEFORE returning bytes to a transport. An old ID is reconciled
    /// before current grants/clocks are considered, never translated twice.
    pub fn prepare_dispatch(
        &mut self,
        session: &SurfaceSession,
        id: &str,
        now: u64,
    ) -> Result<Vec<u8>> {
        session.check_reconciliation_binding(
            self.state.runtime.manifest["id"].as_str().unwrap(),
            now,
        )?;
        if let Some(saved) = self.state.dispatches.get(id) {
            if saved.surface != session.surface() {
                return Err("dispatch belongs to another authenticated surface".into());
            }
            // Completed records may expire at the surface. Never silently retry
            // beyond even the minimum guaranteed retention window.
            let retained_until = saved.intent["deadline"]
                .as_u64()
                .unwrap()
                .max(saved.terminal_at.unwrap_or(0))
                .saturating_add(86_400_000_000);
            if now < saved.prepared_at || now >= retained_until {
                return Err("dispatch requires explicit retention reconciliation".into());
            }
            return hex::decode(&saved.envelope_hex).map_err(|e| e.to_string());
        }
        let intent = self
            .state
            .runtime
            .pending_actions
            .get(id)
            .ok_or("action is not durably pending")?;
        if self.surface_epoch(session.surface()) != 0
            && self.surface_epoch(session.surface()) != session.grant_epoch()
        {
            return Err("stale surface grant epoch".into());
        }
        let envelope = session.translate_action(intent, now)?;
        let raw = cbor(&envelope)?;
        if raw.len() > crate::SURFACE_MAX_FRAME {
            return Err(
                "resource_exhausted: translated envelope exceeds surface frame bound".into(),
            );
        }
        let dispatch = Dispatch {
            intent: intent.clone(),
            surface: session.surface().to_owned(),
            envelope_hex: hex::encode(&raw),
            prepared_at: now,
            terminal_at: None,
            accept: None,
            outcome: None,
        };
        let mut next = self.state.clone();
        next.dispatches.insert(id.into(), dispatch);
        self.commit_dispatch(next)?;
        Ok(raw)
    }
    /// Persist authenticated admission. Positive accept is bookkeeping only;
    /// rejection atomically materializes one terminal inbox append and wake.
    pub fn record_surface_accept(
        &mut self,
        session: &SurfaceSession,
        body: &Value,
        now: u64,
    ) -> Result<()> {
        session.check_reconciliation_binding(
            self.state.runtime.manifest["id"].as_str().unwrap(),
            now,
        )?;
        let id = body["id"].as_str().ok_or("missing action ID")?;
        let mut next = self.state.clone();
        let dispatch = next
            .dispatches
            .get_mut(id)
            .ok_or("accept without retained dispatch")?;
        if dispatch.surface != session.surface() {
            return Err("accept from wrong authenticated surface".into());
        }
        dispatch.check_accept(body)?;
        if let Some(saved) = &dispatch.accept {
            return if saved == body {
                Ok(())
            } else {
                Err("conflicting historical accept".into())
            };
        }
        dispatch.accept = Some(body.clone());
        if body["accepted"] == false {
            dispatch.terminal_at = Some(now);
            let event = json!({"source":next.runtime.manifest["id"],"kind":"runtime.action_rejected","seq":next.runtime.next_source_seq,"logical_time":now,"payload":body,"correlation_id":body["correlation_id"]});
            Self::stage_terminal(&mut next, event, now)?;
        }
        self.commit_dispatch(next)
    }
    /// Atomically retain the authenticated terminal payload, inbox append, pending
    /// removal and one wake. Ignore fresh delivery seq on identical historical
    /// retries; conflicting payloads are fatal rather than a second guest append.
    pub fn record_surface_outcome(
        &mut self,
        session: &SurfaceSession,
        event: &Value,
        now: u64,
    ) -> Result<bool> {
        session.check_reconciliation_binding(
            self.state.runtime.manifest["id"].as_str().unwrap(),
            now,
        )?;
        validate("event", event)?;
        if event["source"] != session.surface()
            || event["kind"] != "action.outcome"
            || event["correlation_id"] != event["payload"]["correlation_id"]
        {
            return Err("unauthenticated outcome source/correlation".into());
        }
        let id = event["payload"]["id"].as_str().ok_or("missing action ID")?;
        let mut next = self.state.clone();
        let dispatch = next
            .dispatches
            .get_mut(id)
            .ok_or("outcome without retained dispatch")?;
        if dispatch.surface != session.surface() {
            return Err("outcome from wrong authenticated surface".into());
        }
        dispatch.check_outcome(&event["payload"])?;
        if let Some(saved) = &dispatch.outcome {
            return if saved == &event["payload"] {
                Ok(false)
            } else {
                Err("conflicting terminal result".into())
            };
        }
        dispatch.outcome = Some(event["payload"].clone());
        dispatch.terminal_at = Some(now);
        Self::stage_terminal(&mut next, event.clone(), now)?;
        self.commit_dispatch(next)?;
        Ok(true)
    }
    fn commit_dispatch(&mut self, next: ServiceState) -> Result<()> {
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
    pub(super) fn validate_dispatches(state: &ServiceState) -> Result<()> {
        for (surface, epoch) in &state.surface_epochs {
            validate("surface-id", &json!(surface))?;
            if *epoch == 0 {
                return Err("invalid recovered surface epoch".into());
            }
        }
        for (id, dispatch) in &state.dispatches {
            dispatch.validate(id, &state.runtime)?;
        }
        Ok(())
    }
}
