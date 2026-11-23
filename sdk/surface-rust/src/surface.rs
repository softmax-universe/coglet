//! The surface core: lifecycle, admission, grants, events, health and kill.
//!
//! `Surface` implements [`ControlPeer`] and is driven by the transports in
//! `transport.rs` (socket, in-process queue, embedded C ABI). Every inbound message is
//! decoded as core-deterministic CBOR, validated against the generated `surface` schema
//! and the pinned operation catalog, and checked against the lifecycle state machine
//! before any side effect. Outbound messages are validated and deterministically encoded
//! with the same schema before they leave.
//!
//! Durability order for an action (surface-protocol.md "Admission, recovery and bounds"):
//! full `act` envelope + admission (or rejection) is `fdatasync`ed in the ledger, THEN the
//! accept is returned and the backend effect started; the terminal result is synced
//! BEFORE the outcome event is emitted. Exact retries are answered from the ledger and
//! never execute twice. This is deduplication, not exactly-once physical effects.

use crate::backend::{ActionRequest, Backend, BackendHealth, KillFence, Native, Sink, SinkItem};
use crate::catalog::{
    Catalog, Direction, UNQUALIFIED, catalog, required_resources, rule_key, schema_uri,
};
use crate::config::{Config, GrantSpec, check_grants};
use crate::error::{ApiError, Code};
use crate::ids::random_id;
use crate::speech::{Normalized, SpeechState};
use crate::store::{EntryState, RETENTION_US, Store, StoreError, WallClock, latch};
use crate::transport::{ControlPeer, MAX_FRAME};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

const MAX_REASON_BYTES: usize = 1024;

/// What the surface advertised and what it refused to advertise (and why).
#[derive(Clone, Debug, Default)]
pub struct Advertisement {
    pub entries: Vec<Json>,
    pub operations: BTreeSet<String>,
    pub resources: BTreeSet<String>,
    /// Backend-declared operations that were NOT advertised, with the reason.
    pub withheld: Vec<(String, &'static str)>,
}

fn advertise(cat: &Catalog, backend: &dyn Backend) -> Advertisement {
    let declared = backend.operations();
    let mut adv = Advertisement::default();
    let session_ok =
        declared.contains("speech.session_open") && !backend.session_routes().is_empty();
    let mut by_entry: BTreeMap<String, [Vec<String>; 3]> = BTreeMap::new();
    for name in &declared {
        let Some(op) = cat.op(name) else {
            adv.withheld
                .push((name.clone(), "not in the pinned catalog"));
            continue;
        };
        let Some(entry) = &op.entry else {
            adv.withheld.push((name.clone(), "not a surface operation"));
            continue;
        };
        if UNQUALIFIED.contains(&name.as_str()) || op.direction == Direction::Gateway {
            adv.withheld.push((
                name.clone(),
                "unqualified optional operation (open M1 gate)",
            ));
            continue;
        }
        if entry == "speech.session" && !session_ok {
            adv.withheld.push((
                name.clone(),
                "provider session needs session_open and allowlisted routes",
            ));
            continue;
        }
        if name == "speech.config" && backend.speech_config().is_none() {
            adv.withheld
                .push((name.clone(), "no installed speech configuration"));
            continue;
        }
        let slot = match op.direction {
            Direction::Event => 0,
            Direction::Read => 1,
            _ => 2,
        };
        by_entry.entry(entry.clone()).or_default()[slot].push(name.clone());
        adv.operations.insert(name.clone());
    }
    for (entry, [events, reads, actions]) in by_entry {
        let mut exclusive = BTreeSet::new();
        let mut schemas = serde_json::Map::new();
        for n in events.iter().chain(&reads).chain(&actions) {
            let op = cat.op(n).expect("checked above");
            let rule = if op.direction == Direction::Read {
                op.output.as_deref().unwrap_or(&op.input)
            } else {
                &op.input
            };
            schemas.insert(n.clone(), json!(schema_uri(rule)));
            if op.direction != Direction::Read {
                exclusive.extend(required_resources(n).iter().map(|s| (*s).to_owned()));
            }
        }
        adv.resources.extend(exclusive.iter().cloned());
        adv.entries.push(json!({
            "name": entry, "events": events, "reads": reads, "actions": actions,
            "exclusive": exclusive.into_iter().collect::<Vec<_>>(), "schemas": schemas,
        }));
    }
    adv
}

struct KillInner {
    latched: Arc<AtomicBool>,
    reason: Mutex<String>,
    backend: Arc<dyn Backend>,
    dir: PathBuf,
    persist_failed: AtomicBool,
    generation: Arc<AtomicU64>,
}

/// Local, latched off-switch. Cloneable and usable from any thread, including while the
/// control queue is full or the runtime is hung: it calls the backend's native
/// [`Backend::stop_all`] directly and never touches the ledger lock or the link. The wire
/// notification (kill + stopped health) follows on the next core tick. No wire message can
/// clear it; only [`Surface::reset_kill`] on a disconnected surface can.
#[derive(Clone)]
pub struct KillSwitch(Arc<KillInner>);

impl KillSwitch {
    /// Returns true if this call engaged the latch (false if already latched).
    pub fn trigger(&self, reason: &str) -> bool {
        let mut r: String = reason.chars().take(MAX_REASON_BYTES).collect();
        while r.len() > MAX_REASON_BYTES {
            r.pop();
        }
        if r.is_empty() {
            r = "local operator kill".into();
        }
        let first = !self.0.latched.swap(true, Ordering::SeqCst);
        if first {
            *self.0.reason.lock().unwrap_or_else(|e| e.into_inner()) = r;
        }
        // Reflex first; persistence is best effort and must not delay native stop.
        self.0.backend.stop_all();
        if latch(&self.0.dir).is_err() {
            self.0.persist_failed.store(true, Ordering::SeqCst);
        }
        first
    }
    pub fn is_latched(&self) -> bool {
        self.0.latched.load(Ordering::SeqCst)
    }
    fn reason(&self) -> String {
        self.0
            .reason
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Idle,
    AwaitReply,
    Active,
    Closed,
}

struct Pending {
    wire_id: String,
    coglet: String,
    op: String,
    args: Json,
    corr: String,
    deadline: u64,
}

#[derive(Default, Clone, Debug)]
pub struct Diagnostics {
    pub native_discarded: u64,
    pub events_suppressed: u64,
    pub late_completions: u64,
    pub unsound_completions: u64,
}

struct Snapshot {
    epoch: u64,
    entries: Vec<GrantSpec>,
}

pub struct Surface {
    cfg: Config,
    backend: Arc<dyn Backend>,
    store: Store,
    sink: Sink,
    kill: KillSwitch,
    kill_applied: bool,
    cat: &'static Catalog,
    adv: Advertisement,
    operator_grants: Vec<GrantSpec>,
    restrictions: Vec<String>,
    speech: SpeechState,
    phase: Phase,
    now: u64,
    last_rx: u64,
    last_health: u64,
    hello_corr: String,
    grants: Snapshot,
    pending: BTreeMap<String, Pending>,
    logical: u64,
    diag: Diagnostics,
    session_mode: Option<String>,
}

fn st<'a>(j: &'a Json, k: &str) -> &'a str {
    j.get(k).and_then(Json::as_str).unwrap_or_default()
}
fn un(j: &Json, k: &str) -> u64 {
    j.get(k).and_then(Json::as_u64).unwrap_or_default()
}
fn serr(e: StoreError) -> String {
    e.to_string()
}

