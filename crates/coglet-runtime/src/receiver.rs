//! Durable back-channel mutation coordinator. Only the authenticated device
//! handler may admit/apply these requests; this is not an authorization API.
use super::*;

#[path = "receiver_control.rs"]
mod control;
#[cfg(test)]
#[path = "receiver_tests.rs"]
mod tests;

const LEDGER_LIMIT: usize = 4096;
const PENDING_LIMIT: usize = 256;
const PENDING_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReceiverState {
    #[serde(default)]
    active_step: Option<String>,
    #[serde(default)]
    retired_wakes: Vec<Value>,
    #[serde(default)]
    cancellations: BTreeMap<String, ReceiverCancellation>,
    #[serde(default)]
    rejected_actions: BTreeMap<String, ReceiverRejectedAction>,
    operations: BTreeMap<String, ReceiverOperation>,
    pending: Vec<String>,
    messages: BTreeMap<String, String>,
    audit: Vec<Value>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiverOperation {
    #[serde(default)]
    response: Option<Value>,
    intent: String,
    request: Value,
    scope: ReceiptScope,
    receipt: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiverCancellation {
    request: Value,
    receipt: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReceiverRejectedAction {
    request: Value,
    accept: Value,
}

impl ReceiverState {
    fn ledger_len(&self) -> usize {
        self.operations.len() + self.cancellations.len() + self.rejected_actions.len()
    }
    pub(super) fn validate(&self) -> Result<()> {
        if self.ledger_len() > LEDGER_LIMIT
            || self.pending.len() > PENDING_LIMIT
            || self.retired_wakes.len() > LEDGER_LIMIT
        {
            return Err("invalid recovered receiver bounds".into());
        }
        for (id, saved) in &self.rejected_actions {
            validate("hub-request", &saved.request)?;
            validate("action-accept", &saved.accept)?;
            let action = &saved.request["params"]["action"];
            if saved.request["method"] != "hub.actions"
                || saved.request["correlation_id"] != *id
                || saved.accept["id"] != action["id"]
                || saved.accept["correlation_id"] != action["correlation_id"]
                || saved.accept["accepted"] != false
                || saved.accept["error"]["code"] != "deadline_exceeded"
                || self.operations.contains_key(id)
                || self.cancellations.contains_key(id)
            {
                return Err("invalid recovered action rejection".into());
            }
        }
        for (id, saved) in &self.cancellations {
            validate("hub-request", &saved.request)?;
            validate("hub-mutation-receipt", &saved.receipt)?;
            if saved.request["method"] != "hub.cancel"
                || saved.request["correlation_id"] != *id
                || saved.request["params"]["operation_id"] != saved.receipt["operation_id"]
                || saved.receipt["state"] != "cancelled"
                || self.operations.contains_key(id)
            {
                return Err("invalid recovered cancellation".into());
            }
        }
        let mut queued = std::collections::BTreeSet::new();
        let mut bytes = 0usize;
        for id in &self.pending {
            let saved = self
                .operations
                .get(id)
                .ok_or("missing receiver operation")?;
            if !queued.insert(id) || saved.receipt["state"] != "queued" {
                return Err("invalid receiver queue".into());
            }
            bytes = bytes
                .checked_add(cbor(&saved.request)?.len())
                .ok_or("receiver byte overflow")?;
        }
        if bytes > PENDING_BYTES {
            return Err("invalid recovered receiver byte bound".into());
        }
        for (id, saved) in &self.operations {
            validate("hub-request", &saved.request)?;
            validate("hub-mutation-receipt", &saved.receipt)?;
            if let Some(response) = &saved.response {
                validate("hub-step-result", response)?;
                if saved.scope.method != "debug.step"
                    || response["receipt"] != saved.receipt
                    || saved.receipt["state"] == "queued"
                {
                    return Err("invalid recovered step response".into());
                }
            } else if saved.scope.method == "debug.step" && saved.receipt["state"] != "queued" {
                return Err("missing recovered step response".into());
            }
            if saved.request["correlation_id"] != *id
                || saved.receipt["operation_id"] != *id
                || saved.intent != receiver_intent(&saved.request)?
                || saved.scope != receiver_scope(&saved.request)
                || (saved.receipt["state"] == "queued") != queued.contains(id)
            {
                return Err("invalid recovered receiver receipt binding".into());
            }
        }
        if let Some(id) = &self.active_step
            && (self.pending.first() != Some(id)
                || !self
                    .operations
                    .get(id)
                    .is_some_and(|op| op.scope.method == "debug.step"))
        {
            return Err("invalid recovered active step".into());
        }
        for event in &self.retired_wakes {
            validate("event", event)?;
            let id = event["correlation_id"]
                .as_str()
                .ok_or("invalid retired wake")?;
            if event["kind"] != "runtime.debug_step"
                || event["payload"] != json!({})
                || !self.operations.get(id).is_some_and(|op| {
                    op.scope.method == "debug.step"
                        && matches!(op.receipt["state"].as_str(), Some("failed" | "cancelled"))
                })
            {
                return Err("invalid recovered retired wake".into());
            }
        }
        Ok(())
    }
}

pub(crate) fn receiver_scope(request: &Value) -> ReceiptScope {
    let context = if matches!(
        request["method"].as_str(),
        Some("debug.write" | "hub.inbox")
    ) {
        &request["params"]
    } else {
        &request["context"]
    };
    ReceiptScope {
        actor: context["actor"].as_str().unwrap_or_default().into(),
        ticket: context["ticket"].as_str().map(str::to_owned),
        method: request["method"].as_str().unwrap_or_default().into(),
    }
}

fn receiver_intent(request: &Value) -> Result<String> {
    Ok(digest(&cbor(
        &json!({"method":request["method"],"target":request["target"],
        "params":request["params"],"context":request["context"]}),
    )?))
}

fn mutation_receipt(id: &str, state: &str, step: Option<u64>, error: Option<&str>) -> Value {
    json!({"operation_id":id,"state":state,"step":step,
        "error":error.map(|code| json!({"code":code,"message":code,"retryable":false}))})
}

impl RuntimeService {
    pub(crate) fn receiver_pending_len(&self) -> usize {
        self.state.receiver.pending.len()
    }

    pub(crate) fn receiver_contains(&self, id: &str) -> bool {
        self.state.receiver.operations.contains_key(id)
            || self.state.receiver.cancellations.contains_key(id)
            || self.state.receiver.rejected_actions.contains_key(id)
    }

    /// Expiry is decided here, under fresh router authority and the coordinator
    /// mutex, before adapter preparation. Nonexpired actions are not admitted by
    /// this rejection-only path: it cannot fabricate an adapter acceptance.
    pub(crate) fn receiver_action_rejection(&mut self, request: &Value) -> Result<Value> {
        let id = request["correlation_id"]
            .as_str()
            .ok_or("invalid_argument")?;
        let action = &request["params"]["action"];
        if let Some(saved) = self.state.receiver.rejected_actions.get(id) {
            return if receiver_intent(&saved.request)? == receiver_intent(request)? {
                Ok(saved.accept.clone())
            } else {
                Err("conflict".into())
            };
        }
        if self.receiver_contains(id)
            || self.managed_contains_lifecycle_operation(id)
            || self.receiver_snapshot().ledger["actions"]
                .contains(action["id"].as_str().ok_or("invalid_argument")?)
        {
            return Err("conflict".into());
        }
        for saved in self.state.receiver.rejected_actions.values() {
            if saved.request["params"]["action"]["id"] == action["id"] {
                return if receiver_intent(&saved.request)? == receiver_intent(request)? {
                    Ok(saved.accept.clone())
                } else {
                    Err("conflict".into())
                };
            }
        }
        if self.state.receiver.ledger_len() >= LEDGER_LIMIT {
            return Err("resource_exhausted".into());
        }
        let now = self.supervised_now()?;
        if action["deadline"].as_u64().ok_or("invalid_argument")? > now {
            return Err("unavailable".into());
        }
        let accept = json!({"id":action["id"],"correlation_id":action["correlation_id"],"accepted":false,
            "error":{"code":"deadline_exceeded","message":"Runtime deadline expired before admission","retryable":false}});
        let mut next = self.state.clone();
        next.receiver.rejected_actions.insert(
            id.into(),
            ReceiverRejectedAction {
                request: request.clone(),
                accept: accept.clone(),
            },
        );
        self.commit_receiver(next)?;
        Ok(accept)
    }

    pub(crate) fn receiver_receipt_for(
        &self,
        id: &str,
        actor: &str,
        ticket: Option<&str>,
    ) -> Option<(String, Value)> {
        if let Some(saved) = self.state.receiver.cancellations.get(id) {
            let scope = receiver_scope(&saved.request);
            return (scope.actor == actor && scope.ticket.as_deref() == ticket)
                .then(|| (scope.method, saved.receipt.clone()));
        }
        let saved = self.state.receiver.operations.get(id)?;
        if saved.scope.actor != actor || saved.scope.ticket.as_deref() != ticket {
            return None;
        }
        Some((saved.scope.method.clone(), saved.receipt.clone()))
    }

    /// One committed materialized snapshot: staged changes already live in the
    /// WAL, while the base remains journal-derived for exact next-step replay.
    pub(crate) fn receiver_snapshot(&self) -> &Runtime {
        self.state
            .staged_boundary
            .as_ref()
            .map(|s| &s.runtime)
            .unwrap_or(&self.state.runtime)
    }

    pub(crate) fn receiver_read(&self, path: &str, offset: u64, limit: u64) -> Result<Value> {
        if limit == 0 || limit > 65536 {
            return Err("invalid_argument".into());
        }
        let runtime = self.receiver_snapshot();
        let raw = if let Some(step) = path.strip_prefix("@step/") {
            if step.is_empty()
                || !step.bytes().all(|c| c.is_ascii_digit())
                || (step.len() > 1 && step.starts_with('0'))
            {
                return Err("permission_denied".into());
            }
            let step: u64 = step.parse().map_err(|_| "invalid_argument")?;
            let record = self
                .store
                .records()
                .iter()
                .find(|r| r["step"].as_u64() == Some(step))
                .ok_or("not_found")?;
            serde_json::to_vec(record).map_err(|e| e.to_string())?
        } else {
            if !crate::valid_path(path)
                || !(path.starts_with("/data/") || path.starts_with("/logs/"))
            {
                return Err("permission_denied".into());
            }
            hex::decode(runtime.files.files().get(path).ok_or("not_found")?)
                .map_err(|e| e.to_string())?
        };
        let text = std::str::from_utf8(&raw).map_err(|_| "invalid_argument")?;
        let start = usize::try_from(offset).map_err(|_| "invalid_argument")?;
        if start > raw.len() || !text.is_char_boundary(start) {
            return Err("invalid_argument".into());
        }
        let mut end = start.saturating_add(limit as usize).min(raw.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start && start < raw.len() {
            return Err("invalid_argument".into());
        }
        Ok(
            json!({"step":Self::last_step(&self.state),"snapshot_hash":runtime.files.tree_hash()?,
            "text":&text[start..end],"next_offset":end,"eof":end==raw.len()}),
        )
    }

    /// Must be called under fresh exact router/actor/ticket authority. Admission
    /// and queued receipt are one frame, and a failed commit publishes neither.
    pub(crate) fn receiver_admit(&mut self, request: &Value) -> Result<Value> {
        validate("hub-request", request).map_err(|_| "invalid_argument")?;
        let method = request["method"].as_str().ok_or("invalid_argument")?;
        if !matches!(
            method,
            "debug.write" | "hub.inbox" | "debug.pause" | "debug.step"
        ) {
            return Err("invalid_argument".into());
        }
        if request["target"] != self.state.runtime.manifest["id"] {
            return Err("permission_denied".into());
        }
        let id = request["correlation_id"]
            .as_str()
            .ok_or("invalid_argument")?;
        let intent = receiver_intent(request)?;
        if let Some(saved) = self.state.receiver.operations.get(id) {
            return if saved.intent == intent {
                Ok(saved.receipt.clone())
            } else {
                Err("conflict".into())
            };
        }
        if self.managed_contains_lifecycle_operation(id) || self.receiver_contains(id) {
            return Err("conflict".into());
        }
        if self.state.receiver.ledger_len() >= LEDGER_LIMIT
            || self.state.receiver.pending.len() >= PENDING_LIMIT
        {
            return Err("resource_exhausted".into());
        }
        let used = self
            .state
            .receiver
            .pending
            .iter()
            .try_fold(0usize, |n, id| {
                Ok::<_, String>(n + cbor(&self.state.receiver.operations[id].request)?.len())
            })?;
        if used.saturating_add(cbor(request)?.len()) > PENDING_BYTES {
            return Err("resource_exhausted".into());
        }
        match method {
            "debug.pause" => (),
            "debug.step" => {
                if !self.managed_paused() {
                    return Err("conflict".into());
                }
            }
            "debug.write" => {
                let path = request["params"]["path"]
                    .as_str()
                    .ok_or("invalid_argument")?;
                if !crate::valid_path(path)
                    || !path.starts_with("/data/")
                    || crate::filesystem::reserved_collision(path)
                    || path.starts_with("/data/runtime/")
                {
                    return Err("permission_denied".into());
                }
                if request["params"]["text"]
                    .as_str()
                    .ok_or("invalid_argument")?
                    .len()
                    > 65536
                {
                    return Err("resource_exhausted".into());
                }
                if request["params"]["expected_hash"]
                    != self.receiver_snapshot().files.tree_hash()?
                {
                    return Err("conflict".into());
                }
            }
            "hub.inbox" => {
                let message = &request["params"]["message"];
                if message["sender"] != message["event"]["source"]
                    || message["event"]["kind"]
                        .as_str()
                        .is_none_or(|k| k.starts_with("runtime.") || k == "action.outcome")
                {
                    return Err("permission_denied".into());
                }
                if serde_json::to_vec(message)
                    .map_err(|e| e.to_string())?
                    .len()
                    > 65536
                {
                    return Err("resource_exhausted".into());
                }
                let message_id = message["id"].as_str().ok_or("invalid_argument")?;
                let hash = digest(&cbor(message)?);
                if self
                    .state
                    .receiver
                    .messages
                    .get(message_id)
                    .is_some_and(|old| old != &hash)
                {
                    return Err("conflict".into());
                }
            }
            _ => unreachable!(),
        }
        let mut next = self.state.clone();
        let receipt = mutation_receipt(id, "queued", None, None);
        if method == "hub.inbox" {
            let message = &request["params"]["message"];
            next.receiver.messages.insert(
                message["id"].as_str().unwrap().into(),
                digest(&cbor(message)?),
            );
        }
        next.receiver.operations.insert(
            id.into(),
            ReceiverOperation {
                response: None,
                intent,
                request: request.clone(),
                scope: receiver_scope(request),
                receipt: receipt.clone(),
            },
        );
        next.receiver.pending.push(id.into());
        let scope = receiver_scope(request);
        let hash = self.receiver_snapshot().files.tree_hash()?;
        next.receiver.audit.push(json!({"operation_id":id,"actor":scope.actor,"ticket":scope.ticket,
            "method":method,"target":request["target"],"step":Self::last_step(&next),"before_hash":hash,"after_hash":hash,"outcome":"queued"}));
        self.commit_receiver(next)?;
        Ok(receipt)
    }

    pub(crate) fn receiver_pending(&self) -> Option<Value> {
        #[cfg(feature = "native-engine")]
        if self.detached_execution {
            return None;
        }
        if self.state.runtime.unresolved_attempt.is_some() {
            return None;
        }
        self.state
            .receiver
            .pending
            .first()
            .map(|id| self.state.receiver.operations[id].request.clone())
    }

    /// Call under the ORIGINAL operation's fresh authority. The callback is
    /// sampled immediately before publication (including after candidate work).
    pub(crate) fn receiver_apply(
        &mut self,
        id: &str,
        authorized: &dyn Fn() -> Result<()>,
    ) -> Result<()> {
        #[cfg(feature = "native-engine")]
        if self.detached_execution {
            return Err("unavailable".into());
        }
        if self.state.receiver.pending.first().map(String::as_str) != Some(id) {
            return Err("conflict".into());
        }
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("unavailable".into());
        }
        let request = self.state.receiver.operations[id].request.clone();
        let before = self.receiver_snapshot().files.tree_hash()?;
        let mut next = self.state.clone();
        let outcome = authorized()
            .and_then(|()| Self::receiver_stage(&mut next, &request))
            .and_then(|()| authorized());
        let error = outcome.as_ref().err().map(|e| match e.as_str() {
            "permission_denied" => "permission_denied",
            "conflict" => "conflict",
            "resource_exhausted" => "resource_exhausted",
            _ => "unavailable",
        });
        if error.is_some() {
            next = self.state.clone();
        }
        let after = next
            .staged_boundary
            .as_ref()
            .map(|s| &s.runtime)
            .unwrap_or(&next.runtime)
            .files
            .tree_hash()?;
        next.receiver.pending.remove(0);
        let step = Self::last_step(&next);
        let saved = next.receiver.operations.get_mut(id).unwrap();
        saved.receipt = mutation_receipt(
            id,
            if error.is_some() { "failed" } else { "applied" },
            error.is_none().then_some(step),
            error,
        );
        if saved.scope.method == "debug.step" {
            saved.response = Some(json!({"step":step,"receipt":saved.receipt}));
        }
        next.receiver.audit.push(json!({"operation_id":id,"actor":saved.scope.actor,"ticket":saved.scope.ticket,
            "method":saved.scope.method,"target":request["target"],"step":step,"before_hash":before,"after_hash":after,"outcome":saved.receipt["state"]}));
        Self::receiver_retire_step(&mut next, id)?;
        self.commit_receiver(next)
    }

    pub(crate) fn receiver_cancel_request(&mut self, request: &Value) -> Result<Value> {
        validate("hub-request", request).map_err(|_| "invalid_argument")?;
        if request["method"] != "hub.cancel"
            || request["target"] != self.state.runtime.manifest["id"]
        {
            return Err("invalid_argument".into());
        }
        let id = request["correlation_id"]
            .as_str()
            .ok_or("invalid_argument")?;
        if let Some(saved) = self.state.receiver.cancellations.get(id) {
            return if receiver_intent(&saved.request)? == receiver_intent(request)? {
                Ok(saved.receipt.clone())
            } else {
                Err("conflict".into())
            };
        }
        if self.receiver_contains(id) || self.managed_contains_lifecycle_operation(id) {
            return Err("conflict".into());
        }
        if self.state.receiver.ledger_len() >= LEDGER_LIMIT {
            return Err("resource_exhausted".into());
        }
        let scope = receiver_scope(request);
        let (mut next, receipt) = self.stage_receiver_cancel(
            request["params"]["operation_id"]
                .as_str()
                .ok_or("invalid_argument")?,
            &scope.actor,
            scope.ticket.as_deref(),
        )?;
        next.receiver.cancellations.insert(
            id.into(),
            ReceiverCancellation {
                request: request.clone(),
                receipt: receipt.clone(),
            },
        );
        self.commit_receiver(next)?;
        Ok(receipt)
    }

    #[cfg(test)]
    pub(crate) fn receiver_cancel(
        &mut self,
        id: &str,
        actor: &str,
        ticket: Option<&str>,
    ) -> Result<Value> {
        let (next, receipt) = self.stage_receiver_cancel(id, actor, ticket)?;
        self.commit_receiver(next)?;
        Ok(receipt)
    }

    fn stage_receiver_cancel(
        &mut self,
        id: &str,
        actor: &str,
        ticket: Option<&str>,
    ) -> Result<(ServiceState, Value)> {
        let saved = self
            .state
            .receiver
            .operations
            .get(id)
            .filter(|s| s.scope.actor == actor && s.scope.ticket.as_deref() == ticket)
            .ok_or("not_found")?;
        if saved.receipt["state"] != "queued"
            || (self.state.receiver.active_step.as_deref() == Some(id)
                && self.state.runtime.unresolved_attempt.is_some())
        {
            return Err("conflict".into());
        }
        let mut next = self.state.clone();
        let receipt = mutation_receipt(id, "cancelled", None, None);
        next.receiver.operations.get_mut(id).unwrap().receipt = receipt.clone();
        if saved.scope.method == "debug.step" {
            next.receiver.operations.get_mut(id).unwrap().response =
                Some(json!({"step":Self::last_step(&next),"receipt":receipt}));
        }
        next.receiver.pending.retain(|pending| pending != id);
        let hash = self.receiver_snapshot().files.tree_hash()?;
        next.receiver.audit.push(json!({"operation_id":id,"actor":actor,"ticket":ticket,"method":saved.scope.method,
            "target":saved.request["target"],"step":Self::last_step(&next),"before_hash":hash,"after_hash":hash,"outcome":"cancelled"}));
        Self::receiver_retire_step(&mut next, id)?;
        Ok((next, receipt))
    }

    fn receiver_stage(next: &mut ServiceState, request: &Value) -> Result<()> {
        if request["method"] == "debug.pause" {
            next.managed.as_mut().ok_or("permission_denied")?.paused = true;
            return Ok(());
        }
        if request["method"] == "debug.step" {
            return Err("unavailable".into());
        }
        if next.runtime.sealed {
            return Err("unavailable".into());
        }
        let mut staged = next
            .staged_boundary
            .take()
            .unwrap_or_else(|| StagedBoundary {
                changes: Vec::new(),
                runtime: next.runtime.clone(),
            });
        let runtime = &mut staged.runtime;
        let scope = receiver_scope(request);
        let method = scope.method.as_str();
        let (path, after, wake) = if method == "debug.write" {
            if request["params"]["expected_hash"] != runtime.files.tree_hash()? {
                return Err("conflict".into());
            }
            (
                request["params"]["path"].as_str().unwrap(),
                hex::encode(request["params"]["text"].as_str().unwrap().as_bytes()),
                None,
            )
        } else {
            let message = &request["params"]["message"];
            let messages =
                crate::boundary::inbox_with_profile(&runtime.files, runtime.contract_profile)?;
            if let Some(existing) = messages.iter().find(|m| m["id"] == message["id"]) {
                let mut delivery = existing.clone();
                delivery
                    .as_object_mut()
                    .ok_or("invalid inbox")?
                    .remove("index");
                if delivery != *message {
                    return Err("conflict".into());
                }
                next.staged_boundary = Some(staged);
                return Ok(());
            }
            let index = u64::try_from(messages.len()).map_err(|_| "resource_exhausted")?;
            let mut line = message.clone();
            line["index"] = json!(index);
            let mut raw = hex::decode(
                runtime
                    .files
                    .files()
                    .get("/data/inbox.jsonl")
                    .map(String::as_str)
                    .unwrap_or(""),
            )
            .map_err(|e| e.to_string())?;
            raw.extend(serde_json::to_vec(&line).map_err(|e| e.to_string())?);
            raw.push(b'\n');
            (
                "/data/inbox.jsonl",
                hex::encode(raw),
                Some((
                    index,
                    message["id"].clone(),
                    message["event"]["correlation_id"].clone(),
                )),
            )
        };
        let change = json!({"actor":scope.actor,"ticket":scope.ticket,"target":request["target"],"audit_id":request["correlation_id"],
            "method":method,"path":path,"before":runtime.files.files().get(path),"after":after});
        runtime.apply_boundary_changes(std::slice::from_ref(&change))?;
        if let Some((index, id, correlation)) = wake {
            let seq = runtime.next_source_seq;
            runtime.next_source_seq = seq.checked_add(1).ok_or("resource_exhausted")?;
            next.router.enqueue(json!({"source":runtime.manifest["id"],"kind":"runtime.inbox","seq":seq,"logical_time":runtime.time,
                "payload":{"index":index,"id":id},"correlation_id":correlation})).map_err(|_| "resource_exhausted")?;
            next.runtime.next_source_seq = runtime.next_source_seq;
        }
        staged.changes.push(change);
        next.staged_boundary = Some(staged);
        Ok(())
    }

    fn commit_receiver(&mut self, next: ServiceState) -> Result<()> {
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
}
