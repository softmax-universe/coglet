//! One back-channel step uses the ordinary fenced engine attempt. Neither
//! persisted running state nor production budgets are relaxed for single-step.
use super::*;

impl RuntimeService {
    pub(in crate::execution) fn receiver_retired_wakes(state: &ServiceState) -> &[Value] {
        &state.receiver.retired_wakes
    }

    pub(super) fn receiver_retire_step(next: &mut ServiceState, id: &str) -> Result<()> {
        if next.receiver.active_step.as_deref() != Some(id) {
            return Ok(());
        }
        if next.runtime.unresolved_attempt.is_some() {
            return Err("unavailable".into());
        }
        next.receiver.active_step = None;
        let removed: Vec<_> = next
            .router
            .entries()
            .filter(|(_, e)| e["kind"] == "runtime.debug_step" && e["correlation_id"] == id)
            .map(|(n, e)| (*n, e.clone()))
            .collect();
        next.router
            .retain_ordinals(|n| !removed.iter().any(|(old, _)| *old == n));
        next.receiver
            .retired_wakes
            .extend(removed.into_iter().map(|(_, e)| e));
        Ok(())
    }

    pub(crate) fn receiver_step_result(&self, id: &str) -> Result<Value> {
        self.state
            .receiver
            .operations
            .get(id)
            .and_then(|op| op.response.clone())
            .ok_or("unavailable".into())
    }
    pub(in crate::execution) fn receiver_valid_debug_wake(
        state: &ServiceState,
        event: &Value,
    ) -> bool {
        let Some(id) = state.receiver.active_step.as_deref() else {
            return false;
        };
        state.receiver.pending.first().map(String::as_str) == Some(id)
            && state.receiver.operations.get(id).is_some_and(|op| {
                op.scope.method == "debug.step" && op.receipt["state"] == "queued"
            })
            && event["source"] == state.runtime.manifest["id"]
            && event["payload"] == json!({})
            && event["correlation_id"] == id
    }

    pub(in crate::execution) fn receiver_step_active(&self) -> bool {
        self.debug_executing && self.state.receiver.active_step.is_some()
    }

    pub(in crate::execution) fn receiver_finish_step(
        next: &mut ServiceState,
        result: &Value,
        before: &Snapshot,
    ) -> Result<()> {
        let Some(id) = next.receiver.active_step.take() else {
            return Ok(());
        };
        if next.receiver.pending.first() != Some(&id) {
            return Err("step queue custody changed".into());
        }
        let step = result["journal"]["step"]
            .as_u64()
            .ok_or("missing step result")?;
        let saved = next
            .receiver
            .operations
            .get_mut(&id)
            .ok_or("missing step operation")?;
        let failed = result["journal"]["status"] != "committed";
        saved.receipt = mutation_receipt(
            &id,
            if failed { "failed" } else { "applied" },
            (!failed).then_some(step),
            failed.then_some("unavailable"),
        );
        saved.response = Some(json!({"step":step,"receipt":saved.receipt}));
        next.receiver.audit.push(json!({"operation_id":id,"actor":saved.scope.actor,"ticket":saved.scope.ticket,
            "method":"debug.step","target":saved.request["target"],"step":step,"before_hash":before.frozen_hub0_hash()?,
            "after_hash":next.runtime.files.frozen_hub0_hash()?,"outcome":saved.receipt["state"]}));
        next.receiver.pending.remove(0);
        Ok(())
    }

    #[cfg(feature = "native-engine")]
    pub(crate) fn receiver_prepare_native_step(
        &mut self,
        id: &str,
        grants: &Value,
        authorized: &dyn Fn() -> Result<()>,
    ) -> Result<
        Option<(
            super::super::detached::DetachedRun,
            native_engine::NativeSchedule,
            native_engine::NativeEngine,
        )>,
    > {
        authorized()?;
        if self.state.receiver.pending.first().map(String::as_str) != Some(id) {
            return Err("conflict".into());
        }
        if !self.managed_paused() {
            self.receiver_apply(id, &|| Err("conflict".into()))?;
            return Ok(None);
        }
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("unavailable".into());
        }
        let Some((config, _)) = self.native.as_ref() else {
            return Err("unavailable".into());
        };
        let config = config.clone();
        let result = (|| {
            let now = self.supervised_now()?;
            let mut next = self.state.clone();
            if next
                .receiver
                .active_step
                .as_deref()
                .is_some_and(|old| old != id)
            {
                return Err("conflict".into());
            }
            next.receiver.active_step = Some(id.into());
            next.runtime.time = now;
            if let Some(staged) = &mut next.staged_boundary {
                staged.runtime.time = now;
            }
            // Only synthesize when there is no eligible input; this wake is
            // retained before guest entry and correlated to this original ID.
            if next.router.is_empty() {
                let seq = next.runtime.next_source_seq;
                next.runtime.next_source_seq = seq.checked_add(1).ok_or("resource_exhausted")?;
                if let Some(staged) = &mut next.staged_boundary {
                    staged.runtime.next_source_seq = next.runtime.next_source_seq;
                }
                next.router.enqueue(
                    json!({"source":next.runtime.manifest["id"],"kind":"runtime.debug_step",
                    "seq":seq,"logical_time":now,"payload":{},"correlation_id":id}),
                )?;
            }
            authorized()?;
            self.commit_receiver(next)?;
            let seed_bytes = cbor(&json!([config.seed, self.state.runtime.step]))?;
            let seed =
                u64::from_str_radix(&digest(&seed_bytes)[..16], 16).map_err(|e| e.to_string())?;
            let context = json!({"trace_id":config.trace_id,"logical_time":now,"seed":seed,
                "budgets":self.state.runtime.manifest["budgets"],"grants":grants,"boundary":[]});
            authorized()?;
            self.debug_executing = true;
            let run = self.prepare_detached(&context)?.ok_or("unavailable")?;
            let (config, engine) = self.native.take().ok_or("unavailable")?;
            Ok(Some((run, config, engine)))
        })();
        // An entered/unreported attempt is never retried or converted into a
        // fabricated terminal result. Reconciliation owns that original attempt.
        if result.is_err() && self.state.runtime.unresolved_attempt.is_none() {
            self.debug_executing = false;
            self.receiver_apply(id, &|| Err("unavailable".into()))?;
            return Ok(None);
        }
        result
    }

    #[cfg(feature = "native-engine")]
    pub(crate) fn receiver_restore_native(
        &mut self,
        id: &str,
        config: native_engine::NativeSchedule,
        engine: native_engine::NativeEngine,
    ) -> Result<()> {
        self.debug_executing = false;
        self.native = Some((config, engine));
        self.detached_execution = false;
        if self.state.runtime.unresolved_attempt.is_none()
            && self.state.receiver.pending.first().map(String::as_str) == Some(id)
        {
            self.receiver_apply(id, &|| Err("unavailable".into()))?;
        }
        Ok(())
    }
}
