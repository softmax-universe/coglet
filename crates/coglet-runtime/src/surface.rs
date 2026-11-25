//! Shared surface-message boundary: catalog/grants, private events/reads,
//! durable receipt authority and native committed-dispatch pumping.
use crate::{Result, cbor, validate};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[path = "surface_authority.rs"]
mod authority;
#[path = "surface_reads.rs"]
mod reads;

pub const SURFACE_MAX_FRAME: usize = 1_048_576;
pub const SURFACE_QUEUE: usize = 128;

/// Trusted supervisor configuration, never populated from hello. Transport
/// authentication must succeed before creating a session with this binding.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceBinding {
    surface: String,
    permitted_coglets: Vec<String>,
}
impl SurfaceBinding {
    pub fn new(surface: String, permitted_coglets: Vec<String>) -> Result<Self> {
        validate("surface-id", &json!(surface))?;
        let mut seen = BTreeSet::new();
        for coglet in &permitted_coglets {
            validate("coglet-id", &json!(coglet))?;
            if !seen.insert(coglet) {
                return Err("duplicate permitted coglet".into());
            }
        }
        Ok(Self {
            surface,
            permitted_coglets,
        })
    }
    pub fn from_config(raw: &[u8]) -> Result<Self> {
        let binding: Self = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
        Self::new(binding.surface, binding.permitted_coglets)
    }
    pub fn surface(&self) -> &str {
        &self.surface
    }
}

