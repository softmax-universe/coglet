//! Native backend boundary.
//!
//! A [`Backend`] is the only place where physical effects (audio, display) happen. The
//! core never fabricates playback: an action completes only when the backend reports it
//! through the [`Sink`]. Every method has a fail-closed default, so a backend that
//! implements nothing advertises nothing and executes nothing.
//!
//! Threading: backends are `Send + Sync`; native callbacks may call the [`Sink`] from any
//! thread. The core drains the sink only on its own (control queue) thread, so ordering
//! of outputs is deterministic. [`Backend::stop_all`] is the kill reflex: it is invoked
//! from the kill switch's thread, must not block and must not wait on the sink or core.

use crate::error::ApiError;
use serde_json::Value as Json;
use std::collections::{BTreeSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// Max undelivered native messages. Overflow fails the link closed (kill still works).
pub const SINK_BOUND: usize = 4096;

/// Shared kill fence. Check under the SAME native state lock as stop_all,
/// atomically with scheduling/applying the effect (including deferred jobs).
#[derive(Clone, Debug)]
pub struct KillFence(
    pub(crate) Arc<AtomicBool>,
    pub(crate) Arc<AtomicU64>,
    pub(crate) u64,
);
impl KillFence {
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst) || self.1.load(Ordering::SeqCst) != self.2
    }
    pub fn check(&self) -> Result<(), ApiError> {
        if self.is_stopped() {
            Err(ApiError::new(
                crate::error::Code::Cancelled,
                "kill fence engaged",
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Clone, Debug)]
pub struct ActionRequest {
    /// Native dispatch token, NOT the wire action id. Complete/cancel using this.
    pub id: String,
    pub wire_id: String,
    pub kill_fence: KillFence,
    pub coglet: String,
    pub op: String,
    pub args: Json,
    /// Exclusive expiry in the surface clock (microseconds).
    pub deadline: u64,
    pub grant_epoch: u64,
}

#[derive(Clone, Debug)]
pub struct ReadSnapshot {
    pub value: Json,
    pub snapshot: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendHealth {
    Ready,
    Degraded,
}

/// Native observations/requests normalised by the core before they reach the wire.
#[derive(Clone, Debug, PartialEq)]
pub enum Native {
    SpeechStart {
        utterance: String,
        position_ms: u64,
    },
    SpeechPartial {
        utterance: String,
        revision: u64,
        text: String,
    },
    SpeechEnd {
        utterance: String,
        revision: u64,
        text: String,
        position_ms: u64,
    },
    Position {
        stream: String,
        chunk: u64,
        item: String,
        played_ms: u64,
        position_ms: u64,
        state: String,
    },
    /// An already-applied native stop/duck (reflex). Never awaits a coglet step.
    BargeIn {
        utterance: String,
        stream: String,
        item: String,
        played_ms: u64,
        position_ms: u64,
        reflex: String,
    },
    /// Provider request for a tool; NOT an executed tool.
    ToolCall {
        session: String,
        call: String,
        name: String,
        arguments: Json,
    },
    /// Provider session ended natively (transport lost, provider closed).
    SessionEnded {
        session: String,
    },
}

#[derive(Debug)]
pub enum SinkItem {
    /// Terminal result for an admitted action: `Ok(())` means the effect was actually
    /// applied (rendered), never "synthesised" or "queued".
    Complete {
        id: String,
        result: Result<(), ApiError>,
    },
    Native {
        observation: Native,
        owner: Option<String>,
        session: Option<String>,
    },
}

#[derive(Default)]
struct SinkInner {
    q: VecDeque<SinkItem>,
    overflow: bool,
    generation: u64,
}

/// Cloneable, thread-safe channel from native code into the core.
#[derive(Clone)]
pub struct Sink {
    inner: Arc<Mutex<SinkInner>>,
    generation: u64,
    owner: Option<String>,
    session: Option<String>,
    fence: KillFence,
}

impl Sink {
    pub(crate) fn new(fence: KillFence) -> Self {
        Self {
            inner: Arc::default(),
            generation: 0,
            owner: None,
            session: None,
            fence,
        }
    }
    /// Native starts initiated through bind (e.g. microphone capture) MUST check
    /// this under their device state lock shared with stop_all. Capture this
    /// immutable fence with the generation, never fetch a newer one in old jobs.
    pub fn kill_fence(&self) -> &KillFence {
        &self.fence
    }
    /// Immutable native acquisition generation (not a timestamp at callback arrival).
    pub fn capture_generation(&self) -> u64 {
        self.generation
    }
    pub fn owner(&self) -> Option<&str> {
        self.owner.as_deref()
    }
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Capture at native generation creation, never inside a late callback.
    pub fn capture(&self, owner: &str, session: Option<&str>) -> Self {
        Self {
            owner: Some(owner.to_owned()),
            session: session.map(str::to_owned),
            ..self.clone()
        }
    }
    pub(crate) fn renew(&self, owner: Option<String>) -> Self {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        g.generation += 1;
        g.q.retain(|item| matches!(item, SinkItem::Complete { .. }));
        Self {
            inner: self.inner.clone(),
            generation: g.generation,
            owner,
            session: None,
            fence: KillFence(
                self.fence.0.clone(),
                self.fence.1.clone(),
                self.fence.1.load(Ordering::SeqCst),
            ),
        }
    }

    pub fn complete(&self, id: &str, result: Result<(), ApiError>) {
        self.push(SinkItem::Complete {
            id: id.to_owned(),
            result,
        });
    }
    pub fn native(&self, n: Native) {
        self.push(SinkItem::Native {
            observation: n,
            owner: self.owner.clone(),
            session: self.session.clone(),
        });
    }
    fn push(&self, item: SinkItem) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if matches!(item, SinkItem::Native { .. }) && self.generation != g.generation {
            return;
        }
        if g.q.len() >= SINK_BOUND {
            g.overflow = true;
        } else {
            g.q.push_back(item);
        }
    }
    pub(crate) fn drain(&self) -> (Vec<SinkItem>, bool) {
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        (g.q.drain(..).collect(), std::mem::take(&mut g.overflow))
    }
}

pub trait Backend: Send + Sync {
    /// Catalog operations this backend genuinely implements. Everything else is neither
    /// advertised nor granted. Unqualified optional operations are dropped by the core
    /// regardless (see `catalog::UNQUALIFIED`).
    fn operations(&self) -> BTreeSet<String> {
        BTreeSet::new()
    }
    /// Snapshot for `speech.config` (installed by trusted operator policy).
    fn speech_config(&self) -> Option<Json> {
        None
    }
    /// Allowlisted provider presets for `speech.session_open` (never URLs/credentials).
    fn session_routes(&self) -> Vec<String> {
        Vec::new()
    }
    /// Allowlisted provider tool names; others are dropped by the core.
    fn tool_allowlist(&self) -> Vec<String> {
        Vec::new()
    }
    /// Refreshed at every grant epoch. Retain a clone when STARTING a native
    /// generation; callbacks must use that clone, never the latest binding.
    /// Provider callbacks must use sink.capture(owner, Some(session_id)).
    /// Mic callbacks use the bound sender's immutable capture generation/owner.
    /// Native starts through this binding MUST check sink.kill_fence() under the
    /// native state lock shared with stop_all, atomically with the device effect.
    fn bind(&self, _sink: Sink) {}
    /// Cheap admission check with no effect (device present, route usable...).
    fn preflight(&self, _req: &ActionRequest, _now: u64) -> Result<(), ApiError> {
        Ok(())
    }
    /// Begin the effect. Completion MUST be reported via [`Sink::complete`] (may be
    /// before this returns). An `Err` is recorded as a failed terminal outcome.
    /// REQUIRED: req.kill_fence.check() under the same backend/device state
    /// lock as stop_all, atomically with starting the effect. Deferred jobs must
    /// retain/recheck the fence at their actual device-start boundary too.
    fn execute(&self, _req: &ActionRequest, _now: u64) -> Result<(), ApiError> {
        Err(ApiError::unavailable(
            "operation not implemented by backend",
        ))
    }
    /// Stop the physical effect identified by its native ActionRequest::id token
    /// (NOT wire_id). No completion is expected afterwards; late completions are
    /// fenced by the core, even if another owner reuses the wire id.
    fn cancel(&self, _dispatch_token: &str) {}
    fn read(&self, _op: &str, _args: &Json, _now: u64) -> Result<ReadSnapshot, ApiError> {
        Err(ApiError::unavailable("read not implemented by backend"))
    }
    /// Native timers (for example expression expiry). Called on every core tick.
    fn tick(&self, _now: u64) {}
    /// Owner lost resources/grants (revoke, expiry, disconnect): stop its native I/O and
    /// reset its state before reassignment.
    fn revoke(&self, _coglet: &str) {}
    /// Kill reflex. Idempotent, non-blocking, independent of the control channel.
    fn stop_all(&self) {}
    fn health(&self) -> BackendHealth {
        BackendHealth::Ready
    }
}
