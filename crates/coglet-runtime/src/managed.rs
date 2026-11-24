//! Durable router-managed lifecycle. The caller verifies signed authority and
//! local grants; this module owns the serialized runtime/WAL boundary only.
use super::*;

/// Retained start/stop receipts. Entries are never evicted.
const OPERATION_LEDGER: usize = 4096;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ManagedState {
    identity: Value,
    install_receipt: Value,
    live_commit: Value,
    running: bool,
    // Unknown attempts cannot be changed to fabricate a cancellation receipt.
    // Retain the stop obligation until authoritative reconciliation permits it.
    cancellation_pending: bool,
    stopped_at: u64,
    #[serde(default)]
    operations: BTreeMap<String, OperationReceipt>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OperationReceipt {
    intent: String,
    receipt: Value,
}

impl ManagedState {
    pub(super) fn identity(&self) -> &Value {
        &self.identity
    }

    pub(super) fn validate(&self) -> Result<()> {
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
        if pin != Some(&self.live_commit) {
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
    /// Immutable verified install metadata, including signed delegation/ref and
    /// caller configuration. Correlation IDs belong to the receipt, not identity.
    pub fn managed_identity(&self) -> Option<&Value> {
        self.state.managed.as_ref().map(|m| &m.identity)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_install_receipt(&self) -> Option<&Value> {
        self.state.managed.as_ref().map(|m| &m.install_receipt)
    }

    /// Original code/manifest/private-files configuration, not live private data.
    pub fn managed_setup(&self) -> &Value {
        &self.state.runtime.initial_setup
    }

    /// Pin an already verified identity and create a stopped service. Repeating
    /// the same identity returns the ORIGINAL receipt, even with a new operation ID.
    /// `live_commit` is explicit, or comes from `initial.statement.new` (the signed
    /// install ref) / `update.statement.new` (trusted provisioning metadata).
    pub fn managed_install(&mut self, identity: &Value, operation_id: &str) -> Result<Value> {
        validate("correlation-id", &json!(operation_id))?;
        if let Some(saved) = &self.state.managed {
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
            identity: identity.clone(),
            install_receipt: receipt.clone(),
            live_commit,
            running: false,
            cancellation_pending: false,
            stopped_at: 0,
            operations: BTreeMap::new(),
        });
        self.commit_managed(next)?;
        Ok(receipt)
    }

    /// The caller must check verified code, current grants and kill switch first.
    /// Native scheduling, when configured with enable_native, resumes without
    /// resetting its durable step or seed identity.
    pub fn managed_start(&mut self) -> Result<()> {
        self.managed_start_inner(None)
    }

    fn managed_start_inner(&mut self, operation: Option<(&str, &str)>) -> Result<()> {
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
        Self::record_operation(&mut next, operation, None)?;
        self.commit_managed(next)
    }

    /// Exclusive &mut access is the safe serialized boundary: no guest can be
    /// running concurrently. Retained translated envelopes may have escaped to
    /// an adapter, so never invent outcomes for them or erase their receipts.
    pub fn managed_stop(&mut self, now: u64) -> Result<()> {
        self.managed_stop_inner(now, None)
    }

    fn managed_stop_inner(&mut self, now: u64, operation: Option<(&str, &str)>) -> Result<()> {
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
                Self::record_operation(&mut stopped, operation, Some(&error))?;
                self.commit_managed(stopped)?;
                return Err(error);
            }
        }
        Self::record_operation(&mut next, operation, None)?;
        self.commit_managed(next)
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
        Ok(None)
    }

    #[cfg(feature = "device")]
    pub(crate) fn managed_operation(
        &mut self,
        id: &str,
        intent: &str,
        start: bool,
        now: u64,
    ) -> Result<Value> {
        if let Some(receipt) = self.managed_operation_receipt(id, intent)? {
            return Ok(receipt);
        }
        let outcome = if start {
            self.managed_start_inner(Some((id, intent)))
        } else {
            self.managed_stop_inner(now, Some((id, intent)))
        };
        // A stop cancellation failure still commits the stopped fence and its
        // failed receipt in one frame; its retry must not stop a later start.
        if let Some(receipt) = self.managed_operation_receipt(id, intent)? {
            return Ok(receipt);
        }
        outcome?;
        Err("unavailable: lifecycle receipt missing".into())
    }

    fn last_step(state: &ServiceState) -> u64 {
        state
            .runtime
            .records
            .last()
            .and_then(|r| r["step"].as_u64())
            .unwrap_or(0)
    }

    fn record_operation(
        next: &mut ServiceState,
        operation: Option<(&str, &str)>,
        error: Option<&str>,
    ) -> Result<()> {
        let Some((id, intent)) = operation else {
            return Ok(());
        };
        let step = Self::last_step(next);
        let receipt = json!({"operation_id":id,"state":if error.is_some() {"failed"} else {"applied"},
            "step":step,"error":error.map(|_| json!({"code":"unavailable","message":"Stop cancellation remains pending","retryable":false}))});
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
        let error = if self.state.runtime.unresolved_attempt.is_some() {
            json!({"code":"unavailable","message":"Unresolved engine attempt requires authoritative reconciliation","retryable":false})
        } else {
            last.and_then(|r| r.pointer("/failure/error"))
                .filter(|e| !e.is_null())
                .cloned()
                .unwrap_or(Value::Null)
        };
        json!({"state":if self.managed_running() {"running"} else {"stopped"},
            "live_commit":self.state.managed.as_ref().map(|m| &m.live_commit),
            "step":last.and_then(|r| r["step"].as_u64()).unwrap_or(0),
            // This seam has no queued router mutation API. Runtime receipts and
            // event batches are not pending router mutations.
            "pending":0,"error":error})
    }

    pub(super) fn managed_running(&self) -> bool {
        self.state
            .managed
            .as_ref()
            .is_none_or(|m| m.running && !m.cancellation_pending)
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
mod tests {
    use super::*;

    fn setup() -> Value {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
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
            .managed_operation(&start_id, &"a".repeat(64), true, 0)
            .unwrap();
        assert_eq!(start["state"], "applied");
        assert!(service.managed_running());
        let len = |s: &RuntimeService| s.state.managed.as_ref().unwrap().operations.len();
        assert_eq!(len(&service), OPERATION_LEDGER - 1);

        // The stop takes the reserved slot and is recorded like any other.
        let stop_id = "4".repeat(32);
        let stopped = service
            .managed_operation(&stop_id, &"d".repeat(64), false, 7)
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
            .managed_operation(&"3".repeat(32), &"c".repeat(64), true, 0)
            .unwrap_err();
        assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        let refused = service.managed_start().unwrap_err();
        assert!(refused.starts_with("resource_exhausted:"), "{refused}");
        let refused = service
            .managed_operation(&"6".repeat(32), &"d".repeat(64), false, 8)
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
                .managed_operation(&stop_id, &"d".repeat(64), false, 9)
                .unwrap(),
            stopped
        );
        assert_eq!(
            service
                .managed_operation(&start_id, &"a".repeat(64), true, 0)
                .unwrap(),
            start
        );
        for (id, intent) in [(&stop_id, "e"), (&start_id, "e"), (&"1".repeat(32), "e")] {
            let conflict = service
                .managed_operation(id, &intent.repeat(64), false, 0)
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
                .managed_operation(&"3".repeat(32), &"c".repeat(64), true, 0)
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
}
