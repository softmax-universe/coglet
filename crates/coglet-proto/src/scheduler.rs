//! Non-waiting per-peer admission. Permits return all capacity on cancellation.
//! Numeric rates are local policy, not wire protocol constants.
use iroh::EndpointId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};

pub const MAX_QUEUED_BYTES: usize = 4 * 1024 * 1024;
pub const CONTROL_BYTES: usize = 65536;
#[derive(Debug, thiserror::Error)]
#[error("peer resource exhausted")]
pub struct Exhausted;

struct Rate {
    remaining: usize,
    start: Instant,
}
impl Default for Rate {
    fn default() -> Self {
        Self {
            remaining: 0,
            start: Instant::now() - Duration::from_secs(60),
        }
    }
}
impl Rate {
    fn take(&mut self, limit: usize) -> bool {
        let now = Instant::now();
        if now.duration_since(self.start) >= Duration::from_secs(60) {
            self.start = now;
            self.remaining = limit;
        }
        if self.remaining == 0 {
            return false;
        }
        self.remaining -= 1;
        true
    }
}
#[derive(Default)]
struct State {
    streams: usize,
    unclassified: usize,
    bulk: usize,
    git: usize,
    bytes: usize,
    bulk_bytes: usize,
    health: Rate,
    enroll: Rate,
    control: Rate,
}
#[derive(Clone, Default)]
pub struct PeerScheduler(Arc<Mutex<State>>);
type Registry = HashMap<(EndpointId, EndpointId), Arc<Mutex<State>>>;
impl PeerScheduler {
    /// Share across every connection and independently constructed Session for
    /// this local identity and authenticated remote peer. Dead peers are pruned.
    pub(crate) fn shared(local: EndpointId, peer: EndpointId) -> Result<Self, Exhausted> {
        static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
        let mut registry = REGISTRY.get_or_init(Default::default).lock().unwrap();
        registry.retain(|_, value| {
            if Arc::strong_count(value) > 1 {
                return true;
            }
            let state = value.lock().unwrap();
            [&state.health, &state.enroll, &state.control]
                .iter()
                .any(|rate| rate.start.elapsed() < Duration::from_secs(60))
        });
        if let Some(state) = registry.get(&(local, peer)) {
            return Ok(Self(state.clone()));
        }
        // Bound disconnected rate history too; do not evict a peer that could
        // reconnect to reset its limits.
        if registry.len() >= 4096 {
            return Err(Exhausted);
        }
        let result = Self::default();
        registry.insert((local, peer), result.0.clone());
        Ok(result)
    }
    pub fn classify(&self) -> Result<Classification, Exhausted> {
        let mut state = self.0.lock().unwrap();
        if state.streams >= 16 || state.unclassified >= 2 {
            return Err(Exhausted);
        }
        state.streams += 1;
        state.unclassified += 1;
        Ok(Classification {
            scheduler: self.clone(),
            active: true,
        })
    }
    pub fn admit(&self, method: &str, bytes: usize) -> Result<Permit, Exhausted> {
        self.admit_inner(method, bytes, false)
    }
    fn admit_inner(
        &self,
        method: &str,
        bytes: usize,
        provisional: bool,
    ) -> Result<Permit, Exhausted> {
        let bulk = !is_control(method);
        let git = matches!(method, "hub.git.push" | "hub.git.fetch");
        let mut state = self.0.lock().unwrap();
        if (!provisional && state.streams >= 16)
            || (bulk && (state.bulk >= 15 || state.streams >= 15 + usize::from(provisional)))
            || (git && state.git >= 2)
            || bytes > MAX_QUEUED_BYTES - state.bytes
            || (bulk && bytes > MAX_QUEUED_BYTES - CONTROL_BYTES - state.bulk_bytes)
        {
            return Err(Exhausted);
        }
        let rate_ok = match method {
            "hub.health" => state.health.take(120),
            "hub.enroll" => state.enroll.take(32),
            _ if !bulk => state.control.take(240),
            _ => true,
        };
        if !rate_ok {
            return Err(Exhausted);
        }
        state.streams += usize::from(!provisional);
        state.bulk += usize::from(bulk);
        state.git += usize::from(git);
        state.bytes += bytes;
        state.bulk_bytes += if bulk { bytes } else { 0 };
        Ok(Permit {
            scheduler: self.clone(),
            bulk,
            git,
            bytes,
        })
    }
}
pub fn is_control(method: &str) -> bool {
    matches!(
        method,
        "hub.stop" | "hub.cancel" | "hub.health" | "hub.receipt"
    )
}
pub struct Classification {
    scheduler: PeerScheduler,
    active: bool,
}
impl Classification {
    pub fn admit(&mut self, method: &str, bytes: usize) -> Result<Permit, Exhausted> {
        if !self.active {
            return Err(Exhausted);
        }
        let permit = self.scheduler.admit_inner(method, bytes, true)?;
        self.scheduler.0.lock().unwrap().unclassified -= 1;
        self.active = false;
        Ok(permit)
    }
}
impl Drop for Classification {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let mut state = self.scheduler.0.lock().unwrap();
        state.streams -= 1;
        state.unclassified -= 1;
    }
}
pub struct Permit {
    scheduler: PeerScheduler,
    bulk: bool,
    git: bool,
    bytes: usize,
}
impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.scheduler.0.lock().unwrap();
        state.streams -= 1;
        state.bulk -= usize::from(self.bulk);
        state.git -= usize::from(self.git);
        state.bytes -= self.bytes;
        state.bulk_bytes -= if self.bulk { self.bytes } else { 0 };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provisional_slot_sixteen_cannot_become_bulk_even_with_existing_control() {
        let scheduler = PeerScheduler::default();
        let mut held = Vec::new();
        held.push(scheduler.admit("hub.cancel", 1).unwrap());
        for _ in 0..14 {
            held.push(scheduler.admit("hub.status", 1).unwrap());
        }
        let mut classification = scheduler.classify().unwrap();
        assert!(classification.admit("hub.status", 1).is_err());
        drop(classification);
        assert!(scheduler.admit("hub.status", 1).is_err());
        assert!(scheduler.admit("hub.stop", 1).is_ok());
    }
    #[test]
    fn reconnect_does_not_reset_peer_rate_limits() {
        let local = iroh::SecretKey::from_bytes(&[11; 32]).public();
        let peer = iroh::SecretKey::from_bytes(&[12; 32]).public();
        let scheduler = PeerScheduler::shared(local, peer).unwrap();
        for _ in 0..120 {
            drop(scheduler.admit("hub.health", 0).unwrap());
        }
        drop(scheduler);
        let scheduler = PeerScheduler::shared(local, peer).unwrap();
        assert!(scheduler.admit("hub.health", 0).is_err());
    }
}
