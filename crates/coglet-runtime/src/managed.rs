//! Durable router-managed lifecycle. The caller verifies signed authority and
//! local grants; this module owns the serialized runtime/WAL boundary only.
use super::*;

#[path = "code_update.rs"]
mod code_update;
#[path = "policy_emergency.rs"]
mod policy_emergency;
pub(crate) use code_update::CodeRevision;
use policy_emergency::EmergencyRecord;

#[path = "policy_state.rs"]
mod policy_state;

/// Retained start/stop receipts. Entries are never evicted.
const OPERATION_LEDGER: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedState {
    #[serde(default)]
    emergency: Option<EmergencyRecord>,
    #[serde(default)]
    policy: Option<policy_state::PolicyState>,
    identity: Value,
    install_receipt: Value,
    live_commit: Value,
    running: bool,
    #[serde(default)]
    pub(super) paused: bool,
    /// Persisted before SQL push admission: no next-step pinning until the
    /// authority-serialized recovery driver has drained admitted code work.
    #[serde(default)]
    code_pending: bool,
    #[serde(default)]
    recovery: Option<StartRecovery>,
    #[serde(default)]
    recovery_fault: Option<Value>,
    // Unknown attempts cannot be changed to fabricate a cancellation receipt.
    // Retain the stop obligation until authoritative reconciliation permits it.
    cancellation_pending: bool,
    stopped_at: u64,
    #[serde(default)]
    operations: BTreeMap<String, OperationReceipt>,
    /// Original actor/ticket/method of the install admission. Absent only for
    /// receipts written without scope; those are never readable via hub.receipt.
    #[serde(default)]
    install_scope: Option<ReceiptScope>,
}

/// Original authenticated scope of an admitted lifecycle operation, persisted
/// in the same frame as the receipt it protects.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptScope {
    pub(crate) actor: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ticket: Option<String>,
    pub(crate) method: String,
}

type Operation<'a> = (&'a str, &'a str, &'a ReceiptScope);

/// Authority retained atomically with the original running transition.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StartRecovery {
    pub(crate) operation_id: String,
    pub(crate) intent: String,
    pub(crate) scope: ReceiptScope,
    pub(crate) grant_generation: u64,
    pub(crate) delegation_generation: u64,
    pub(crate) delegation: Value,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationReceipt {
    intent: String,
    receipt: Value,
    #[serde(default)]
    scope: Option<ReceiptScope>,
}

impl ManagedState {
    pub(super) fn identity(&self) -> &Value {
        &self.identity
    }

    pub(super) fn validate_code_history(&self, segments: &[trace::CompletedSegment]) -> Result<()> {
        let mut commit = self
            .identity
            .get("live_commit")
            .or_else(|| self.identity.pointer("/initial/statement/new"))
            .or_else(|| self.identity.pointer("/update/statement/new"))
            .ok_or("missing install code pin")?;
        let mut generation = self
            .identity
            .pointer("/initial/statement/generation")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let mut ids = std::collections::BTreeSet::new();
        for revision in segments.iter().filter_map(|s| s.code_revision.as_ref()) {
            let next_generation = revision.signed_ref["statement"]["generation"]
                .as_u64()
                .ok_or("missing code generation")?;
            if next_generation <= generation || !ids.insert(&revision.operation_id) {
                return Err("recovered code generation/operation did not advance".into());
            }
            let saved = self
                .operations
                .get(&revision.operation_id)
                .ok_or("missing code operation receipt")?;
            if saved.intent != revision.intent
                || saved.scope.as_ref() != Some(&revision.scope)
                || saved.receipt["state"] != "applied"
            {
                return Err("recovered code receipt differs from transition".into());
            }
            generation = next_generation;
            commit = &revision.signed_ref["statement"]["new"];
        }
        if commit != &self.live_commit
            || self.operations.iter().any(|(id, op)| {
                op.scope
                    .as_ref()
                    .is_some_and(|s| s.method == "hub.git.push")
                    && op.receipt["state"] == "applied"
                    && !ids.contains(id)
            })
        {
            return Err("recovered live code lacks exact transition custody".into());
        }
        Ok(())
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.validate_emergency()?;
        if !self.identity.is_object() || (self.running && self.cancellation_pending) {
            return Err("invalid recovered managed lifecycle".into());
        }
        validate("hub-oid", &self.live_commit)?;
        validate("hub-mutation-receipt", &self.install_receipt)?;
        if self.install_receipt["state"] != "applied" || self.install_receipt["step"] != 0 {
            return Err("invalid recovered managed install receipt".into());
        }
        let pin = self
            .identity
            .get("live_commit")
            .or_else(|| self.identity.pointer("/initial/statement/new"))
            .or_else(|| self.identity.pointer("/update/statement/new"));
        if pin != Some(&self.live_commit)
            && !self.operations.values().any(|op| {
                op.scope
                    .as_ref()
                    .is_some_and(|s| s.method == "hub.git.push")
                    && op.receipt["state"] == "applied"
            })
        {
            return Err("recovered managed live commit differs from install identity".into());
        }
        for (id, saved) in &self.operations {
            validate("correlation-id", &json!(id))?;
            validate("hub-mutation-receipt", &saved.receipt)?;
            if saved.receipt["operation_id"] != *id
                || saved.intent.len() != 64
                || !saved
                    .intent
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("invalid recovered lifecycle operation receipt".into());
            }
        }
        if self.operations.len() > OPERATION_LEDGER {
            return Err("invalid recovered lifecycle receipt bound".into());
        }
        Ok(())
    }
}

