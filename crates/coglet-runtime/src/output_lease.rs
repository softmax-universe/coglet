//! ADR-0033 §4/§5: process-local output custody and the durable emergency stop
//! obligation seam.
//!
//! `OutputGate` is deliberately independent of router SQL, the runtime service
//! mutex and every guest call: its only publication critical section is a
//! single nonblocking socket send. It is created CLOSED at every
//! process start; nothing restores it from storage.
//!
//! The durable half (the pre-armed stop obligation in the SAME WAL as the
//! installed-push state) can only live inside `RuntimeService`'s private state,
//! owned by policy_emergency in the managed/WAL module tree. No lease is enabled
//! before that same-store obligation has committed.
use crate::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, Ordering},
};
#[cfg(test)]
use std::time::Duration;

const CLOSED: u8 = 0;
const ENABLED: u8 = 1;
const REVOKED: u8 = 2;
const UNCERTAIN: u8 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseState {
    /// Boot state, and the state after an enable was never granted.
    Closed,
    Enabled,
    /// Explicitly revoked in this daemon instance. Sticky: no re-enable.
    Revoked,
    /// A revoke (or its durable record) could not be completed. Sticky.
    Uncertain,
}
impl LeaseState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Closed => "closed",
            Self::Enabled => "enabled",
            Self::Revoked => "revoked",
            Self::Uncertain => "uncertain",
        }
    }
}

/// Per-target output gate. Every publication of bytes to an adapter goes through
/// [`OutputGate::publish`]; revocation linearizes against it.
#[derive(Debug)]
pub struct OutputGate {
    state: AtomicU8,
    publication: Mutex<()>,
}
impl OutputGate {
    /// Always CLOSED: there is intentionally no constructor from stored state.
    pub fn new_closed() -> Arc<Self> {
        Arc::new(Self {
            state: AtomicU8::new(CLOSED),
            publication: Mutex::new(()),
        })
    }
    pub fn state(&self) -> LeaseState {
        match self.state.load(Ordering::SeqCst) {
            CLOSED => LeaseState::Closed,
            ENABLED => LeaseState::Enabled,
            REVOKED => LeaseState::Revoked,
            _ => LeaseState::Uncertain,
        }
    }
    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::SeqCst) == ENABLED
    }
    /// Only CLOSED -> ENABLED. Revoked/uncertain/already-enabled all refuse; a
    /// revoked gate can be reopened only by a new daemon process.
    pub fn enable(&self) -> Result<()> {
        self.state
            .compare_exchange(CLOSED, ENABLED, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| ())
            .map_err(|_| "permission_denied: output lease cannot be enabled in this state".into())
    }
    /// Close synchronously, independently of SQL, runtime custody and guests.
    /// The state change excludes new publishers; draining the one nonblocking
    /// send already holding publication custody excludes post-return exposure.
    /// Never abandon a still-running publisher on a timeout.
    pub fn revoke(&self) -> Result<()> {
        // Preserve a prior UNCERTAIN; otherwise REVOKED.
        let mut cur = self.state.load(Ordering::SeqCst);
        loop {
            let next = if cur == UNCERTAIN { UNCERTAIN } else { REVOKED };
            match self
                .state
                .compare_exchange(cur, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
        // Only a single nonblocking send may own this mutex in production.
        // Never return while a publisher can still expose bytes: a timeout here
        // would turn an uncertainty report into a post-return physical effect.
        let _drained = self.publication.lock().unwrap_or_else(|p| p.into_inner());
        Ok(())
    }
    /// Sticky closed-and-unknown (failed durable record, drain overrun).
    pub fn mark_uncertain(&self) {
        self.state.store(UNCERTAIN, Ordering::SeqCst);
    }
    /// Expose one complete framed buffer with exactly one nonblocking syscall.
    /// A short write or backpressure ends the connection; never wait for a peer
    /// while holding publication custody. Buffer construction happens outside.
    pub(crate) fn send(&self, stream: &std::os::unix::net::UnixStream, frame: &[u8]) -> Result<()> {
        use std::os::fd::AsRawFd;
        self.publish(|| {
            // SAFETY: stream owns a live fd; frame is readable for frame.len().
            // MSG_DONTWAIT is per-call and does not alter concurrent reads.
            let sent = unsafe {
                libc::send(
                    stream.as_raw_fd(),
                    frame.as_ptr().cast(),
                    frame.len(),
                    libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                )
            };
            if sent < 0 {
                Err(std::io::Error::last_os_error().to_string())
            } else if sent as usize != frame.len() {
                Err("unavailable: partial output frame; closing connection".into())
            } else {
                Ok(())
            }
        })?
    }
    /// The only production caller is `send`: do not place blocking callbacks,
    /// allocations, retries, service locks or guest calls in this critical section.
    fn publish<R>(&self, expose: impl FnOnce() -> R) -> Result<R> {
        let _held = self
            .publication
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if self.state.load(Ordering::SeqCst) != ENABLED {
            return Err("unavailable: output lease closed".into());
        }
        Ok(expose())
    }
}

/// The exact stop to reconcile: retained BEFORE the lease is granted, in the
/// installed-push WAL. `request` is the full original `hub.stop` envelope whose
/// `operation_intent` digest equals `intent`; reconcile never re-derives actor,
/// ticket or ID from a reader or from live configuration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmergencyObligation {
    pub version: u8,
    pub target: String,
    pub operation_id: String,
    pub intent: String,
    pub request: Value,
    pub actor: String,
    pub ticket: Option<String>,
    pub grant_generation: u64,
    pub delegation_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]