impl Surface {
    pub fn open(cfg: Config, state_dir: &Path, backend: Arc<dyn Backend>) -> Result<Self, String> {
        Self::open_with_clock(cfg, state_dir, backend, crate::store::system_wall_clock())
    }

    pub fn open_with_clock(
        cfg: Config,
        state_dir: &Path,
        backend: Arc<dyn Backend>,
        wall: WallClock,
    ) -> Result<Self, String> {
        cfg.validate()?;
        let cat = catalog()?;
        let adv = advertise(cat, backend.as_ref());
        let store = Store::open_with_clock(state_dir, &cfg.surface, wall).map_err(serr)?;
        let kill = KillSwitch(Arc::new(KillInner {
            latched: Arc::new(AtomicBool::new(store.latched())),
            reason: Mutex::new("latched before restart".into()),
            backend: backend.clone(),
            dir: state_dir.to_owned(),
            persist_failed: AtomicBool::new(false),
            generation: Arc::new(AtomicU64::new(0)),
        }));
        let sink = Sink::new(KillFence(
            kill.0.latched.clone(),
            kill.0.generation.clone(),
            0,
        ));
        backend.bind(sink.clone());
        let mut speech = SpeechState::new(&backend.tool_allowlist());
        for (_, e) in store.entries() {
            if matches!(e.state, EntryState::Rejected { .. }) {
                continue;
            }
            // Restore closed stream/session tombstones so IDs stay unusable after restart.
            let msg = coglet_types::decode_typed::<Json>(
                &rule_key("surface:surface-message"),
                &e.envelope,
            )
            .map_err(|x| format!("internal: ledger envelope undecodable: {x}"))?;
            let a = &msg["body"]["action"];
            speech.restore_action(st(a, "coglet"), st(a, "capability"), &a["args"]);
        }
        if store.latched() || !store.recovered().is_empty() {
            // Uncertain physical work from a crash is stopped, never replayed.
            backend.stop_all();
        }
        let session_mode = backend.speech_config().and_then(|j| {
            j.get("session_mode")
                .and_then(Json::as_str)
                .map(str::to_owned)
        });
        let me = Surface {
            operator_grants: cfg.grants.clone(),
            now: cfg.start_clock,
            last_rx: cfg.start_clock,
            last_health: cfg.start_clock,
            hello_corr: String::new(),
            cfg,
            backend,
            store,
            sink,
            kill,
            kill_applied: false,
            cat,
            adv,
            restrictions: Vec::new(),
            speech,
            phase: Phase::Idle,
            logical: 0,
            grants: Snapshot {
                epoch: 0,
                entries: Vec::new(),
            },
            pending: BTreeMap::new(),
            diag: Diagnostics::default(),
            session_mode,
        };
        // Fail early on configs that cannot yield a valid snapshot after restriction.
        let (eff, _) = me.restrict(&me.operator_grants.clone(), None);
        check_grants(&eff, None, &me.cfg.bound_coglets).map_err(|e| e.message)?;
        Ok(me)
    }

