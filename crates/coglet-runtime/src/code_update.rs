//! Serialized installed-code transitions. A code revision pins a NEW ABI trace;
//! it is not a debug mutation and never rewrites an existing trace's code pin.
use super::*;

/// Verified by the router-link owner before entering this private coordinator
/// seam. Retained in the WAL to derive the successor from its predecessor rather
/// than trusting a free-standing recovered executable snapshot.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct CodeRevision {
    pub(crate) setup: Value,
    pub(crate) signed_ref: Value,
    pub(crate) operation_id: String,
    pub(crate) intent: String,
    pub(crate) scope: ReceiptScope,
}

impl CodeRevision {
    pub(crate) fn successor(&self, old: &Runtime, trace_id: &Value) -> Result<Runtime> {
        validate("correlation-id", &json!(self.operation_id))?;
        validate("hub-oid", &self.signed_ref["statement"]["new"])?;
        if self.intent.len() != 64
            || !self
                .intent
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || self.scope.method != "hub.git.push"
            || self.signed_ref["statement"]["generation"]
                .as_u64()
                .is_none()
        {
            return Err("invalid code revision custody".into());
        }
        let code = self.setup["initial_files"]
            .as_object()
            .ok_or("missing replacement code")?;
        if code.is_empty() || code.keys().any(|p| !p.starts_with("/code/")) {
            return Err("replacement code cannot contain private files".into());
        }
        if self.setup["manifest"]["id"] != old.manifest["id"] {
            return Err("replacement manifest identity differs".into());
        }
        // The internal cut may follow a committed step or an empty stopped
        // trace. No synthetic ABI boundary or guest attempt is manufactured.
        let mut sealed = old.clone();
        sealed.sealed = true;
        let mut next = sealed.start_segment(trace_id)?;
        let mut files = next.files.files().clone();
        files.retain(|p, _| !p.starts_with("/code/"));
        for (path, bytes) in code {
            files.insert(
                path.clone(),
                bytes.as_str().ok_or("invalid code bytes")?.to_owned(),
            );
        }
        next.initial_setup["initial_files"] = json!(files);
        next.initial_setup["manifest"] = self.setup["manifest"].clone();
        // from_fixture enforces manifest, mount, executable/provenance and
        // carried pending-state invariants without executing a guest.
        let checked = Runtime::from_fixture(&next.initial_setup)?;
        next.files = checked.files;
        next.manifest = checked.manifest;
        Ok(next)
    }
}

