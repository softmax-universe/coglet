//! Emergency custody is an ordinary managed WAL transition, not a second store.
use super::*;
use crate::output_lease::{EmergencyCustody, EmergencyObligation, EmergencyStatus};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Armed,
    StopPending,
    Stopped,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EmergencyRecord {
    obligation: EmergencyObligation,
    phase: Phase,
    revoked_by: Option<String>,
}
impl EmergencyRecord {
    fn scope(&self) -> ReceiptScope {
        ReceiptScope {
            actor: self.obligation.actor.clone(),
            ticket: self.obligation.ticket.clone(),
            method: "hub.stop".into(),
        }
    }
    fn validate(&self, managed: &ManagedState) -> Result<()> {
        let o = &self.obligation;
        validate("hub-request", &o.request)?;
        let router = managed
            .identity
            .pointer("/policy_admission/router")
            .and_then(Value::as_str)
            .ok_or("emergency custody lacks original router")?;

        // The signed statement's target is checked by policy validation; require
        // this obligation to match that exact retained activation target too.
        let applied = managed
            .policy
            .as_ref()
            .ok_or("emergency custody lacks applied provenance")?;

        if o.version != 1
            || o.target != applied.target()
            || o.request["method"] != "hub.stop"
            || o.request["target"] != o.target
            || o.request["correlation_id"] != o.operation_id
            || o.request["context"]["actor"] != o.actor
            || o.request["context"]["ticket"].as_str() != o.ticket.as_deref()
            || o.intent
                != digest(&crate::cbor(&json!({"peer":router,
                "method":o.request["method"],"target":o.request["target"],
                "params":o.request["params"],"context":o.request["context"]}))?)
            || managed.install_receipt["operation_id"] == o.operation_id
        {
            return Err("invalid retained emergency obligation".into());
        }
        if let Some(saved) = managed.operations.get(&o.operation_id) {
            if saved.intent != o.intent || saved.scope.as_ref() != Some(&self.scope()) {
                return Err("emergency operation context collision".into());
            }
            if self.phase == Phase::Stopped && saved.receipt["state"] != "applied" {
                return Err("emergency stopped without applied receipt".into());
            }
        } else if self.phase == Phase::Stopped {
            return Err("emergency stopped without retained receipt".into());
        }
        if self.phase == Phase::StopPending && self.revoked_by.as_ref().is_none_or(String::is_empty)
        {
            return Err("emergency revoke lacks daemon identity".into());
        }
        Ok(())
    }
}
impl ManagedState {
    pub(super) fn validate_emergency(&self) -> Result<()> {
        if let Some(record) = &self.emergency {
            record.validate(self)?;
        }
        Ok(())
    }
    pub(super) fn emergency_id_conflicts(
        &self,
        id: &str,
        intent: &str,
        scope: &ReceiptScope,
    ) -> bool {
        self.emergency.as_ref().is_some_and(|r| {
            r.obligation.operation_id == id
                && (r.obligation.intent != intent || r.scope() != *scope)
        })
    }
    pub(super) fn emergency_reserved(&self, id: &str) -> bool {
        self.emergency.as_ref().is_some_and(|r| {
            r.phase != Phase::Stopped
                && r.obligation.operation_id != id
                && !self.operations.contains_key(&r.obligation.operation_id)
        })
    }
}
#[cfg(feature = "device")]
impl EmergencyCustody for RuntimeService {
    fn emergency_status(&self, target: &str) -> Result<EmergencyStatus> {
        if !self.store.is_healthy() || self.state.runtime.manifest["id"] != target {
            return Err("unavailable: emergency WAL custody unavailable".into());
        }
        let managed = self.state.managed.as_ref().ok_or("not installed")?;
        let Some(r) = &managed.emergency else {
            return Ok(EmergencyStatus::None);
        };
        r.validate(managed)?;
        Ok(match r.phase {
            Phase::Armed => EmergencyStatus::Armed(r.obligation.clone()),
            Phase::StopPending => EmergencyStatus::StopPending(r.obligation.clone()),
            Phase::Stopped => EmergencyStatus::Stopped(r.obligation.clone()),
        })
    }
    fn emergency_retain(&mut self, obligation: &EmergencyObligation) -> Result<()> {
        let managed = self.state.managed.as_ref().ok_or("not installed")?;
        if !self.store.is_healthy()
            || self.state.runtime.unresolved_attempt.is_some()
            || !self.managed_running()
            || self.state.runtime.manifest["id"] != obligation.target
        {
            return Err("unavailable: cannot arm emergency custody".into());
        }
        if let Some(r) = &managed.emergency {
            if r.phase == Phase::Armed && r.obligation == *obligation {
                return Ok(());
            }
            if r.phase != Phase::Stopped || r.obligation.operation_id == obligation.operation_id {
                return Err("conflict: unresolved or reused emergency obligation".into());
            }
        }
        if managed.operations.contains_key(&obligation.operation_id)
            || managed.operations.len() >= OPERATION_LEDGER - 1
        {
            return Err("conflict: emergency operation ID or receipt capacity unavailable".into());
        }
        let record = EmergencyRecord {
            obligation: obligation.clone(),
            phase: Phase::Armed,
            revoked_by: None,
        };
        record.validate(managed)?;
        let mut next = self.state.clone();
        next.managed.as_mut().unwrap().emergency = Some(record);
        self.commit_managed(next)
    }
    fn emergency_record_revoked(&mut self, target: &str, daemon_instance: &str) -> Result<()> {
        match self.emergency_status(target)? {
            EmergencyStatus::None | EmergencyStatus::Stopped(_) => return Ok(()),
            _ => {}
        }
        if daemon_instance.is_empty() {
            return Err("invalid daemon identity".into());
        }
        let mut next = self.state.clone();
        let r = next.managed.as_mut().unwrap().emergency.as_mut().unwrap();
        r.phase = Phase::StopPending;
        r.revoked_by = Some(daemon_instance.into());
        self.commit_managed(next)
    }
    fn emergency_record_stopped(&mut self, target: &str, receipt: &Value) -> Result<()> {
        let status = self.emergency_status(target)?;
        let obligation = match status {
            EmergencyStatus::Armed(o)
            | EmergencyStatus::StopPending(o)
            | EmergencyStatus::Stopped(o) => o,
            EmergencyStatus::None => {
                return Err("conflict: no retained emergency obligation".into());
            }
        };
        let managed = self.state.managed.as_ref().unwrap();
        let saved = managed
            .operations
            .get(&obligation.operation_id)
            .ok_or("unknown emergency stop receipt")?;
        if self.state.runtime.unresolved_attempt.is_some()
            || managed.running
            || managed.cancellation_pending
            || receipt["state"] != "applied"
            || saved.receipt != *receipt
            || saved.intent != obligation.intent
        {
            return Err("unavailable: emergency stop not authoritatively applied".into());
        }
        let mut next = self.state.clone();
        next.managed
            .as_mut()
            .unwrap()
            .emergency
            .as_mut()
            .unwrap()
            .phase = Phase::Stopped;
        self.commit_managed(next)
    }
}