    pub fn kill_switch(&self) -> KillSwitch {
        self.kill.clone()
    }
    pub fn advertisement(&self) -> &Advertisement {
        &self.adv
    }
    /// Grant restrictions applied because the backend lacks the operation/resource.
    pub fn restrictions(&self) -> &[String] {
        &self.restrictions
    }
    pub fn grant_epoch(&self) -> u64 {
        self.grants.epoch
    }
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }
    pub fn diagnostics(&self) -> Diagnostics {
        let mut d = self.diag.clone();
        d.native_discarded += self.speech.discarded;
        d
    }
    pub fn is_negotiated(&self) -> bool {
        self.phase == Phase::Active
    }

    /// Explicit local operator reset of the kill latch. Only a disconnected surface may be
    /// reset; resuming needs a fresh connection, negotiation and a higher grant epoch.
    pub fn reset_kill(&mut self) -> Result<(), String> {
        if matches!(self.phase, Phase::Active | Phase::AwaitReply) {
            return Err("conflict: disconnect before resetting the kill latch".into());
        }
        self.check_persistence()?;
        self.store.clear_latch().map_err(serr)?;
        self.kill.0.persist_failed.store(false, Ordering::SeqCst);
        self.kill.0.generation.fetch_add(1, Ordering::SeqCst);
        self.kill.0.latched.store(false, Ordering::SeqCst);
        self.kill_applied = false;
        Ok(())
    }

    /// Fire the kill switch and flush its wire notification now.
    pub fn local_kill(&mut self, now: u64, reason: &str) -> Result<Vec<Vec<u8>>, String> {
        self.kill.trigger(reason);
        self.tick(now)
    }

    /// Trusted operator / hub-policy path (not a wire message): replace the full grant
    /// snapshot. Entries are intersected with what the backend implements. The new epoch is
    /// persisted before it is published; owners that lose authority are fenced first.
    pub fn install_grants(
        &mut self,
        now: u64,
        specs: Vec<GrantSpec>,
    ) -> Result<Vec<Vec<u8>>, ApiError> {
        self.check_clock(now).map_err(ApiError::internal)?;
        check_grants(&specs, Some(now), &self.cfg.bound_coglets)?;
        let mut out = Vec::new();
        self.operator_grants = specs;
        if self.phase == Phase::Active {
            let (eff, _) = self.restrict(&self.operator_grants.clone(), Some(now));
            let effective = if self.kill.is_latched() {
                Vec::new()
            } else {
                eff
            };
            self.replace_snapshot(now, effective, &mut out)
                .map_err(ApiError::internal)?;
        }
        out.into_iter()
            .map(|m| self.enc(&m).map_err(ApiError::internal))
            .collect()
    }

    /// Operator revocation of one coglet (stops its native I/O, cancels its work, bumps epoch).
    pub fn revoke(&mut self, now: u64, coglet: &str) -> Result<Vec<Vec<u8>>, String> {
        self.check_clock(now)?;
        self.operator_grants.retain(|g| g.coglet != coglet);
        let mut out = Vec::new();
        if self.phase == Phase::Active && self.grants.entries.iter().any(|g| g.coglet == coglet) {
            let entries: Vec<GrantSpec> = self
                .grants
                .entries
                .iter()
                .filter(|g| g.coglet != coglet)
                .cloned()
                .collect();
            self.replace_snapshot(now, entries, &mut out)?;
        }
        self.encode_all(out)
    }

    // ------------------------------------------------------------ helpers

    fn check_persistence(&self) -> Result<(), String> {
        if self.kill.0.persist_failed.load(Ordering::SeqCst) {
            Err("unavailable: kill latch persistence failed; stopped, repair storage and reopen before operator reset".into())
        } else {
            Ok(())
        }
    }

    fn check_clock(&mut self, now: u64) -> Result<(), String> {
        self.check_persistence()?;
        if now < self.now {
            return Err("invalid_argument: clock rollback".into());
        }
        self.now = now;
        Ok(())
    }

    fn enc(&self, msg: &Json) -> Result<Vec<u8>, String> {
        let bytes = crate::codec::encode(&rule_key("surface:surface-message"), msg)
            .map_err(|e| format!("internal: outbound message invalid: {e}"))?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME {
            return Err("resource_exhausted: outbound frame exceeds 1 MiB".into());
        }
        Ok(bytes)
    }

    fn encode_all(&self, msgs: Vec<Json>) -> Result<Vec<Vec<u8>>, String> {
        msgs.iter().map(|m| self.enc(m)).collect()
    }

    fn fresh(&self) -> Result<String, String> {
        random_id()
    }

    fn retain_until(&self, deadline: u64, now: u64) -> u64 {
        self.store
            .wall_now()
            .saturating_add(RETENTION_US)
            .saturating_add(
                deadline
                    .saturating_sub(now)
                    .min(self.cfg.limits.max_deadline_ahead_us),
            )
    }

    /// Intersect operator grants with the backend's advertised operations/resources.
    fn restrict(&self, specs: &[GrantSpec], now: Option<u64>) -> (Vec<GrantSpec>, Vec<String>) {
        let mut notes = Vec::new();
        let mut out = Vec::new();
        for g in specs {
            if let Some(now) = now
                && g.expires <= now
            {
                notes.push(format!("{}: grant expired", g.coglet));
                continue;
            }
            let operations: Vec<String> = g
                .operations
                .iter()
                .filter(|o| self.adv.operations.contains(*o))
                .cloned()
                .collect();
            for o in g
                .operations
                .iter()
                .filter(|o| !self.adv.operations.contains(*o))
            {
                notes.push(format!("{}: {o} not advertised, grant withheld", g.coglet));
            }
            let resources: Vec<String> = g
                .resources
                .iter()
                .filter(|r| self.adv.resources.contains(*r))
                .cloned()
                .collect();
            if operations.is_empty() {
                continue;
            }
            out.push(GrantSpec {
                coglet: g.coglet.clone(),
                operations,
                resources,
                expires: g.expires,
            });
        }
        (out, notes)
    }

    fn event(&mut self, now: u64, kind: &str, payload: Json, corr: &str) -> Result<Json, String> {
        let seq = self.store.next_seq().map_err(serr)?;
        self.logical = self.logical.max(now);
        Ok(json!({"type": "event", "body": {
            "source": self.cfg.surface, "kind": kind, "seq": seq,
            "logical_time": self.logical, "payload": payload, "correlation_id": corr}}))
    }

    fn accept_msg(id: &str, corr: &str, err: Option<&ApiError>) -> Json {
        match err {
            None => {
                json!({"type": "accept", "body": {"id": id, "correlation_id": corr, "accepted": true}})
            }
            Some(e) => json!({"type": "accept", "body": {
                "id": id, "correlation_id": corr, "accepted": false, "error": e.to_json()}}),
        }
    }

    fn health_msg(&self, now: u64) -> Result<Json, String> {
        let stopped = self.kill.is_latched();
        let state = if stopped {
            "stopped"
        } else if self.backend.health() == BackendHealth::Degraded {
            "degraded"
        } else {
            "ready"
        };
        let pending = if stopped {
            0
        } else {
            self.pending.len() as u64
        };
        Ok(json!({"type": "health", "body": {
            "state": state, "clock": now, "pending": pending, "correlation_id": self.fresh()?}}))
    }

    // ------------------------------------------------------------ grants

    /// Replace the effective snapshot with `entries`: fence every coglet that lost
    /// authority first (native stop, cancel admitted work), then persist and publish the
    /// next epoch.
    fn replace_snapshot(
        &mut self,
        now: u64,
        entries: Vec<GrantSpec>,
        out: &mut Vec<Json>,
    ) -> Result<(), String> {
        let old = std::mem::take(&mut self.grants.entries);
        for g in &old {
            let keep = entries.iter().find(|n| n.coglet == g.coglet);
            let lost = keep.is_none_or(|n| {
                g.operations.iter().any(|o| !n.operations.contains(o))
                    || g.resources.iter().any(|r| !n.resources.contains(r))
            });
            if lost {
                self.fence_owner(now, &g.coglet, out)?;
            }
        }
        let epoch = self.grants.epoch + 1;
        self.store.set_epoch(epoch).map_err(serr)?;
        self.grants = Snapshot { epoch, entries };
        self.refresh_sink();
        if self.phase == Phase::Active {
            let msg = self.grants_msg()?;
            out.push(msg);
        }
        Ok(())
    }

    fn refresh_sink(&mut self) {
        let owner = self
            .grants
            .entries
            .iter()
            .find(|g| g.resources.iter().any(|r| r == "audio.mic"))
            .map(|g| g.coglet.clone());
        self.sink = self.sink.renew(owner);
        self.backend.bind(self.sink.clone());
    }

    fn grants_msg(&self) -> Result<Json, String> {
        let entries: Vec<Json> = self.grants.entries.iter().map(GrantSpec::to_json).collect();
        Ok(json!({"type": "grants", "body": {
            "epoch": self.grants.epoch, "entries": entries, "correlation_id": self.fresh()?}}))
    }

    /// Stop `coglet`'s native I/O and cancel everything it has admitted.
    fn fence_owner(&mut self, now: u64, coglet: &str, out: &mut Vec<Json>) -> Result<(), String> {
        self.backend.revoke(coglet);
        for id in self.speech.release_owner(Some(coglet)) {
            self.backend.cancel(&id);
            self.finish(
                now,
                &id,
                "cancelled",
                Some(ApiError::new(Code::Cancelled, "grant revoked")),
                out,
            )?;
        }
        let ids: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| p.coglet == coglet)
            .map(|(k, _)| k.clone())
            .collect();
        for id in ids {
            self.backend.cancel(&id);
            self.finish(
                now,
                &id,
                "cancelled",
                Some(ApiError::new(Code::Cancelled, "grant revoked")),
                out,
            )?;
        }
        Ok(())
    }

    // ------------------------------------------------------------ outcomes

    fn finish(
        &mut self,
        now: u64,
        id: &str,
        status: &str,
        err: Option<ApiError>,
        out: &mut Vec<Json>,
    ) -> Result<(), String> {
        let mut work = VecDeque::from([(id.to_owned(), status.to_owned(), err)]);
        while let Some((id, status, err)) = work.pop_front() {
            let Some(p) = self.pending.remove(&id) else {
                self.diag.late_completions += 1;
                continue;
            };
            let ok = status == "succeeded";
            let outcome = if ok {
                json!({"id": p.wire_id, "correlation_id": p.corr, "status": status, "result": {}})
            } else {
                let e = err.unwrap_or_else(|| ApiError::internal("action failed"));
                json!({"id": p.wire_id, "correlation_id": p.corr, "status": status, "error": e.to_json()})
            };
            if ok {
                self.cat
                    .validate_output(&p.op, &json!({}))
                    .map_err(|e| format!("internal: {e}"))?;
            }
            let retain = self.retain_until(p.deadline, now);
            self.store
                .record_terminal(&p.coglet, &p.wire_id, outcome.clone(), retain)
                .map_err(serr)?;
            for later in self.speech.terminal(&p.op, &p.args, &id, ok) {
                self.backend.cancel(&later);
                work.push_back((
                    later,
                    "cancelled".into(),
                    Some(ApiError::new(
                        Code::Cancelled,
                        "earlier chunk of the stream ended",
                    )),
                ));
            }
            if self.phase == Phase::Active {
                let ev = self.event(now, "action.outcome", outcome, &p.corr)?;
                out.push(ev);
            }
        }
        Ok(())
    }

    fn fail_status(e: &ApiError) -> &'static str {
        match e.code {
            Code::Cancelled => "cancelled",
            Code::DeadlineExceeded => "expired",
            _ => "failed",
        }
    }

    // ------------------------------------------------------------ admission

    #[allow(clippy::too_many_arguments)]
    fn admission_checks(
        &self,
        now: u64,
        epoch: u64,
        coglet: &str,
        op: &str,
        args: &Json,
        deadline: u64,
    ) -> Result<(), ApiError> {
        if self.kill.is_latched() {
            return Err(ApiError::denied("kill latch engaged"));
        }
        if epoch != self.grants.epoch {
            return Err(ApiError::denied("stale or unknown grant epoch"));
        }
        let grant = self
            .grants
            .entries
            .iter()
            .find(|g| g.coglet == coglet && g.expires > now && g.operations.iter().any(|o| o == op))
            .ok_or_else(|| ApiError::denied("operation not granted to coglet"))?;
        match self.cat.op(op) {
            Some(o) if o.direction == Direction::Action => {}
            _ => return Err(ApiError::invalid("operation is not an action")),
        }
        if let Some(r) = required_resources(op)
            .iter()
            .find(|r| !grant.resources.iter().any(|x| x == *r))
        {
            return Err(ApiError::denied(format!(
                "resource {r} not owned by coglet"
            )));
        }
        self.cat
            .validate_input(op, args)
            .map_err(|e| ApiError::invalid(format!("arguments rejected: {e}")))?;
        if now >= deadline {
            return Err(ApiError::new(
                Code::DeadlineExceeded,
                "deadline already reached",
            ));
        }
        if deadline - now > self.cfg.limits.max_deadline_ahead_us {
            return Err(ApiError::invalid("deadline more than 60 s ahead"));
        }
        if self.pending.len() >= self.cfg.limits.max_pending {
            return Err(ApiError::exhausted("pending action queue full"));
        }
        self.speech.check(
            op,
            coglet,
            args,
            self.session_mode.as_deref(),
            &self.backend.session_routes(),
        )?;
        Ok(())
    }

    fn handle_act(
        &mut self,
        raw: &[u8],
        body: &Json,
        now: u64,
        out: &mut Vec<Json>,
    ) -> Result<(), String> {
        let epoch = un(body, "grant_epoch");
        let a = &body["action"];
        let (coglet, id, corr, op) = (
            st(a, "coglet"),
            st(a, "id"),
            st(a, "correlation_id"),
            st(a, "capability"),
        );
        let (args, deadline) = (&a["args"], un(a, "deadline"));
        if !self.cfg.bound_coglets.iter().any(|c| c == coglet) {
            out.push(Self::accept_msg(
                id,
                corr,
                Some(&ApiError::denied("coglet is not bound to this surface")),
            ));
            return Ok(());
        }
        // Retry lookup precedes fresh admission and expiry checks.
        if let Some(e) = self.store.lookup(coglet, id) {
            if e.envelope != raw {
                out.push(Self::accept_msg(
                    id,
                    corr,
                    Some(&ApiError::conflict("conflicting reuse of action id")),
                ));
                return Ok(());
            }
            let (ecorr, state) = (e.corr.clone(), e.state.clone());
            match state {
                EntryState::Rejected { error, .. } => {
                    out.push(json!({"type": "accept", "body": {
                        "id": id, "correlation_id": ecorr, "accepted": false, "error": error}}));
                }
                EntryState::Pending => out.push(Self::accept_msg(id, &ecorr, None)),
                EntryState::Terminal { outcome, .. } => {
                    out.push(Self::accept_msg(id, &ecorr, None));
                    let ev = self.event(now, "action.outcome", outcome, &ecorr)?;
                    out.push(ev);
                }
            }
            return Ok(());
        }
        if self.store.len() >= self.cfg.limits.max_ledger {
            out.push(Self::accept_msg(
                id,
                corr,
                Some(&ApiError::exhausted("dedup ledger full")),
            ));
            return Ok(());
        }
        let token = self.fresh()?;
        let req = ActionRequest {
            id: token.clone(),
            wire_id: id.to_owned(),
            kill_fence: KillFence(
                self.kill.0.latched.clone(),
                self.kill.0.generation.clone(),
                self.kill.0.generation.load(Ordering::SeqCst),
            ),
            coglet: coglet.to_owned(),
            op: op.to_owned(),
            args: args.clone(),
            deadline,
            grant_epoch: epoch,
        };
        let admission = self
            .admission_checks(now, epoch, coglet, op, args, deadline)
            .and_then(|()| self.backend.preflight(&req, now))
            .and_then(|()| req.kill_fence.check());
        if let Err(e) = admission {
            let retain = self.retain_until(deadline, now);
            self.store
                .record_rejection(coglet, id, corr, raw, epoch, deadline, e.to_json(), retain)
                .map_err(serr)?;
            out.push(Self::accept_msg(id, corr, Some(&e)));
            return Ok(());
        }
        self.store
            .record_admission(coglet, id, corr, raw, epoch, deadline)
            .map_err(serr)?;
        let cancelled = self.speech.admit_bound(op, coglet, &token, id, args, now);
        self.pending.insert(
            token.clone(),
            Pending {
                wire_id: id.to_owned(),
                coglet: coglet.into(),
                op: op.into(),
                args: args.clone(),
                corr: corr.into(),
                deadline,
            },
        );
        out.push(Self::accept_msg(id, corr, None));
        for c in cancelled {
            self.backend.cancel(&c);
            self.finish(
                now,
                &c,
                "cancelled",
                Some(ApiError::new(Code::Cancelled, "stream stopped")),
                out,
            )?;
        }
        if let Err(e) = req
            .kill_fence
            .check()
            .and_then(|()| self.backend.execute(&req, now))
        {
            let status = Self::fail_status(&e);
            self.finish(now, &token, status, Some(e), out)?;
        }
        Ok(())
    }

    fn handle_read(&mut self, body: &Json, now: u64, out: &mut Vec<Json>) -> Result<(), String> {
        let corr = st(body, "correlation_id");
        let reply = |e: &ApiError| {
            json!({"type": "read_reply", "body": {
            "correlation_id": corr, "ok": false, "error": e.to_json()}})
        };
        let (coglet, op, args) = (st(body, "coglet"), st(body, "capability"), &body["args"]);
        let result = (|| -> Result<crate::backend::ReadSnapshot, ApiError> {
            if !self.cfg.bound_coglets.iter().any(|c| c == coglet) || self.kill.is_latched() {
                return Err(ApiError::denied("not permitted"));
            }
            if un(body, "grant_epoch") != self.grants.epoch {
                return Err(ApiError::denied("stale or unknown grant epoch"));
            }
            if !self.grants.entries.iter().any(|g| {
                g.coglet == coglet && g.expires > now && g.operations.iter().any(|o| o == op)
            }) {
                return Err(ApiError::denied("operation not granted to coglet"));
            }
            match self.cat.op(op) {
                Some(o) if o.direction == Direction::Read => {}
                _ => return Err(ApiError::invalid("operation is not a read")),
            }
            self.cat
                .validate_input(op, args)
                .map_err(|e| ApiError::invalid(format!("arguments rejected: {e}")))?;
            let deadline = un(body, "deadline");
            if now >= deadline {
                return Err(ApiError::new(
                    Code::DeadlineExceeded,
                    "deadline already reached",
                ));
            }
            if deadline - now > self.cfg.limits.max_deadline_ahead_us {
                return Err(ApiError::invalid("deadline more than 60 s ahead"));
            }
            let snap = self.backend.read(op, args, now)?;
            self.cat
                .validate_output(op, &snap.value)
                .map_err(|e| ApiError::internal(format!("backend read violates schema: {e}")))?;
            Ok(snap)
        })();
        out.push(match result {
            Ok(s) => json!({"type": "read_reply", "body": {
                "correlation_id": corr, "ok": true, "value": s.value, "snapshot": s.snapshot}}),
            Err(e) => reply(&e),
        });
        Ok(())
    }

    // ------------------------------------------------------------ native events

    fn deliver(
        &mut self,
        now: u64,
        kind: &str,
        payload: Json,
        owner: Option<&str>,
        out: &mut Vec<Json>,
    ) -> Result<(), String> {
        let need = required_resources(kind);
        let has_recipient = self.phase == Phase::Active
            && !self.kill.is_latched()
            && self.grants.entries.iter().any(|g| {
                owner.is_none_or(|o| o == g.coglet)
                    && g.expires > now
                    && g.operations.iter().any(|o| o == kind)
                    && need.iter().all(|r| g.resources.iter().any(|x| x == r))
            });
        if !has_recipient || self.cat.validate_input(kind, &payload).is_err() {
            self.diag.events_suppressed += 1;
            return Ok(());
        }
        let corr = self.fresh()?;
        let ev = self.event(now, kind, payload, &corr)?;
        out.push(ev);
        Ok(())
    }

    fn handle_native(&mut self, now: u64, n: Native, out: &mut Vec<Json>) -> Result<(), String> {
        match self.speech.normalize(n) {
            Normalized::Event {
                kind,
                payload,
                owner,
            } => {
                let barge_stop = kind == "speech.barge_in" && payload["reflex"] == "stopped";
                let stream = st(&payload, "stream").to_owned();
                self.deliver(now, kind, payload, owner.as_deref(), out)?;
                if barge_stop {
                    for id in self.speech.reflex_stopped(&stream) {
                        self.backend.cancel(&id);
                        self.finish(
                            now,
                            &id,
                            "cancelled",
                            Some(ApiError::new(Code::Cancelled, "barge-in stopped playback")),
                            out,
                        )?;
                    }
                }
            }
            Normalized::StreamStopped { cancel, .. } => {
                for id in cancel {
                    self.backend.cancel(&id);
                    self.finish(
                        now,
                        &id,
                        "cancelled",
                        Some(ApiError::new(Code::Cancelled, "stopped")),
                        out,
                    )?;
                }
            }
            Normalized::Discarded(_) => {}
        }
        Ok(())
    }

    // ------------------------------------------------------------ service loop

    fn apply_kill(&mut self, now: u64, out: &mut Vec<Json>) -> Result<(), String> {
        self.kill_applied = true;
        for id in self.speech.release_owner(None) {
            self.backend.cancel(&id);
        }
        let reason = self.kill.reason();
        self.grants.entries.clear();
        self.refresh_sink();
        if self.phase == Phase::Active {
            out.push(json!({"type": "kill", "body": {"reason": reason, "correlation_id": self.fresh()?}}));
            out.push(self.health_msg(now)?);
            self.last_health = now;
        }
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        for id in ids {
            self.backend.cancel(&id);
            self.finish(
                now,
                &id,
                "cancelled",
                Some(ApiError::new(Code::Cancelled, "kill switch")),
                out,
            )?;
        }
        Ok(())
    }

    fn service(&mut self, now: u64, out: &mut Vec<Json>) -> Result<(), String> {
        self.check_persistence()?;
        if let Err(e) = self.store.prune_and_compact() {
            self.kill.trigger("receipt compaction failed");
            return Err(serr(e));
        }
        if self.kill.is_latched() && !self.kill_applied {
            self.apply_kill(now, out)?;
        }
        let (items, overflow) = self.sink.drain();
        if overflow {
            self.backend.stop_all();
            return Err("resource_exhausted: native event queue overflow".into());
        }
        for item in items {
            match item {
                SinkItem::Complete { id, result } => {
                    let Some(p) = self.pending.get(&id) else {
                        self.diag.late_completions += 1;
                        continue;
                    };
                    if !self.speech.chunk_is_head(&p.op, &p.args, &id) {
                        // Audio cannot have played out of order; fail the stream closed.
                        self.diag.unsound_completions += 1;
                        self.backend.cancel(&id);
                        self.finish(
                            now,
                            &id,
                            "failed",
                            Some(ApiError::internal("backend completed chunk out of order")),
                            out,
                        )?;
                        continue;
                    }
                    match result {
                        Ok(()) => self.finish(now, &id, "succeeded", None, out)?,
                        Err(e) => {
                            let status = Self::fail_status(&e);
                            self.finish(now, &id, status, Some(e), out)?;
                        }
                    }
                }
                SinkItem::Native {
                    observation,
                    owner,
                    session,
                } => {
                    let authorized = owner.as_ref().is_some_and(|o| {
                        self.grants
                            .entries
                            .iter()
                            .any(|g| &g.coglet == o && g.expires > now)
                    });
                    let session_ok = match &observation {
                        Native::ToolCall { session: sid, .. }
                        | Native::SessionEnded { session: sid } => {
                            session.as_deref() == Some(sid.as_str())
                        }
                        _ => true,
                    };
                    if !self.kill.is_latched() && authorized && session_ok {
                        if self.speech.captured_native_allowed(
                            &observation,
                            owner.as_deref(),
                            session.as_deref(),
                        ) {
                            self.handle_native(now, observation, out)?;
                        } else {
                            self.diag.native_discarded += 1;
                        }
                    } else {
                        self.diag.native_discarded += 1;
                    }
                }
            }
        }
        // Deadline overrun expires the action and stops its physical effect.
        let due: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, p)| now >= p.deadline)
            .map(|(k, _)| k.clone())
            .collect();
        for id in due {
            self.backend.cancel(&id);
            self.finish(
                now,
                &id,
                "expired",
                Some(ApiError::new(Code::DeadlineExceeded, "deadline reached")),
                out,
            )?;
        }
        if self.phase == Phase::Active {
            let expired: Vec<String> = self
                .grants
                .entries
                .iter()
                .filter(|g| g.expires <= now)
                .map(|g| g.coglet.clone())
                .collect();
            if !expired.is_empty() {
                let entries = self
                    .grants
                    .entries
                    .iter()
                    .filter(|g| g.expires > now)
                    .cloned()
                    .collect();
                self.replace_snapshot(now, entries, out)?;
            }
        }
        for stream in self.speech.close_idle(now, self.cfg.limits.stream_idle_us) {
            let _ = stream; // unfinalized and idle: no admitted chunk can still be pending
        }
        self.backend.tick(now);
        if self.phase == Phase::Active
            && now.saturating_sub(self.last_health) >= self.cfg.limits.health_interval_us
        {
            out.push(self.health_msg(now)?);
            self.last_health = now;
        }
        Ok(())
    }

    fn silence_check(&self, now: u64) -> Result<(), String> {
        if matches!(self.phase, Phase::AwaitReply | Phase::Active)
            && now.saturating_sub(self.last_rx) >= self.cfg.limits.silence_timeout_us
        {
            return Err("unavailable: no valid message for 3 s".into());
        }
        Ok(())
    }

    fn negotiate(&mut self, now: u64, out: &mut Vec<Json>) -> Result<(), String> {
        self.phase = Phase::Active;
        self.kill_applied = false;
        self.last_health = now;
        out.push(json!({"type": "capabilities", "body": {
            "revision": 1, "clock": now, "entries": self.adv.entries, "correlation_id": self.fresh()?}}));
        let epoch = self
            .cfg
            .initial_grant_epoch
            .max(self.store.grant_epoch() + 1);
        self.store.set_epoch(epoch).map_err(serr)?;
        let (eff, notes) = self.restrict(&self.operator_grants.clone(), Some(now));
        self.restrictions = notes;
        let entries = if self.kill.is_latched() {
            Vec::new()
        } else {
            eff
        };
        self.grants = Snapshot { epoch, entries };
        self.refresh_sink();
        out.push(self.grants_msg()?);
        Ok(())
    }
}