#[cfg(any(feature = "device", test))]
impl RuntimeService {
    /// Reconciliation never invokes a guest. Confirmed WAL receipts win over
    /// later authority changes; only new effects need the supplied fresh gate.
    pub(crate) fn complete_code_revision(
        &mut self,
        revision: &CodeRevision,
        gate: &dyn Fn() -> Result<u64>,
    ) -> Result<Value> {
        match self.apply_code_revision(revision, gate) {
            Ok(receipt) => Ok(receipt),
            Err(error)
                if self.store.is_healthy()
                    && matches!(
                        error.split(": ").next(),
                        Some("permission_denied" | "conflict" | "invalid_argument")
                    ) =>
            {
                let managed = self.state.managed.as_ref().ok_or("not installed")?;
                if managed.operations.contains_key(&revision.operation_id)
                    || managed.install_receipt["operation_id"] == revision.operation_id
                {
                    return Err(error);
                }
                if managed.operations.len() >= OPERATION_LEDGER - 1 {
                    return Err("resource_exhausted: lifecycle receipt ledger full".into());
                }
                let receipt = json!({"operation_id":revision.operation_id,"state":"failed","step":null,
                    "error":{"code":error.split(": ").next().unwrap(),"message":"Router code application refused","retryable":false}});
                validate("hub-mutation-receipt", &receipt)?;
                let mut next = self.state.clone();
                next.managed.as_mut().unwrap().operations.insert(
                    revision.operation_id.clone(),
                    OperationReceipt {
                        intent: revision.intent.clone(),
                        receipt: receipt.clone(),
                        scope: Some(revision.scope.clone()),
                    },
                );
                self.commit_managed(next)?;
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    }
    /// Caller holds SQL write custody and fresh exact push/delegation authority.
    /// Exclusive &mut access waits for the current guest to finish. A failed WAL
    /// commit exposes neither the new engine nor an applied receipt.
    pub(crate) fn apply_code_revision(
        &mut self,
        revision: &CodeRevision,
        gate: &dyn Fn() -> Result<u64>,
    ) -> Result<Value> {
        let managed = self.state.managed.as_ref().ok_or("not installed")?;
        if managed.install_receipt["operation_id"] == revision.operation_id {
            return Err("conflict: code operation collides with install".into());
        }
        if let Some(saved) = managed.operations.get(&revision.operation_id) {
            return if saved.intent == revision.intent
                && saved.scope.as_ref() == Some(&revision.scope)
            {
                Ok(saved.receipt.clone())
            } else {
                Err("conflict: code operation intent differs".into())
            };
        }
        // Preserve the stop-reserved slot, including stopped-only updates.
        if managed.operations.len() >= OPERATION_LEDGER - 1 {
            return Err("resource_exhausted: lifecycle receipt ledger full".into());
        }
        if !self.store.is_healthy() || self.state.runtime.unresolved_attempt.is_some() {
            return Err("unavailable: unresolved runtime custody prevents code change".into());
        }
        #[cfg(feature = "native-engine")]
        if self.detached_execution {
            return Err("unavailable: detached runtime custody prevents code change".into());
        }
        gate()?;
        let previous = self
            .state
            .completed_segments
            .iter()
            .rev()
            .find_map(|s| s.code_revision.as_ref());
        let previous_generation = previous
            .map(|r| &r.signed_ref["statement"]["generation"])
            .or_else(|| managed.identity.pointer("/initial/statement/generation"))
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if revision.signed_ref["statement"]["generation"]
            .as_u64()
            .is_none_or(|g| g <= previous_generation)
        {
            return Err("conflict: code generation did not advance".into());
        }
        let mut next = self.state.clone();
        // Hub ordering: materialized debug/inbox work first, then the code cut.
        // Seal real pending changes on the OLD trace; no fake /code boundary.
        let old_trace = next.runtime.trace_id.clone().unwrap_or_else(|| {
            json!(
                digest(format!("install:{}", managed.install_receipt["operation_id"]).as_bytes())
                    [..32]
                    .to_owned()
            )
        });
        let boundary = if let Some(staged) = next.staged_boundary.take() {
            // A superseded heard read may leave an empty materialized stage.
            // It carries coordinator reservations, not an ABI mutation: do not
            // manufacture an illegal empty trailing boundary for it.
            let boundary = if staged.changes.is_empty() {
                None
            } else {
                Some(next.runtime.seal_boundary(
                    &old_trace,
                    staged.runtime.time.max(next.runtime.time),
                    &staged.changes,
                )?)
            };
            next.runtime.next_source_seq = next
                .runtime
                .next_source_seq
                .max(staged.runtime.next_source_seq);
            next.runtime.time = next.runtime.time.max(staged.runtime.time);
            boundary
        } else {
            None
        };
        let trace = json!(
            digest(format!("code:{}:{}", revision.operation_id, revision.intent).as_bytes())[..32]
                .to_owned()
        );
        if next.completed_segments.iter().any(|s| s.trace_id == trace) || old_trace == trace {
            return Err("conflict: code trace identity already used".into());
        }
        let successor = revision
            .successor(&next.runtime, &trace)
            .map_err(|e| format!("invalid_argument: {e}"))?;
        let step = self.managed_last_step();
        let receipt = json!({"operation_id":revision.operation_id,"state":"applied","step":step,"error":null});
        validate("hub-mutation-receipt", &receipt)?;
        next.completed_segments.push(trace::CompletedSegment {
            hash_upgrade: false,
            initial_setup: next.runtime.initial_setup.clone(),
            trace_id: old_trace,
            end: self.store.records().len() + usize::from(boundary.is_some()),
            successor_trace: Some(trace),
            code_revision: Some(revision.clone()),
        });
        next.runtime = successor;
        next.fenced_staged_prefix = None;
        let managed = next.managed.as_mut().unwrap();
        managed.live_commit = revision.signed_ref["statement"]["new"].clone();
        managed.operations.insert(
            revision.operation_id.clone(),
            OperationReceipt {
                intent: revision.intent.clone(),
                receipt: receipt.clone(),
                scope: Some(revision.scope.clone()),
            },
        );
        managed.policy_revision(
            revision,
            &next.runtime,
            &receipt,
            self.store.records().len() + usize::from(boundary.is_some()),
        )?;
        #[cfg(feature = "native-engine")]
        let replacement = self
            .native
            .as_ref()
            .map(|(config, _)| {
                let mut config = config.clone();
                config.trace_id = next
                    .runtime
                    .trace_id
                    .as_ref()
                    .unwrap()
                    .as_str()
                    .unwrap()
                    .to_owned();
                let engine = native_engine::NativeEngine::new(
                    hex::decode(&next.runtime.files.files()["/code/bin/step.wasm"])
                        .map_err(|e| e.to_string())?,
                    &next.runtime.manifest["budgets"],
                    config.runtime_bytes,
                    config.stack_bytes,
                )
                .map_err(|e| format!("invalid_argument: replacement native module: {e}"))?;
                Ok::<_, String>((config, engine))
            })
            .transpose()?;
        gate()?;
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            boundary.as_ref(),
        )?;
        self.state = next;
        #[cfg(feature = "native-engine")]
        {
            self.native = replacement;
        }
        Ok(receipt)
    }
}

#[cfg(test)]
#[path = "code_update_tests.rs"]
mod tests;