pub struct SurfaceSession {
    catalog: std::sync::Arc<crate::TrustedExtensionCatalog>,
    binding: SurfaceBinding,
    negotiated: bool,
    closed: bool,
    stopped: bool,
    peer_clock: Option<u64>,
    runtime_clock: u64,
    last_received: u64,
    last_health: u64,
    output: VecDeque<Vec<u8>>,
    revision: u64,
    epoch: u64,
    operations: BTreeMap<String, BTreeSet<String>>,
    resources: BTreeSet<String>,
    grants: BTreeMap<String, Value>,
    mapping: Option<(u64, u64)>,
    killed: bool,
    reads: BTreeMap<String, reads::PendingRead>,
    read_ids: BTreeSet<String>,
    read_results: VecDeque<(String, Value)>,
    read_result_authority: BTreeMap<String, reads::PendingRead>,
    heard_reads: BTreeSet<String>,
    read_expiry_diagnostics: VecDeque<Value>,
    service: Option<std::sync::Arc<std::sync::Mutex<crate::RuntimeService>>>,
    sent_dispatches: BTreeSet<String>,
}
impl SurfaceSession {
    pub(crate) fn transport_now(&self, probe_clock: std::time::Instant) -> Result<u64> {
        if let Some(service) = &self.service {
            service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .supervised_now()
        } else {
            u64::try_from(probe_clock.elapsed().as_micros())
                .map_err(|_| "runtime clock overflow".into())
        }
    }
    pub fn new(binding: SurfaceBinding) -> Self {
        Self {
            catalog: std::sync::Arc::new(crate::TrustedExtensionCatalog::default()),
            binding,
            negotiated: false,
            closed: false,
            stopped: true,
            peer_clock: None,
            runtime_clock: 0,
            last_received: 0,
            last_health: 0,
            output: VecDeque::new(),
            revision: 0,
            epoch: 0,
            operations: BTreeMap::new(),
            resources: BTreeSet::new(),
            grants: BTreeMap::new(),
            mapping: None,
            killed: false,
            reads: BTreeMap::new(),
            read_ids: BTreeSet::new(),
            read_results: VecDeque::new(),
            read_result_authority: BTreeMap::new(),
            heard_reads: BTreeSet::new(),
            read_expiry_diagnostics: VecDeque::new(),
            service: None,
            sent_dispatches: BTreeSet::new(),
        }
    }
    pub fn is_closed(&self) -> bool {
        self.closed
    }
    /// Native transport pump. Reconnect automatically reconciles saved IDs.
    pub fn pump_dispatch(&mut self, now: u64) -> Result<()> {
        if !self.negotiated || self.closed {
            return Ok(());
        }
        let Some(service) = self.service.clone() else {
            return Ok(());
        };
        self.pump_supervisor_reads(now)?;
        #[cfg(feature = "native-engine")]
        service
            .lock()
            .map_err(|_| "runtime service poisoned")?
            .run_native(self, now)?;
        while self.output.len() < SURFACE_QUEUE {
            // Guest execution and fsync can outlive the admission lease. Native
            // routes recheck the supervised clock for EACH new dispatch, never
            // authorize with the pre-execution transport sample.
            #[cfg(feature = "native-engine")]
            let now = {
                let mut guard = service.lock().map_err(|_| "runtime service poisoned")?;
                if guard.native_enabled() {
                    guard.supervised_now()?
                } else {
                    now
                }
            };
            let next = service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .next_dispatch(self, &self.sent_dispatches, now)?;
            let Some((id, bytes)) = next else {
                break;
            };
            self.output.push_back(bytes);
            self.sent_dispatches.insert(id);
        }
        Ok(())
    }
    fn health(&self, now: u64, correlation: &Value) -> Result<Value> {
        let (state, pending) = if let Some(service) = &self.service {
            let pending = service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .admitted_pending(self.surface());
            let ready = !self.stopped
                && !self.killed
                && self.grants.values().any(|g| {
                    self.surface_now(now)
                        .is_ok_and(|clock| g["expires"].as_u64().is_some_and(|e| e > clock))
                });
            (if ready { "ready" } else { "stopped" }, pending)
        } else {
            ("stopped", 0)
        };
        Ok(
            json!({"type":"health","body":{"state":state,"clock":now,"pending":pending,"correlation_id":correlation}}),
        )
    }
    pub fn front(&self) -> Option<&[u8]> {
        self.output.front().map(Vec::as_slice)
    }
    pub(crate) fn output_depth(&self) -> usize {
        self.output.len()
    }
    pub fn pop(&mut self) -> Option<Vec<u8>> {
        self.output.pop_front()
    }
    pub fn disconnect(&mut self) {
        if let Some(service) = &self.service
            && let Ok(mut service) = service.lock()
        {
            let _ = service.purge_surface_events(self.surface(), self.epoch);
        }
        self.closed = true;
        self.stopped = true;
        self.negotiated = false;
        self.peer_clock = None;
        self.output.clear();
        self.grants.clear();
        self.mapping = None;
        self.reads.clear();
        self.read_results.clear();
        self.read_result_authority.clear();
        self.heard_reads.clear();
    }
    fn enqueue(&mut self, message: Value) -> Result<()> {
        validate("surface-message", &message)?;
        let bytes = cbor(&message)?;
        if bytes.len() > SURFACE_MAX_FRAME {
            return Err("response too large".into());
        }
        self.output.push_back(bytes);
        Ok(())
    }
    fn error(&mut self, code: &str, correlation: &Value) -> Result<()> {
        self.enqueue(json!({"type":"error","body":{"error":{"code":code,"message":"Surface session refused","retryable":false},"correlation_id":correlation}}))?;
        // Retain the one terminal error for recv/socket flush, but no authority.
        self.closed = true;
        self.stopped = true;
        self.negotiated = false;
        self.peer_clock = None;
        Ok(())
    }
    /// Drive health/inactivity on the serialized control queue. Embedded callers
    /// must poll at least once per second while connected.
    pub fn tick(&mut self, now: u64) {
        if self.closed {
            return;
        }
        if now < self.runtime_clock {
            self.disconnect();
            return;
        }
        self.runtime_clock = now;
        self.expire_reads(now);
        if !self.negotiated {
            return;
        }
        if now - self.last_received >= 3_000_000 {
            self.disconnect();
            return;
        }
        if now - self.last_health >= 1_000_000 {
            if self.output.len() == SURFACE_QUEUE {
                self.disconnect();
                return;
            }
            let mut id = [0u8; 16];
            // Runtime correlation IDs remain unique across process restarts.
            #[cfg(unix)]
            let random_ok = unsafe { libc::getentropy(id.as_mut_ptr().cast(), id.len()) } == 0;
            #[cfg(not(unix))]
            let random_ok = false;
            if !random_ok
                || self
                    .health(now, &json!(hex::encode(id)))
                    .and_then(|h| self.enqueue(h))
                    .is_err()
            {
                self.disconnect();
                return;
            }
            self.last_health = now;
        }
    }
    /// Same return values as the C send API. Full queue copies nothing and does
    /// not update protocol state, so the exact input can be retried.
    pub fn send(&mut self, raw: &[u8], now: u64) -> i32 {
        if self.closed {
            return -2;
        }
        if raw.is_empty() || raw.len() > SURFACE_MAX_FRAME {
            return -1;
        }
        if self.output.len() == SURFACE_QUEUE {
            return 1;
        }
        if self.negotiated && now.saturating_sub(self.last_received) >= 3_000_000 {
            self.disconnect();
            return -2;
        }
        if self.receive(raw, now).is_err() {
            self.disconnect();
            return -2;
        }
        0
    }
    fn receive(&mut self, raw: &[u8], now: u64) -> Result<()> {
        let decoded = coglet_types::decode_profile(raw)?;
        coglet_types::validate("surface-message", &decoded)?;
        let message: Value = decoded.deserialized().map_err(|e| e.to_string())?;
        let body = &message["body"];
        let kind = message["type"].as_str().ok_or("missing type")?;
        if now < self.runtime_clock {
            return Err("runtime clock rollback".into());
        }
        if !self.negotiated {
            if kind != "hello" {
                return Err("hello required".into());
            }
            if body["surface"] != self.binding.surface {
                return self.error("unauthenticated", &body["correlation_id"]);
            }
            if !body["offer"]["versions"]
                .as_array()
                .ok_or("offer required")?
                .iter()
                .any(|v| v == "0.1")
            {
                return self.error("unsupported_version", &body["correlation_id"]);
            }
            self.enqueue(json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":body["correlation_id"]}}))?;
            self.negotiated = true;
            self.last_health = now;
            self.peer_clock = body["clock"].as_u64();
        } else {
            // Runtime->surface and repeated negotiation messages are fatal,
            // even when structurally valid in the bidirectional schema.
            match kind {
                "health" => {
                    let clock = body["clock"].as_u64().ok_or("missing clock")?;
                    if self.peer_clock.is_some_and(|previous| clock < previous) {
                        return Err("surface clock rollback".into());
                    }
                    self.peer_clock = Some(clock);
                    self.mapping = Some((now, clock));
                    self.last_health = now;
                    self.enqueue(self.health(now, &body["correlation_id"])?)?;
                }
                "kill" => {
                    if let Some(service) = &self.service {
                        service
                            .lock()
                            .map_err(|_| "runtime service poisoned")?
                            .purge_surface_events(self.surface(), self.epoch)?;
                    }
                    self.killed = true;
                    self.grants.clear();
                    self.stopped = true;
                    self.expire_reads(now);
                    self.enqueue(json!({"type":"health","body":{"state":"stopped","clock":now,"pending":0,"correlation_id":body["correlation_id"]}}))?;
                }
                "capabilities" => self.install_capabilities(body, now)?,
                "grants" => self.install_grants(body, now)?,
                "accept" => {
                    let service = self
                        .service
                        .clone()
                        .ok_or("no durable receipt coordinator")?;
                    service
                        .lock()
                        .map_err(|_| "runtime service poisoned")?
                        .record_surface_accept(self, body, now)?;
                }
                "event" if body["kind"] == "action.outcome" => {
                    let service = self
                        .service
                        .clone()
                        .ok_or("no durable receipt coordinator")?;
                    service
                        .lock()
                        .map_err(|_| "runtime service poisoned")?
                        .record_surface_outcome(self, body, now)?;
                }
                "read_reply" => self.receive_read_reply(body, now)?,
                "event" => {
                    let service = self.service.clone().ok_or("no durable event coordinator")?;
                    let heard = service
                        .lock()
                        .map_err(|_| "runtime service poisoned")?
                        .record_surface_event(self, body, now)?;
                    if let Some(id) = heard {
                        let owner = service
                            .lock()
                            .map_err(|_| "runtime service poisoned")?
                            .heard_owner();
                        // Persistence is not part of this not-yet-issued read's
                        // allowance. Existing PendingRead deadlines stay fixed.
                        let publication_time = service
                            .lock()
                            .map_err(|_| "runtime service poisoned")?
                            .read_publication_now(now)?;
                        // This event passed the arrival-time inactivity check.
                        // Count persistence against this receipt, not the prior
                        // message. A >=3s commit still fails the unchanged fence.
                        self.last_received = now;
                        self.request_read(
                            &owner,
                            "speech.heard",
                            json!({"item":body["payload"]["item"]}),
                            &id,
                            publication_time
                                .checked_add(1_000_000)
                                .ok_or("read deadline overflow")?,
                            publication_time,
                        )?;
                        self.heard_reads.insert(id);
                    }
                }
                "error" => {
                    self.disconnect();
                }
                _ => return Err("wrong direction or lifecycle".into()),
            }
        }
        // This is the transport's input high-water mark, not an internal
        // publication sample: its post-send pump may reuse the arrival sample.
        self.runtime_clock = now;
        self.last_received = now;
        Ok(())
    }
}
