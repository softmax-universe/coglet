//! Replaceable Hermes Kanban adapter. Identifiers stay Hermes-API shaped;
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
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Succeeded { note: String },
    Failed { note: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FollowUp {
    pub task_id: String,
    pub kind: String,
    pub note: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterError {
    Uncertain,
    Failed(String),
    Conflict(String),
}

pub trait HermesAdapter {
    fn create(&mut self, spec: &TaskSpec) -> Result<RemoteTask, AdapterError>;
    fn claim(&mut self, remote_id: &str, owner: &str, until: u64) -> Result<Lease, AdapterError>;
    fn heartbeat(&mut self, remote_id: &str, until: u64) -> Result<(), AdapterError>;
    fn retry(&mut self, remote_id: &str) -> Result<(), AdapterError>;
    fn cancel(&mut self, remote_id: &str) -> Result<(), AdapterError>;
    fn progress(&mut self, remote_id: &str, note: &str) -> Result<(), AdapterError>;
}

#[derive(Clone, Debug, Default)]
pub struct MockAdapter {
    pub created: BTreeMap<String, RemoteTask>,
    pub claims: Vec<Lease>,
    pub heartbeats: Vec<(String, u64)>,
    pub retries: Vec<String>,
    pub cancels: Vec<String>,
    pub notes: Vec<(String, String)>,
    pub results: BTreeMap<String, Outcome>,
    pub crash_keys: Vec<String>,
    pub fail_keys: Vec<String>,
}

impl MockAdapter {
    pub fn complete(&mut self, remote_id: &str, outcome: Outcome) {
        self.results.insert(remote_id.into(), outcome);
    }

    pub fn take_result(&mut self, remote_id: &str) -> Option<Outcome> {
        self.results.remove(remote_id)
    }
}

impl HermesAdapter for MockAdapter {
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
            remote_id: format!("kanban:{}", spec.idempotency_key),
            idempotency_key: spec.idempotency_key.clone(),
        };
        self.created
            .insert(spec.idempotency_key.clone(), remote.clone());
        Ok(remote)
    }

    fn claim(&mut self, remote_id: &str, owner: &str, until: u64) -> Result<Lease, AdapterError> {
        if !self.created.values().any(|t| t.remote_id == remote_id) {
            return Err(AdapterError::Conflict("unknown remote".into()));
        }
        let lease = Lease {
            remote_id: remote_id.into(),
            owner: owner.into(),
            until,
        };
        self.claims.push(lease.clone());
        Ok(lease)
    }

    fn heartbeat(&mut self, remote_id: &str, until: u64) -> Result<(), AdapterError> {
        self.heartbeats.push((remote_id.into(), until));
        Ok(())
    }

    fn retry(&mut self, remote_id: &str) -> Result<(), AdapterError> {
        self.retries.push(remote_id.into());
        Ok(())
    }

    fn cancel(&mut self, remote_id: &str) -> Result<(), AdapterError> {
        self.cancels.push(remote_id.into());
        Ok(())
    }

    fn progress(&mut self, remote_id: &str, note: &str) -> Result<(), AdapterError> {
        self.notes.push((remote_id.into(), note.into()));
        Ok(())
    }
}