impl ControlPeer for Surface {
    fn start(&mut self) -> Result<Vec<Vec<u8>>, String> {
        if matches!(self.phase, Phase::AwaitReply | Phase::Active) {
            return Err("conflict: connection already started".into());
        }
        self.check_persistence()?;
        if let Err(e) = self.store.mark_running() {
            self.kill.trigger("cannot persist running marker");
            return Err(serr(e));
        }
        self.phase = Phase::AwaitReply;
        self.last_rx = self.now;
        self.grants.entries.clear();
        self.refresh_sink();
        self.hello_corr = match &self.cfg.hello_correlation {
            Some(c) => c.clone(),
            None => self.fresh()?,
        };
        let hello = json!({"type": "hello", "body": {
            "surface": self.cfg.surface, "offer": {"versions": self.cfg.versions},
            "clock": self.now, "correlation_id": self.hello_corr}});
        Ok(vec![self.enc(&hello)?])
    }

    fn receive(&mut self, bytes: &[u8], now: u64) -> Result<Vec<Vec<u8>>, String> {
        self.check_clock(now)?;
        self.silence_check(now)?;
        if bytes.is_empty() || bytes.len() > MAX_FRAME {
            return Err("invalid_argument: frame size".into());
        }
        let msg: Json = coglet_types::decode_typed(&rule_key("surface:surface-message"), bytes)
            .map_err(|e| format!("invalid_argument: {e}"))?;
        let (ty, body) = (st(&msg, "type"), &msg["body"]);
        let mut out = Vec::new();
        match (self.phase, ty) {
            (Phase::AwaitReply, "hello_reply") => {
                if st(body, "correlation_id") != self.hello_corr {
                    return Err("invalid_argument: hello_reply correlation mismatch".into());
                }
                let v = st(&body["selection"], "version");
                if !self.cfg.versions.iter().any(|x| x == v) {
                    return Err("unsupported_version: selection not in offer".into());
                }
                self.negotiate(now, &mut out)?;
            }
            (Phase::AwaitReply, "error") | (Phase::Active, "error") => {
                return Err(format!(
                    "unavailable: peer error {}",
                    st(&body["error"], "code")
                ));
            }
            (Phase::Active, "act") => self.handle_act(bytes, body, now, &mut out)?,
            (Phase::Active, "read") => self.handle_read(body, now, &mut out)?,
            (Phase::Active, "health") => {}
            (
                _,
                "hello" | "capabilities" | "grants" | "event" | "read_reply" | "accept" | "kill",
            ) => {
                return Err(format!(
                    "invalid_argument: {ty} is not valid runtime to surface"
                ));
            }
            _ => {
                return Err(format!(
                    "invalid_argument: {ty} invalid in current lifecycle state"
                ));
            }
        }
        self.last_rx = now;
        self.service(now, &mut out)?;
        self.encode_all(out)
    }