pub enum EmergencyStatus {
    /// No obligation has ever been retained for this WAL.
    None,
    /// Retained; no revoke recorded. After a restart this is UNRESOLVED: a
    /// failed revoke write is indistinguishable from "never revoked", so it must
    /// be reconciled (stop applied) before any new lease.
    Armed(EmergencyObligation),
    /// Revoke durably recorded; stop receipt not yet applied.
    StopPending(EmergencyObligation),
    /// Authoritative applied stop receipt recorded for this obligation.
    Stopped(EmergencyObligation),
}

/// Durable emergency custody. REQUIRED integration (managed.rs / policy_state.rs
/// owner): implement on `RuntimeService` inside the same module tree as
/// `ManagedState` (private `state`/`store`), persisting in the existing WAL via
/// `commit_managed`-style atomic frames, never a second store:
///
/// * `emergency_status` — read-only, from the committed state.
/// * `emergency_retain` — one atomic frame. `None` -> `Armed`. Identical
///   obligation while `Armed` -> idempotent Ok. `Stopped` with a DIFFERENT
///   `operation_id` -> replace with new `Armed`. Anything else (StopPending, or
///   same ID after Stopped, or a changed obligation while Armed/StopPending) ->
///   `conflict`. Must reject an `operation_id` already used in `operations` or by
///   the install receipt (existing ID-collision rule).
/// * `emergency_record_revoked` — `Armed|StopPending` -> `StopPending` (records
///   the daemon instance); idempotent; `None`/`Stopped` -> Ok without change.
/// * `emergency_record_stopped` — requires `receipt.state == "applied"` for the
///   obligation's `operation_id` AND `unresolved_attempt.is_none()`; else Err and
///   no change. Never erases historical receipts.
///
/// Open/replay must validate the record (version, obligation intent digest) and
/// fail closed on a corrupt one.
pub(crate) trait EmergencyCustody {
    fn emergency_status(&self, target: &str) -> Result<EmergencyStatus>;
    fn emergency_retain(&mut self, obligation: &EmergencyObligation) -> Result<()>;
    fn emergency_record_revoked(&mut self, target: &str, daemon_instance: &str) -> Result<()>;
    fn emergency_record_stopped(&mut self, target: &str, receipt: &Value) -> Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn nonreading_peer_cannot_hold_revoke_or_expose_after_return() {
        use std::io::Read;
        let (writer, mut reader) = std::os::unix::net::UnixStream::pair().unwrap();
        let gate = OutputGate::new_closed();
        gate.enable().unwrap();
        let bytes = [42u8; 4096];
        let started = std::time::Instant::now();
        let mut writes = 0;
        while gate.send(&writer, &bytes).is_ok() {
            writes += 1;
            assert!(writes < 4096, "socket buffer never reached backpressure");
        }
        assert!(writes > 0, "authorized pre-revoke publication control");
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "send blocked on a nonreading peer"
        );
        gate.revoke().unwrap();
        assert!(gate.send(&writer, b"post-return").is_err());
        drop(writer);
        let mut received = Vec::new();
        reader.read_to_end(&mut received).unwrap();
        assert!(received.len() >= writes * bytes.len());
        assert!(received.iter().all(|b| *b == 42));
        assert!(!gate.is_open());
    }

    #[test]
    fn gate_boots_closed_and_refuses_publication() {
        let gate = OutputGate::new_closed();
        assert_eq!(gate.state(), LeaseState::Closed);
        assert!(gate.publish(|| unreachable!()).is_err());
        gate.enable().unwrap();
        assert_eq!(gate.publish(|| 7).unwrap(), 7);
        assert!(gate.enable().is_err(), "no double enable");
    }

    #[test]
    fn revoke_is_sticky_and_blocks_later_publication() {
        let gate = OutputGate::new_closed();
        gate.enable().unwrap();
        gate.revoke().unwrap();
        assert_eq!(gate.state(), LeaseState::Revoked);
        assert!(gate.publish(|| unreachable!()).is_err());
        assert!(gate.enable().is_err(), "revoked gate cannot be re-enabled");
        gate.mark_uncertain();
        assert_eq!(gate.state(), LeaseState::Uncertain);
        gate.revoke().unwrap();
        assert_eq!(
            gate.state(),
            LeaseState::Uncertain,
            "uncertain is preserved"
        );
        let closed = OutputGate::new_closed();
        closed.revoke().unwrap();
        assert!(closed.enable().is_err(), "revoke before enable is sticky");
    }

    #[test]
    fn revoke_waits_for_started_write_and_nothing_follows() {
        let gate = OutputGate::new_closed();
        gate.enable().unwrap();
        let started = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let writer = {
            let (gate, started, finished) = (gate.clone(), started.clone(), finished.clone());
            std::thread::spawn(move || {
                gate.publish(|| {
                    started.store(true, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(150));
                    finished.store(true, Ordering::SeqCst);
                })
                .unwrap();
            })
        };
        while !started.load(Ordering::SeqCst) {
            std::thread::yield_now();
        }
        gate.revoke().unwrap();
        assert!(
            finished.load(Ordering::SeqCst),
            "revoke returned only after the pre-revoke exposure completed"
        );
        writer.join().unwrap();
        let exposed_after = AtomicBool::new(false);
        assert!(
            gate.publish(|| exposed_after.store(true, Ordering::SeqCst))
                .is_err()
        );
        assert!(!exposed_after.load(Ordering::SeqCst));
    }

    #[test]
    fn concurrent_publishers_never_expose_after_revoke_returns() {
        let gate = OutputGate::new_closed();
        gate.enable().unwrap();
        let revoked = Arc::new(AtomicBool::new(false));
        let violations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let (gate, revoked, violations) =
                    (gate.clone(), revoked.clone(), violations.clone());
                std::thread::spawn(move || {
                    for _ in 0..20_000 {
                        let _ = gate.publish(|| {
                            if revoked.load(Ordering::SeqCst) {
                                violations.fetch_add(1, Ordering::SeqCst);
                            }
                        });
                    }
                })
            })
            .collect();
        std::thread::sleep(Duration::from_millis(2));
        gate.revoke().unwrap();
        revoked.store(true, Ordering::SeqCst);
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(violations.load(Ordering::SeqCst), 0);
    }
}