impl RuntimeService {
    pub(crate) fn code_pending(&self) -> bool {
        self.state.managed.as_ref().is_some_and(|m| m.code_pending)
    }

    #[cfg(any(feature = "device", test))]
    pub(crate) fn fence_code(&mut self, pending: bool) -> Result<()> {
        if self.code_pending() == pending {
            return Ok(());
        }
        let mut next = self.state.clone();
        next.managed.as_mut().ok_or("not installed")?.code_pending = pending;
        self.commit_managed(next)
    }
    #[cfg(feature = "device")]
    pub(crate) fn fence_new_code(&mut self, id: &str) -> Result<()> {
        if self
            .state
            .managed
            .as_ref()
            .is_some_and(|m| m.operations.contains_key(id))
        {
            return Ok(());
        }
        self.fence_code(true)
    }

    /// Immutable verified install metadata, including signed delegation/ref and
    /// caller configuration. Correlation IDs belong to the receipt, not identity.
    pub fn managed_identity(&self) -> Option<&Value> {
        self.state.managed.as_ref().map(|m| &m.identity)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_install_receipt(&self) -> Option<&Value> {
        self.state.managed.as_ref().map(|m| &m.install_receipt)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_contains_lifecycle_operation(&self, id: &str) -> bool {
        self.state.managed.as_ref().is_some_and(|m| {
            m.install_receipt["operation_id"] == id
                || m.operations.contains_key(id)
                // A retained emergency stop ID is reserved before its receipt exists.
                || m.emergency.as_ref().is_some_and(|r| r.obligation.operation_id == id)
        })
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_push_conflicts(
        &self,
        id: &str,
        intent: &str,
        scope: &ReceiptScope,
    ) -> bool {
        self.receiver_contains(id)
            || self.state.managed.as_ref().is_some_and(|m| {
                m.install_receipt["operation_id"] == id
                    || m.emergency_id_conflicts(id, intent, scope)
                    || m.operations
                        .get(id)
                        .is_some_and(|r| r.intent != intent || r.scope.as_ref() != Some(scope))
            })
    }

    /// Original code/manifest/private-files configuration, not live private data.
    pub fn managed_setup(&self) -> &Value {
        self.state
            .completed_segments
            .first()
            .map(|s| &s.initial_setup)
            .unwrap_or(&self.state.runtime.initial_setup)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_code_ref(&self) -> Option<&Value> {
        self.state
            .completed_segments
            .iter()
            .rev()
            .find_map(|s| s.code_revision.as_ref().map(|r| &r.signed_ref))
            .or_else(|| self.managed_identity()?.get("initial"))
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_verify_code(&self, setup: &Value) -> Result<()> {
        let expected: BTreeMap<String, String> =
            serde_json::from_value(setup["initial_files"].clone())
                .map_err(|_| "permission_denied")?;
        let actual: BTreeMap<_, _> = self
            .state
            .runtime
            .files
            .files()
            .iter()
            .filter(|(p, _)| p.starts_with("/code/"))
            .map(|(p, b)| (p.clone(), b.clone()))
            .collect();
        if self.state.runtime.manifest != setup["manifest"] || actual != expected {
            return Err("permission_denied".into());
        }
        Ok(())
    }

    /// Pin an already verified identity and create a stopped service. Repeating
    /// the same identity returns the ORIGINAL receipt, even with a new operation ID.
    /// `live_commit` is explicit, or comes from `initial.statement.new` (the signed
    /// install ref) / `update.statement.new` (trusted provisioning metadata).
    pub fn managed_install(&mut self, identity: &Value, operation_id: &str) -> Result<Value> {
        self.managed_install_scoped(identity, operation_id, None)
    }

    pub(crate) fn managed_install_scoped(
        &mut self,
        identity: &Value,
        operation_id: &str,
        scope: Option<&ReceiptScope>,
    ) -> Result<Value> {
        validate("correlation-id", &json!(operation_id))?;
        #[cfg(feature = "device")]
        if self.receiver_contains(operation_id) {
            return Err("conflict: operation belongs to the back channel".into());
        }
        if let Some(saved) = &self.state.managed {
            // An identical installed identity is not permission to borrow a
            // different caller's receipt or collide with another method's ID.
            if scope.is_some()
                && (scope != saved.install_scope.as_ref()
                    || saved.operations.contains_key(operation_id))
            {
                return Err("conflict: managed install operation scope differs".into());
            }
            return if saved.identity == *identity {
                Ok(saved.install_receipt.clone())
            } else {
                Err("conflict: managed install identity differs".into())
            };
        }
        if !identity.is_object() {
            return Err("managed install identity must be an object".into());
        }
        let live_commit = identity
            .get("live_commit")
            .or_else(|| identity.pointer("/initial/statement/new"))
            .or_else(|| identity.pointer("/update/statement/new"))
            .ok_or("managed install identity requires a signed live commit pin")?
            .clone();
        validate("hub-oid", &live_commit)?;
        // An existing legacy execution cannot be retroactively called an install.
        if !self.store.records().is_empty()
            || self.state.runtime.unresolved_attempt.is_some()
            || self.state.staged_boundary.is_some()
            || !self.state.router.is_empty()
        {
            return Err("conflict: managed install requires a fresh service".into());
        }
        for (path, bytes) in self.state.runtime.files.files() {
            if path.starts_with("/data/") && (path != "/data/inbox.jsonl" || !bytes.is_empty()) {
                return Err("managed install requires an empty private data namespace".into());
            }
        }
        let receipt = json!({"operation_id":operation_id,"state":"applied","step":0,"error":null});
        validate("hub-mutation-receipt", &receipt)?;
        let mut next = self.state.clone();
        next.managed = Some(ManagedState {
            emergency: None,
            policy: None,
            identity: identity.clone(),
            install_receipt: receipt.clone(),
            live_commit,
            running: false,
            paused: false,
            code_pending: false,
            recovery: None,
            recovery_fault: None,
            cancellation_pending: false,
            stopped_at: 0,
            operations: BTreeMap::new(),
            install_scope: scope.cloned(),
        });
        next.managed
            .as_mut()
            .unwrap()
            .policy_install(&next.runtime)?;
        self.commit_managed(next)?;
        Ok(receipt)
    }

    /// The caller must check verified code, current grants and kill switch first.
    /// Native scheduling, when configured with enable_native, resumes without
    /// resetting its durable step or seed identity.
    pub fn managed_start(&mut self) -> Result<()> {
        self.managed_start_inner(None, None)
    }

    fn managed_start_inner(
        &mut self,
        operation: Option<Operation<'_>>,
        recovery: Option<&StartRecovery>,
    ) -> Result<()> {
        if self.state.managed.is_none() {
            return Err("managed service is not installed".into());
        }
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err(
                "unresolved engine attempt; authoritative outcome recovery required".into(),
            );
        }
        // The last ledger slot is reserved for a stop, so the fence stays
        // reachable and recorded when the ledger fills. No start of any kind
        // is admitted past that point: entries are never evicted, and a start
        // without a ledger entry could be reversed by a replayed stop.
        if self.state.managed.as_ref().unwrap().operations.len() >= OPERATION_LEDGER - 1 {
            return Err("resource_exhausted: lifecycle receipt ledger full".into());
        }
        let mut next = self.state.clone();
        if next.managed.as_ref().unwrap().cancellation_pending {
            let now = next
                .managed
                .as_ref()
                .unwrap()
                .stopped_at
                .max(next.runtime.time);
            Self::cancel_managed_undelivered(&mut next, now)?;
        }
        next.managed.as_mut().unwrap().running = true;
        next.managed.as_mut().unwrap().paused = false;
        next.managed.as_mut().unwrap().recovery = recovery.cloned();
        next.managed.as_mut().unwrap().recovery_fault = None;
        Self::record_operation(&mut next, operation, None, self.managed_last_step())?;
        self.commit_managed(next)
    }

    /// Fence new execution immediately, retaining a queued stop receipt while
    /// a detached guest still owns its attempt. Retained envelopes may escape to
    /// an adapter, so never invent outcomes for them or erase their receipts.
    pub fn managed_stop(&mut self, now: u64) -> Result<()> {
        self.managed_stop_inner(now, None)
    }

    fn managed_stop_inner(&mut self, now: u64, operation: Option<Operation<'_>>) -> Result<()> {
        if self.state.managed.is_none() {
            return Err("managed service is not installed".into());
        }
        let mut next = self.state.clone();
        let managed = next.managed.as_mut().unwrap();
        // Prepared bytes may not have reached the adapter yet. Never retransmit
        // them after STOP, even on a later START. Original receipts remain valid;
        // there is no safe fabricated outcome for an already exposed effect.
        for dispatch in next.dispatches.values_mut() {
            dispatch.stopped = true;
        }
        managed.running = false;
        managed.paused = false;
        managed.cancellation_pending = true;
        managed.stopped_at = now.max(next.runtime.time).max(managed.stopped_at);
        let now = managed.stopped_at;
        // Even an unknown engine outcome must be durably stopped, but its exact
        // inputs and queue reservations must remain untouched for reconciliation.
        if next.runtime.unresolved_attempt.is_none() {
            let stopped = next.clone();
            if let Err(error) =
                Self::cancel_managed_undelivered(&mut next, now.max(self.state.runtime.time))
            {
                // Queue pressure must never leave execution enabled. Preserve a
                // durable stop obligation and report that cancellation is pending.
                let mut stopped = stopped;
                Self::record_operation(
                    &mut stopped,
                    operation,
                    Some(&error),
                    self.managed_last_step(),
                )?;
                self.commit_managed(stopped)?;
                return Err(error);
            }
        }
        Self::record_operation(&mut next, operation, None, self.managed_last_step())?;
        if next.runtime.unresolved_attempt.is_some()
            && let Some((id, _, _)) = operation
        {
            let receipt = &mut next
                .managed
                .as_mut()
                .unwrap()
                .operations
                .get_mut(id)
                .unwrap()
                .receipt;
            receipt["state"] = json!("queued");
            receipt["step"] = Value::Null;
            validate("hub-mutation-receipt", receipt)?;
        }
        self.commit_managed(next)
    }

    /// Complete stops only in the same transition that reconciles the original
    /// guest journal. Unknown attempts retain queued receipts across reopen.
    pub(super) fn finish_pending_stops(next: &mut ServiceState, step: u64) -> Result<()> {
        let Some(managed) = &next.managed else {
            return Ok(());
        };
        if !managed.cancellation_pending || next.runtime.unresolved_attempt.is_some() {
            return Ok(());
        }
        let now = managed.stopped_at.max(next.runtime.time);
        // Cancellation failure retains the durable stopped obligation, matching
        // the synchronous stop path, without partially publishing cancellation.
        let mut candidate = next.clone();
        let error = Self::cancel_managed_undelivered(&mut candidate, now).err();
        if error.is_none() {
            *next = candidate;
        }
        for saved in next.managed.as_mut().unwrap().operations.values_mut() {
            if saved.scope.as_ref().is_some_and(|s| s.method == "hub.stop")
                && saved.receipt["state"] == "queued"
            {
                saved.receipt["state"] = json!(if error.is_some() { "failed" } else { "applied" });
                saved.receipt["step"] = if error.is_some() {
                    Value::Null
                } else {
                    json!(step)
                };
                saved.receipt["error"] = error.as_ref().map(|_| json!({"code":"unavailable","message":"Stop cancellation remains pending","retryable":false})).unwrap_or(Value::Null);
                validate("hub-mutation-receipt", &saved.receipt)?;
            }
        }
        Ok(())
    }

    /// Authenticated caller supplies a digest of peer/method/target/params/context,
    /// excluding timeout. Always reauthorize before this lookup.
    #[cfg(feature = "device")]
    pub(crate) fn managed_operation_receipt(
        &self,
        id: &str,
        intent: &str,
    ) -> Result<Option<Value>> {
        validate("correlation-id", &json!(id))?;
        if self.receiver_contains(id) {
            return Err("conflict: operation ID already used by receiver".into());
        }
        let managed = self.state.managed.as_ref().ok_or("not installed")?;
        if managed.install_receipt["operation_id"] == id {
            return Err("conflict: operation ID already used for install".into());
        }
        if let Some(saved) = managed.operations.get(id) {
            return if saved.intent == intent {
                Ok(Some(saved.receipt.clone()))
            } else {
                Err("conflict: operation intent differs".into())
            };
        }
        if managed.operations.len() >= OPERATION_LEDGER {
            return Err("resource_exhausted: lifecycle receipt ledger full".into());
        }
        if managed.emergency_reserved(id) && managed.operations.len() >= OPERATION_LEDGER - 1 {
            return Err("resource_exhausted: emergency stop receipt slot reserved".into());
        }
        Ok(None)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_operation(
        &mut self,
        id: &str,
        intent: &str,
        start: bool,
        now: u64,
        scope: &ReceiptScope,
    ) -> Result<Value> {
        if self
            .state
            .managed
            .as_ref()
            .is_some_and(|m| m.emergency_id_conflicts(id, intent, scope))
        {
            return Err("conflict: emergency operation scope differs".into());
        }
        if let Some(receipt) = self.managed_operation_receipt(id, intent)? {
            return Ok(receipt);
        }
        let outcome = if start {
            self.managed_start_inner(Some((id, intent, scope)), None)
        } else {
            self.managed_stop_inner(now, Some((id, intent, scope)))
        };
        // A stop cancellation failure still commits the stopped fence and its
        // failed receipt in one frame; its retry must not stop a later start.
        if let Some(receipt) = self.managed_operation_receipt(id, intent)? {
            return Ok(receipt);
        }
        outcome?;
        Err("unavailable: lifecycle receipt missing".into())
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_start_authorized(&mut self, recovery: &StartRecovery) -> Result<Value> {
        if let Some(receipt) =
            self.managed_operation_receipt(&recovery.operation_id, &recovery.intent)?
        {
            return Ok(receipt);
        }
        self.managed_start_inner(
            Some((&recovery.operation_id, &recovery.intent, &recovery.scope)),
            Some(recovery),
        )?;
        self.managed_operation_receipt(&recovery.operation_id, &recovery.intent)?
            .ok_or_else(|| "unavailable: lifecycle receipt missing".into())
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_recovery(&self) -> Result<StartRecovery> {
        let managed = self.state.managed.as_ref().ok_or("not installed")?;
        let recovery = managed
            .recovery
            .as_ref()
            .ok_or("missing start recovery context")?;
        let saved = managed
            .operations
            .get(&recovery.operation_id)
            .ok_or("missing original start receipt")?;
        if saved.intent != recovery.intent
            || saved.scope.as_ref() != Some(&recovery.scope)
            || recovery.scope.method != "hub.start"
            || saved.receipt["state"] != "applied"
            || self.state.runtime.unresolved_attempt.is_some()
            || self.state.staged_boundary.is_some()
            || managed.cancellation_pending
            || managed.recovery_fault.is_some()
        {
            return Err("invalid or fenced start recovery context".into());
        }
        Ok(recovery.clone())
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_recovery_fault(&mut self) -> Result<()> {
        let mut next = self.state.clone();
        let managed = next.managed.as_mut().ok_or("not installed")?;
        managed.running = false;
        managed.recovery_fault = Some(
            json!({"code":"unavailable","message":"Restart admission refused; fresh authorized start required","retryable":false}),
        );
        self.commit_managed(next)?;
        #[cfg(feature = "native-engine")]
        {
            self.native = None;
        }
        Ok(())
    }

    /// Receipt of an operation admitted by exactly this actor/ticket, with the
    /// method it was admitted under. Unknown IDs and other scopes are `None`.
    #[cfg(feature = "device")]
    pub(crate) fn managed_receipt_for(
        &self,
        id: &str,
        actor: &str,
        ticket: Option<&str>,
    ) -> Option<(String, Value)> {
        if self.receiver_contains(id) {
            return self.receiver_receipt_for(id, actor, ticket);
        }
        let managed = self.state.managed.as_ref()?;
        let same = |s: &ReceiptScope| s.actor == actor && s.ticket.as_deref() == ticket;
        if managed.install_receipt["operation_id"] == id {
            let scope = managed.install_scope.as_ref().filter(|s| same(s))?;
            return Some((scope.method.clone(), managed.install_receipt.clone()));
        }
        let saved = managed.operations.get(id)?;
        let scope = saved.scope.as_ref().filter(|s| same(s))?;
        Some((scope.method.clone(), saved.receipt.clone()))
    }

    pub(super) fn last_step(state: &ServiceState) -> u64 {
        state
            .runtime
            .records
            .last()
            .and_then(|r| r["step"].as_u64())
            .unwrap_or(0)
    }

    fn managed_last_step(&self) -> u64 {
        // Router status/receipt steps span code revisions; ABI record steps
        // remain trace-local and restart at zero for each separately pinned code.
        self.store
            .records()
            .iter()
            .filter(|r| r.get("step").is_some())
            .count()
            .saturating_sub(1) as u64
    }

    fn record_operation(
        next: &mut ServiceState,
        operation: Option<Operation<'_>>,
        error: Option<&str>,
        step: u64,
    ) -> Result<()> {
        let Some((id, intent, scope)) = operation else {
            return Ok(());
        };

        // A failed receipt carries no step (`hub-receipt-failed`). With a step
        // it would not validate, and the `?` below would return before the
        // stopped fence and this receipt were committed.
        let receipt = json!({"operation_id":id,"state":if error.is_some() {"failed"} else {"applied"},
            "step":if error.is_some() { Value::Null } else { json!(step) },"error":error.map(|_| json!({"code":"unavailable","message":"Stop cancellation remains pending","retryable":false}))});
        validate("hub-mutation-receipt", &receipt)?;
        next.managed
            .as_mut()
            .ok_or("not installed")?
            .operations
            .insert(
                id.into(),
                OperationReceipt {
                    intent: intent.into(),
                    receipt,
                    scope: Some(scope.clone()),
                },
            );
        Ok(())
    }

    /// Frozen hub-status projection: `step`, `pending`, `error` are wire names
    /// for last committed step, pending mutations and last shared error.
    pub fn managed_status(&self) -> Value {
        let last = self
            .store
            .records()
            .iter()
            .rev()
            .find(|r| r.get("step").is_some());
        let fault = self
            .state
            .managed
            .as_ref()
            .and_then(|m| m.recovery_fault.as_ref());
        let error = if let Some(fault) = fault {
            fault.clone()
        } else if self.state.runtime.unresolved_attempt.is_some() {
            json!({"code":"unavailable","message":"Unresolved engine attempt requires authoritative reconciliation","retryable":false})
        } else {
            last.and_then(|r| r.pointer("/failure/error"))
                .filter(|e| !e.is_null())
                .cloned()
                .unwrap_or(Value::Null)
        };
        json!({"state":if fault.is_some() {"faulted"} else if self.managed_paused() {"paused"} else if self.managed_running() {"running"} else {"stopped"},
            "live_commit":self.state.managed.as_ref().map(|m| &m.live_commit),
            "step":self.managed_last_step(),
            // Event batches and adapter receipts are not router mutations.
            "pending":self.receiver_pending_count(),"error":error})
    }

    pub(super) fn managed_paused(&self) -> bool {
        self.state.managed.as_ref().is_some_and(|m| m.paused)
    }

    pub(super) fn managed_running(&self) -> bool {
        self.state
            .managed
            .as_ref()
            .is_none_or(|m| m.running && !m.cancellation_pending && m.recovery_fault.is_none())
    }
    #[cfg(feature = "native-engine")]
    pub(super) fn receiver_pending_for_execution(&self) -> bool {
        self.receiver_pending_count() != 0
    }

    fn receiver_pending_count(&self) -> usize {
        #[cfg(feature = "device")]
        {
            self.receiver_pending_len()
        }
        #[cfg(not(feature = "device"))]
        {
            0
        }
    }

    pub(super) fn require_managed_running(&self) -> Result<()> {
        if self.managed_running() {
            Ok(())
        } else {
            Err("managed runtime is stopped".into())
        }
    }
    fn commit_managed(&mut self, next: ServiceState) -> Result<()> {
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
    fn cancel_managed_undelivered(next: &mut ServiceState, now: u64) -> Result<()> {
        let runtime = next
            .staged_boundary
            .as_ref()
            .map(|s| &s.runtime)
            .unwrap_or(&next.runtime);
        let pending: Vec<Value> = next
            .dispatch_order
            .iter()
            .filter(|id| !next.dispatches.contains_key(*id))
            .filter_map(|id| runtime.pending_actions.get(id).cloned())
            .collect();
        for action in pending {
            let payload = json!({"id":action["id"],"correlation_id":action["correlation_id"],"accepted":false,
                "error":{"code":"cancelled","message":"Runtime stopped before dispatch","retryable":false}});
            let event = json!({"source":action["coglet"],"kind":"runtime.action_rejected",
                "seq":next.runtime.next_source_seq,"logical_time":now,"payload":payload,
                "correlation_id":action["correlation_id"]});
            Self::stage_terminal(next, event, now)?;
        }
        next.managed.as_mut().unwrap().cancellation_pending = false;
        Ok(())
    }
}

#[cfg(all(test, feature = "device"))]
#[path = "managed_restart_tests.rs"]
mod restart_tests;

#[cfg(all(test, feature = "device"))]
mod tests {
    use super::*;

    fn scope(start: bool) -> ReceiptScope {
        ReceiptScope {
            actor: "ledger-test".into(),
            ticket: None,
            method: if start { "hub.start" } else { "hub.stop" }.into(),
        }
    }

    fn setup() -> Value {
        fixture("no-effects")
    }

    pub(super) fn fixture(name: &str) -> Value {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["steps"][0]["send"]
            .clone();
        setup["initial_files"]
            .as_object_mut()
            .unwrap()
            .retain(|p, _| !p.starts_with("/data/"));
        setup
    }

    fn fill_ledger_to(service: &mut RuntimeService, len: usize) {
        let operations = &mut service.state.managed.as_mut().unwrap().operations;
        for n in 0.. {
            if operations.len() >= len {
                break;
            }
            let id = format!("{n:032x}");
            let receipt = json!({"operation_id":id,"state":"applied","step":0,"error":null});
            operations.insert(
                id,
                OperationReceipt {
                    intent: "b".repeat(64),
                    receipt,
                    scope: None,
                },
            );
        }
        assert_eq!(operations.len(), len);
    }

    #[test]
    fn last_operation_ledger_slot_is_reserved_for_a_recorded_stop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.wal");
        let setup = setup();
        let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
        let identity = json!({"initial":{"statement":{"new":"a".repeat(40)}}});
        service.managed_install(&identity, &"1".repeat(32)).unwrap();

        // The last start that fits leaves exactly the reserved slot.
        fill_ledger_to(&mut service, OPERATION_LEDGER - 2);
        let start_id = "2".repeat(32);
        let start = service
            .managed_operation(&start_id, &"a".repeat(64), true, 0, &scope(true))
            .unwrap();
        assert_eq!(start["state"], "applied");
        assert!(service.managed_running());
        let len = |s: &RuntimeService| s.state.managed.as_ref().unwrap().operations.len();
        assert_eq!(len(&service), OPERATION_LEDGER - 1);

        // The stop takes the reserved slot and is recorded like any other.
        let stop_id = "4".repeat(32);
        let stopped = service
            .managed_operation(&stop_id, &"d".repeat(64), false, 7, &scope(false))
            .unwrap();
        assert_eq!(
            stopped,
            json!({"operation_id":stop_id,"state":"applied","step":0,"error":null})
        );
        assert!(!service.managed_running());
        assert_eq!(len(&service), OPERATION_LEDGER);

        // No start of any kind is admitted any more, so nothing can run again
        // for a replayed stop to reverse. Nothing is evicted.
        let refused = service
            .managed_operation(&"3".repeat(32), &"c".repeat(64), true, 0, &scope(true))
            .unwrap_err();
        assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        let refused = service.managed_start().unwrap_err();
        assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        let refused = service
            .managed_operation(&"6".repeat(32), &"d".repeat(64), false, 8, &scope(false))
            .unwrap_err();
        assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        assert!(!service.managed_running());
        assert_eq!(len(&service), OPERATION_LEDGER);

        // Recorded IDs keep original-receipt and conflict semantics when full,
        // across a real WAL reopen.
        drop(service);
        let mut service = RuntimeService::open(&path, &setup, 8).unwrap();
        assert_eq!(service.managed_status()["state"], "stopped");
        assert_eq!(
            service
                .managed_operation(&stop_id, &"d".repeat(64), false, 9, &scope(false))
                .unwrap(),
            stopped
        );
        assert_eq!(
            service
                .managed_operation(&start_id, &"a".repeat(64), true, 0, &scope(true))
                .unwrap(),
            start
        );
        for (id, intent) in [(&stop_id, "e"), (&start_id, "e"), (&"1".repeat(32), "e")] {
            let conflict = service
                .managed_operation(id, &intent.repeat(64), false, 0, &scope(false))
                .unwrap_err();
            assert!(conflict.starts_with("conflict:"), "{conflict}");
        }
        assert!(!service.managed_running());
    }

    #[test]
    fn start_is_refused_once_only_the_reserved_stop_slot_is_left() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.wal");
        let mut service = RuntimeService::open(&path, &setup(), 8).unwrap();
        let identity = json!({"initial":{"statement":{"new":"a".repeat(40)}}});
        service.managed_install(&identity, &"1".repeat(32)).unwrap();
        fill_ledger_to(&mut service, OPERATION_LEDGER - 1);
        for refused in [
            service
                .managed_operation(&"3".repeat(32), &"c".repeat(64), true, 0, &scope(true))
                .unwrap_err(),
            service.managed_start().unwrap_err(),
        ] {
            assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        }
        assert!(!service.managed_running());
        assert_eq!(
            service.state.managed.as_ref().unwrap().operations.len(),
            OPERATION_LEDGER - 1
        );
    }

    #[test]
    fn failed_stop_is_fenced_recorded_and_replayed_as_the_original_receipt() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.wal");
        let setup = fixture("action");
        // A one-entry queue makes the stop's cancellation fail under pressure.
        let mut service = RuntimeService::open(&path, &setup, 1).unwrap();
        let identity = json!({"initial":{"statement":{"new":"a".repeat(40)}}});
        service.managed_install(&identity, &"1".repeat(32)).unwrap();
        service
            .managed_operation(&"2".repeat(32), &"a".repeat(64), true, 0, &scope(true))
            .unwrap();
        let event = setup["inputs"]["events"][0].clone();
        service.enqueue(event.clone()).unwrap();
        service
            .run_next(
                &setup["inputs"],
                &mut MockEngine::new(setup["guest_script"].clone()),
            )
            .unwrap()
            .unwrap();
        let mut later = event;
        later["seq"] = json!(later["seq"].as_u64().unwrap() + 1);
        service.enqueue(later).unwrap();

        let stop_id = "4".repeat(32);
        let failed = service
            .managed_operation(&stop_id, &"d".repeat(64), false, 100, &scope(false))
            .unwrap();
        validate("hub-mutation-receipt", &failed).unwrap();
        assert_eq!(failed["operation_id"], stop_id);
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["step"], Value::Null);
        assert_eq!(failed["error"]["code"], "unavailable");
        assert!(!service.managed_running());

        // The fence and the failed receipt are in the WAL: after a reopen the
        // retry is a replay of the original receipt, and changed intent on
        // the same ID conflicts.
        drop(service);
        let mut service = RuntimeService::open(&path, &setup, 1).unwrap();
        assert_eq!(service.managed_status()["state"], "stopped");
        assert_eq!(
            service.managed_receipt_for(&stop_id, "ledger-test", None),
            Some(("hub.stop".into(), failed.clone()))
        );
        assert_eq!(service.managed_receipt_for(&stop_id, "other", None), None);
        assert_eq!(
            service.managed_receipt_for(&stop_id, "ledger-test", Some("foreign")),
            None
        );
        assert_eq!(
            service
                .managed_operation(&stop_id, &"d".repeat(64), false, 200, &scope(false))
                .unwrap(),
            failed
        );
        let conflict = service
            .managed_operation(&stop_id, &"e".repeat(64), false, 200, &scope(false))
            .unwrap_err();
        assert!(conflict.starts_with("conflict:"), "{conflict}");
        assert!(!service.managed_running());
    }
}
