//! Replaceable Hermes Kanban adapter. Identifiers stay public-API shaped;
//! this module never opens Hermes internal tables.
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskSpec {
    pub idempotency_key: String,
    pub kind: String,
    pub payload_cbor_hex: String,
    pub correlation_id: String,
    pub task_id: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTask {
    pub remote_id: String,
    pub idempotency_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lease {
    pub remote_id: String,
    pub owner: String,
    pub until: u64,
    pub attempt: u64,
    /// Issued lease identity. Results, heartbeats and cancellation must present it.
    pub token: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Succeeded { note: String },
    Failed { note: String },
}

/// Worker report fenced by the attempt and lease the dispatcher issued.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerReport {
    pub attempt: u64,
    pub lease: String,
    pub outcome: Outcome,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    pub event_id: String,
    pub task_id: String,
    pub kind: String,
    pub note: String,
    pub attempt: u64,
    pub lease: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    Uncertain,
    Failed(String),
    Conflict(String),
}

pub trait HermesAdapter {
    fn supports_cancel(&self) -> bool {
        false
    }
    /// Unsupported retry leaves the exact attempt held for evidence polling,
    /// never a local requeue that would invent a remote lifecycle operation.
    fn supports_retry(&self) -> bool {
        true
    }
    fn create(&mut self, spec: &TaskSpec) -> Result<RemoteTask, AdapterError>;
    fn claim(
        &mut self,
        remote_id: &str,
        owner: &str,
        until: u64,
        attempt: u64,
    ) -> Result<Lease, AdapterError>;
    /// Recover an uncertain grant from authoritative evidence before repeating it.
    /// None means proven absent (or an adapter with idempotent claim replay).
    /// An expired recovered lease retains only result correlation, not authority.
    fn recover_claim(
        &mut self,
        _remote_id: &str,
        _owner: &str,
        _until: u64,
        _attempt: u64,
    ) -> Result<Option<Lease>, AdapterError> {
        Ok(None)
    }
    /// Revalidate/renew a durably saved token after a crash, without obtaining a new claim.
    fn restore_lease(
        &mut self,
        remote_id: &str,
        owner: &str,
        until: u64,
        attempt: u64,
        token: &str,
    ) -> Result<Lease, AdapterError>;
    fn heartbeat(
        &mut self,
        remote_id: &str,
        until: u64,
        attempt: u64,
        lease: &str,
    ) -> Result<u64, AdapterError>;
    fn retry(&mut self, remote_id: &str) -> Result<(), AdapterError>;
    fn cancel(&mut self, remote_id: &str, attempt: u64, lease: &str) -> Result<(), AdapterError>;
    fn progress(
        &mut self,
        remote_id: &str,
        note: &str,
        attempt: u64,
        lease: &str,
    ) -> Result<(), AdapterError>;
    fn poll(
        &mut self,
        remote_id: &str,
        attempt: u64,
        lease: &str,
    ) -> Result<Option<WorkerReport>, AdapterError>;
}

#[derive(Clone, Debug, Default)]
pub struct MockAdapter {
    pub created: BTreeMap<String, RemoteTask>,
    pub claims: Vec<Lease>,
    pub heartbeats: Vec<(String, u64)>,
    pub retries: Vec<String>,
    pub cancels: Vec<String>,
    pub notes: Vec<(String, String)>,
    pub results: BTreeMap<String, WorkerReport>,
    pub crash_keys: Vec<String>,
    pub fail_keys: Vec<String>,
}

impl MockAdapter {
    pub fn complete(&mut self, remote_id: &str, report: WorkerReport) {
        self.results.insert(remote_id.into(), report);
    }
}

impl HermesAdapter for MockAdapter {
    fn supports_cancel(&self) -> bool {
        true
    }
    fn create(&mut self, spec: &TaskSpec) -> Result<RemoteTask, AdapterError> {
        if self.crash_keys.iter().any(|k| k == &spec.idempotency_key) {
            self.crash_keys.retain(|k| k != &spec.idempotency_key);
            return Err(AdapterError::Uncertain);
        }
        if self.fail_keys.iter().any(|k| k == &spec.idempotency_key) {
            self.fail_keys.retain(|k| k != &spec.idempotency_key);
            return Err(AdapterError::Failed("scripted".into()));
        }
        if let Some(existing) = self.created.get(&spec.idempotency_key) {
            return Ok(existing.clone());
        }
        let remote = RemoteTask {
            remote_id: format!("k{}", spec.idempotency_key),
            idempotency_key: spec.idempotency_key.clone(),
        };
        self.created
            .insert(spec.idempotency_key.clone(), remote.clone());
        Ok(remote)
    }

    fn claim(
        &mut self,
        remote_id: &str,
        owner: &str,
        until: u64,
        attempt: u64,
    ) -> Result<Lease, AdapterError> {
        if !self.created.values().any(|t| t.remote_id == remote_id) {
            return Err(AdapterError::Conflict("unknown remote".into()));
        }
        let lease = Lease {
            remote_id: remote_id.into(),
            owner: owner.into(),
            until,
            attempt,
            token: format!("{owner}:{attempt}:{remote_id}"),
        };
        self.claims.push(lease.clone());
        Ok(lease)
    }

    fn restore_lease(
        &mut self,
        remote_id: &str,
        owner: &str,
        until: u64,
        attempt: u64,
        token: &str,
    ) -> Result<Lease, AdapterError> {
        Ok(Lease {
            remote_id: remote_id.into(),
            owner: owner.into(),
            until,
            attempt,
            token: token.into(),
        })
    }
    fn heartbeat(
        &mut self,
        remote_id: &str,
        until: u64,
        attempt: u64,
        lease: &str,
    ) -> Result<u64, AdapterError> {
        let known = self
            .claims
            .iter()
            .any(|c| c.remote_id == remote_id && c.attempt == attempt && c.token == lease);
        if !known {
            return Err(AdapterError::Conflict("stale lease".into()));
        }
        self.heartbeats.push((remote_id.into(), until));
        Ok(until)
    }

    fn retry(&mut self, remote_id: &str) -> Result<(), AdapterError> {
        self.retries.push(remote_id.into());
        Ok(())
    }

    fn cancel(&mut self, remote_id: &str, attempt: u64, lease: &str) -> Result<(), AdapterError> {
        let known = self
            .claims
            .iter()
            .any(|c| c.remote_id == remote_id && c.attempt == attempt && c.token == lease);
        if !known {
            return Err(AdapterError::Conflict("stale lease".into()));
        }
        self.cancels.push(remote_id.into());
        Ok(())
    }

    fn progress(
        &mut self,
        remote_id: &str,
        note: &str,
        attempt: u64,
        lease: &str,
    ) -> Result<(), AdapterError> {
        let known = self
            .claims
            .iter()
            .any(|c| c.remote_id == remote_id && c.attempt == attempt && c.token == lease);
        if !known {
            return Err(AdapterError::Conflict("stale lease".into()));
        }
        self.notes.push((remote_id.into(), note.into()));
        Ok(())
    }

    fn poll(
        &mut self,
        remote_id: &str,
        _attempt: u64,
        _lease: &str,
    ) -> Result<Option<WorkerReport>, AdapterError> {
        Ok(self.results.get(remote_id).cloned())
    }
}