    fn tick(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String> {
        self.check_clock(now)?;
        self.silence_check(now)?;
        let mut out = Vec::new();
        self.service(now, &mut out)?;
        self.encode_all(out)
    }

    fn disconnect(&mut self, now: u64) -> Result<(), String> {
        if self.phase == Phase::Closed {
            return Ok(());
        }
        self.now = self.now.max(now);
        let was_active = self.phase == Phase::Active;
        self.phase = Phase::Closed;
        let mut first_err = None;
        // Stop capture/output for everyone, fence provider sessions, then reconcile.
        for g in std::mem::take(&mut self.grants.entries) {
            self.backend.revoke(&g.coglet);
        }
        if was_active || !self.pending.is_empty() {
            self.backend.stop_all();
        }
        for id in self.speech.release_owner(None) {
            self.backend.cancel(&id);
        }
        let ids: Vec<String> = self.pending.keys().cloned().collect();
        let mut sink_out = Vec::new();
        for id in ids {
            self.backend.cancel(&id);
            if let Err(e) = self.finish(
                self.now,
                &id,
                "cancelled",
                Some(ApiError::new(Code::Cancelled, "connection lost")),
                &mut sink_out,
            ) && first_err.is_none()
            {
                first_err = Some(e);
            }
        }
        let _ = self.sink.drain();
        if first_err.is_none()
            && !self.kill.is_latched()
            && !self.kill.0.persist_failed.load(Ordering::SeqCst)
        {
            self.store.mark_clean().map_err(serr)?;
        }
        first_err.map_or(Ok(()), Err)
    }
}
