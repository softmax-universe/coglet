use crate::{ContractProfile, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::VecDeque;

/// Arrival ordinals belong to the host, never the source's clock. Persist this
/// value together with consumption when committing a runtime transaction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Router {
    capacity: usize,
    next_ordinal: u64,
    queue: VecDeque<(u64, Value)>,
    #[serde(default)]
    contract_profile: ContractProfile,
}

impl Router {
    pub fn new(capacity: usize) -> Result<Self> {
        Self::with_contract_profile(capacity, ContractProfile::Surface01)
    }

    pub fn with_contract_profile(
        capacity: usize,
        contract_profile: ContractProfile,
    ) -> Result<Self> {
        if capacity == 0 {
            return Err("queue capacity must be positive".into());
        }
        Ok(Self {
            capacity,
            next_ordinal: 0,
            queue: VecDeque::new(),
            contract_profile,
        })
    }

    pub fn enqueue(&mut self, event: Value) -> Result<u64> {
        self.contract_profile.validate("event", &event)?;
        if self.queue.len() == self.capacity {
            return Err("resource_exhausted: router full".into());
        }
        let ordinal = self.next_ordinal;
        let next = ordinal.checked_add(1).ok_or("arrival ordinal exhausted")?;
        self.queue.push_back((ordinal, event));
        self.next_ordinal = next;
        Ok(ordinal)
    }

    /// Pin a prefix by copying. It is consumed only after a durable attempt.
    pub fn batch(&self, max_events: usize) -> Result<Vec<Value>> {
        if max_events == 0 {
            return Err("max_events must be positive".into());
        }
        Ok(self
            .queue
            .iter()
            .take(max_events)
            .map(|(_, e)| e.clone())
            .collect())
    }

    pub fn consume(&mut self, count: usize) -> Result<()> {
        if count > self.queue.len() {
            return Err("cannot consume absent events".into());
        }
        self.queue.drain(..count);
        Ok(())
    }

    pub(crate) fn validate_recovered(&self) -> Result<()> {
        if self.capacity == 0 || self.queue.len() > self.capacity {
            return Err("invalid recovered router capacity".into());
        }
        let mut previous = None;
        for (ordinal, event) in &self.queue {
            if *ordinal >= self.next_ordinal || previous.is_some_and(|old| old >= *ordinal) {
                return Err("invalid recovered router arrival order".into());
            }
            self.contract_profile.validate("event", event)?;
            previous = Some(*ordinal);
        }
        Ok(())
    }

    pub(crate) fn events(&self) -> impl Iterator<Item = &Value> {
        self.queue.iter().map(|(_, event)| event)
    }
    pub fn contract_profile(&self) -> ContractProfile {
        self.contract_profile
    }
    pub(crate) fn entries(&self) -> impl Iterator<Item = &(u64, Value)> {
        self.queue.iter()
    }
    pub fn is_full(&self) -> bool {
        self.queue.len() >= self.capacity
    }
    pub(crate) fn retain_ordinals(&mut self, mut keep: impl FnMut(u64) -> bool) {
        self.queue.retain(|(ordinal, _)| keep(*ordinal));
    }
    pub fn len(&self) -> usize {
        self.queue.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}
