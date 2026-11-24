//! Durable translated envelopes and authenticated receipt reconciliation.
//!
//! This is dispatch metadata, not a replacement for the immutable ABI intent.
use super::*;
use crate::{SurfaceSession, TrustedExtensionCatalog};
#[cfg(test)]
#[path = "dispatch_tests.rs"]
mod tests;

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
    pub(super) fn matches_tool_result(&self, surface: &str, args: &Value) -> bool {
        self.surface == surface
            && self.accept.as_ref().is_none_or(|v| v["accepted"] != false)
            && self.intent["capability"] == "speech.tool_result"
            && self.intent["args"]["session"] == args["session"]
            && self.intent["args"]["call"] == args["call"]
    }
    fn validate(
        &self,
        id: &str,
        runtime: &Runtime,
        catalog: &TrustedExtensionCatalog,
    ) -> Result<()> {
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
            self.check_outcome(outcome, catalog)?;
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
    fn check_outcome(&self, body: &Value, catalog: &TrustedExtensionCatalog) -> Result<()> {
        validate("action-outcome", body)?;
        if self.accept.as_ref().is_none_or(|a| a["accepted"] != true)
            || body["id"] != self.intent["id"]
            || body["correlation_id"] != self.intent["correlation_id"]
        {
            return Err("outcome has no authenticated admission".into());
        }
        if body["status"] == "succeeded" {
            let op = self.intent["capability"].as_str().unwrap();
            catalog.check(op, "action", true, &body["result"])?;
        }
        Ok(())
    }
}
impl RuntimeService {
    pub(crate) fn admitted_pending(&self, surface: &str) -> usize {
        self.state
            .dispatches
            .values()
            .filter(|d| {
                d.surface == surface
                    && d.accept.as_ref().is_some_and(|a| a["accepted"] == true)
                    && d.terminal_at.is_none()
            })
            .count()
    }
    /// Retained IDs reconcile before current authority checks. New intents
    /// without authority block later intents rather than reordering call order.
    pub(crate) fn next_dispatch(
        &mut self,
        session: &SurfaceSession,
        sent: &std::collections::BTreeSet<String>,
        now: u64,
    ) -> Result<Option<(String, Vec<u8>)>> {
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery".into());
        }
        for id in self.state.dispatch_order.clone() {
            if sent.contains(&id) {
                continue;
            }
            if let Some(saved) = self.state.dispatches.get(&id) {
                if saved.terminal_at.is_some() || saved.surface != session.surface() {
                    continue;
                }
            } else {
                // Keep already prepared IDs reconcilable, but do not admit a
                // new effect or materialize a terminal over an unknown attempt.
                if self.state.runtime.unresolved_attempt.is_some() {
                    continue;
                }
                let pending = self
                    .state
                    .staged_boundary
                    .as_ref()
                    .map(|s| &s.runtime)
                    .unwrap_or(&self.state.runtime);
                let Some(action) = pending.pending_actions.get(&id).cloned() else {
                    continue;
                };
                if action["deadline"].as_u64().is_some_and(|d| d <= now) {
                    self.reject_undispatched(&action, now, "deadline_exceeded")?;
                    continue;
                }
                if session
                    .authorize(
                        action["coglet"].as_str().unwrap(),
                        action["capability"].as_str().unwrap(),
                        "action",
                        now,
                    )
                    .is_err()
                {
                    return Ok(None);
                }
                if session.translate_action(&action, now).is_err()
                    || self.check_new_provider_action(session, &action).is_err()
                {
                    self.reject_undispatched(&action, now, "invalid_argument")?;
                    continue;
                }
            }
            let bytes = self.prepare_dispatch(session, &id, now)?;
            return Ok(Some((id, bytes)));
        }
        Ok(None)
    }
    fn reject_undispatched(&mut self, action: &Value, now: u64, code: &str) -> Result<()> {
        let mut next = self.state.clone();
        let payload = json!({"id":action["id"],"correlation_id":action["correlation_id"],"accepted":false,
            "error":{"code":code,"message":"Runtime dispatch refused","retryable":false}});
        let event = json!({"source":action["coglet"],"kind":"runtime.action_rejected",
            "seq":next.runtime.next_source_seq,"logical_time":now,"payload":payload,
            "correlation_id":action["correlation_id"]});
        Self::stage_terminal(&mut next, event, now)?;
        self.commit_dispatch(next)
    }
    fn stage_terminal(next: &mut ServiceState, event: Value, now: u64) -> Result<()> {
        if next.runtime.unresolved_attempt.is_some() {
            return Err("terminal receipt deferred until attempt reconciliation".into());
        }
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
        self.purge_surface_events(surface, self.surface_epoch(surface))?;
        next.router = self.state.router.clone();
        next.private_events = self.state.private_events.clone();
        next.heard_pending = self.state.heard_pending.clone();
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
        if !self.store.is_healthy() {
            return Err("runtime requires WAL recovery".into());
        }
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
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("new dispatch deferred until attempt reconciliation".into());
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
        self.check_new_provider_action(session, intent)?;
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
        Self::advance_surface_sequence(&mut next, event)?;
        let dispatch = next
            .dispatches
            .get_mut(id)
            .ok_or("outcome without retained dispatch")?;
        if dispatch.surface != session.surface() {
            return Err("outcome from wrong authenticated surface".into());
        }
        dispatch.check_outcome(&event["payload"], &self.catalog)?;
        if let Some(saved) = &dispatch.outcome {
            if saved != &event["payload"] {
                return Err("conflicting terminal result".into());
            }
            self.commit_dispatch(next)?;
            return Ok(false);
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
    pub(crate) fn check_heard_owner(
        &self,
        session: &SurfaceSession,
        coglet: &str,
        id: &str,
    ) -> Result<()> {
        if let Some(provider) = self
            .state
            .speech_temporal
            .playback
            .provider(session.surface(), id)
        {
            self.state.speech_temporal.playback.check_provider_owner(
                session.surface(),
                id,
                coglet,
                session.grant_epoch(),
            )?;
            return self.check_provider_session(session, coglet, &json!(provider));
        }
        let d = self
            .state
            .dispatches
            .get(id)
            .ok_or("not_found: no retained item owner")?;
        if d.surface != session.surface()
            || d.intent["coglet"] != coglet
            || !matches!(
                d.intent["capability"].as_str(),
                Some("speech.speak_chunk" | "speech.backchannel")
            )
            || d.accept.as_ref().is_none_or(|v| v["accepted"] != true)
        {
            return Err("permission_denied: not the item's speaker owner".into());
        }
        let raw = hex::decode(&d.envelope_hex).map_err(|e| e.to_string())?;
        let envelope: Value = coglet_types::decode("surface-message", &raw)?
            .deserialized()
            .map_err(|e| e.to_string())?;
        if envelope["body"]["grant_epoch"] != session.grant_epoch() {
            return Err("permission_denied: item owner revoked".into());
        }
        Ok(())
    }
    pub(super) fn playback_owner(
        &self,
        session: &SurfaceSession,
        coglet: &str,
        payload: &Value,
    ) -> Result<Option<String>> {
        let id = payload["item"].as_str().ok_or("missing playback item")?;
        if let Some(d) = self.state.dispatches.get(id) {
            self.check_heard_owner(session, coglet, id)?;
            if d.intent["args"]["stream"] != payload["stream"]
                || (payload.get("chunk").is_some()
                    && payload["chunk"]
                        != d.intent["args"].get("chunk").cloned().unwrap_or(json!(0)))
            {
                return Err("playback does not match admitted stream/chunk".into());
            }
            return Ok(None);
        }
        if let Some(provider) = self
            .state
            .speech_temporal
            .playback
            .provider(session.surface(), id)
        {
            self.check_provider_session(session, coglet, &json!(provider))?;
            return Ok(Some(provider.to_owned()));
        }
        // Frozen playback has no session field. Only an unambiguous live retained
        // provider session may establish native ownership; never invent admission.
        let candidates: Vec<String> = self
            .state
            .dispatches
            .values()
            .filter(|d| {
                d.surface == session.surface() && d.intent["capability"] == "speech.session_open"
            })
            .filter_map(|d| d.intent["args"]["session"].as_str())
            .map(str::to_owned)
            .collect();
        if candidates.len() != 1 {
            return Err("native item has no unambiguous live provider owner".into());
        }
        let provider = candidates.into_iter().next().unwrap();
        self.check_provider_session(session, coglet, &json!(provider))?;
        Ok(Some(provider))
    }
    fn check_new_provider_action(&self, session: &SurfaceSession, intent: &Value) -> Result<()> {
        if self
            .state
            .speech_temporal
            .playback
            .provider(
                session.surface(),
                intent["id"].as_str().ok_or("missing action ID")?,
            )
            .is_some()
        {
            return Err("conflict: action ID reuses a native provider item".into());
        }
        if intent["capability"] == "speech.tool_result" {
            self.check_provider_session(
                session,
                intent["coglet"].as_str().unwrap(),
                &intent["args"]["session"],
            )?;
            self.state.speech_temporal.check_tool_result(
                session.surface(),
                &intent["args"],
                &self.state.dispatches,
            )?;
        }
        Ok(())
    }
    pub(super) fn check_provider_session(
        &self,
        session: &SurfaceSession,
        coglet: &str,
        id: &Value,
    ) -> Result<()> {
        let matching = |d: &&Dispatch| {
            d.surface == session.surface()
                && d.intent["coglet"] == coglet
                && d.intent["args"]["session"] == *id
        };
        let opened = self.state.dispatches.values().filter(matching).any(|d| {
            d.intent["capability"] == "speech.session_open"
                && d.outcome
                    .as_ref()
                    .is_some_and(|v| v["status"] == "succeeded")
                && hex::decode(&d.envelope_hex)
                    .ok()
                    .and_then(|raw| coglet_types::decode("surface-message", &raw).ok())
                    .and_then(|v| v.deserialized::<Value>().ok())
                    .is_some_and(|v| v["body"]["grant_epoch"] == session.grant_epoch())
        });
        let closed = self.state.dispatches.values().filter(matching).any(|d| {
            d.intent["capability"] == "speech.session_close"
                && d.accept.as_ref().is_some_and(|v| v["accepted"] == true)
        });
        if !opened || closed {
            return Err("private provider event has no live owner session".into());
        }
        Ok(())
    }
    pub(super) fn validate_dispatches(
        state: &ServiceState,
        catalog: &TrustedExtensionCatalog,
    ) -> Result<()> {
        for surface in state.surface_sequences.keys() {
            validate("surface-id", &json!(surface))?;
        }
        for (surface, epoch) in &state.surface_epochs {
            validate("surface-id", &json!(surface))?;
            if *epoch == 0 {
                return Err("invalid recovered surface epoch".into());
            }
        }
        for (id, dispatch) in &state.dispatches {
            dispatch.validate(id, &state.runtime, catalog)?;
        }
        Ok(())
    }
}
