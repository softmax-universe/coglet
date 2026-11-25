//! Fast loop: low-latency control with asynchronous planning (guest SDK, ABI 0.1).
//!
//! A [`FastLoop`] splits a controller into two paths that never wait on each other:
//!
//! * **Fast path, every observation.** [`Controller::decide`] runs locally, charged
//!   against a deterministic per-tick work [`Budget`]. If it exhausts the budget the
//!   loop uses the controller's O(1) [`Controller::fallback`] instead. No LLM call,
//!   inbox read or other blocking work happens on this path.
//! * **Slow path, asynchronous.** When the controller wants a plan and none is pending,
//!   the loop stages one `llm.complete` request and keeps ticking. The reply arrives in
//!   a later step through the inbox (`runtime.inbox` wake + `/data/inbox.jsonl`), is
//!   validated and parsed by [`Controller::parse_plan`], and is used from the next tick.
//!   Failures, refusals, malformed plans and late or foreign replies never stall the
//!   fast path; they only back off the next request.
//!
//! The budget counts abstract work units the controller charges itself, so it is
//! deterministic and replay-equal; it is not a wall-clock measurement. The runtime's own
//! manifest `compute` and `wall_time_us` budgets remain the hard bounds on a step.
//! All loop state lives in one file under `/data`, so replaying the recorded events
//! and inbox bytes reproduces every effect and every state byte.
use coglet_guest_rust::{Action, Context, Event, Host, HostError, LlmRequest};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

/// Largest inbox file the loop will read, in bytes.
pub const MAX_INBOX_BYTES: usize = 1024 * 1024;

/// The fast path ran out of work units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exhausted;

/// Deterministic per-tick work budget. Controllers call [`Budget::charge`] before each
/// bounded unit of work (a cell scanned, a candidate scored) and stop on `Err`.
#[derive(Debug, Clone)]
pub struct Budget {
    limit: u64,
    used: u64,
}
impl Budget {
    pub fn new(limit: u64) -> Self {
        Self { limit, used: 0 }
    }
    /// Charge `units`; refuses (and charges nothing) once the limit would be exceeded.
    pub fn charge(&mut self, units: u64) -> Result<(), Exhausted> {
        match self.used.checked_add(units) {
            Some(total) if total <= self.limit => {
                self.used = total;
                Ok(())
            }
            _ => Err(Exhausted),
        }
    }
    pub fn used(&self) -> u64 {
        self.used
    }
    pub fn limit(&self) -> u64 {
        self.limit
    }
}

/// One action the controller wants staged: capability name plus profile-map arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub capability: String,
    pub args: Map<String, Value>,
}

/// Product-specific policy plugged into the loop. All methods must be deterministic
/// functions of their arguments.
pub trait Controller {
    /// Slow-path output, kept in loop state. Must be small and bounded.
    type Plan: Serialize + DeserializeOwned + Clone;
    /// Decoded observation.
    type Obs;

    /// Event kind that is one control tick (for example `game.state`).
    fn observation_kind(&self) -> &str;
    /// Decode an observation payload. `None` counts as a bad observation and is skipped.
    fn parse_observation(&self, payload: &Value) -> Option<Self::Obs>;
    /// The episode is over: no action, no plan request.
    fn is_terminal(&self, obs: &Self::Obs) -> bool;
    /// Fast path. The plan is mutable so the controller can record progress (a cursor);
    /// that progress is persisted with the plan. Charge `budget` for every bounded unit of work; return `Err` as soon as
    /// a charge is refused. `Ok(None)` means "no action this tick".
    fn decide(
        &self,
        obs: &Self::Obs,
        plan: Option<&mut Self::Plan>,
        budget: &mut Budget,
    ) -> Result<Option<Command>, Exhausted>;
    /// Used when `decide` exhausts its budget. Must be O(1) and cannot be charged.
    fn fallback(&self, obs: &Self::Obs) -> Option<Command>;
    /// Whether a new plan should be requested now (called only with nothing pending).
    fn wants_plan(&self, obs: &Self::Obs, plan: Option<&Self::Plan>) -> bool;
    /// Slow-path prompt input for `llm.complete`. Must be bounded and O(1)-ish.
    fn plan_request(&self, obs: &Self::Obs) -> Map<String, Value>;
    /// Validate and decode a successful reply's `output`. `None` rejects it.
    fn parse_plan(&self, output: &Value) -> Option<Self::Plan>;
    /// The plan is spent (goal reached, last step consumed); the loop drops it.
    fn plan_finished(&self, obs: &Self::Obs, plan: &Self::Plan) -> bool;
}

/// Loop wiring and bounds, supplied by the product (not chosen by the guest).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// This coglet's ID (`coglet:` + 64 hex), stamped on every action.
    pub coglet: String,
    /// Authenticated sender of `llm.reply` inbox messages (the router/gateway ID).
    pub llm_sender: String,
    /// Routing model name for `llm.complete`.
    pub route: String,
    /// Per-tick work-unit limit for [`Controller::decide`].
    pub tick_budget: u64,
    /// Ticks after which an applied plan is dropped as stale.
    pub plan_ttl_ticks: u64,
    /// Ticks after which an unanswered request is abandoned; its late reply is ignored.
    pub plan_timeout_ticks: u64,
    /// Ticks to wait before requesting again after a failure, rejection or timeout.
    pub backoff_ticks: u64,
    /// Logical-time microseconds granted to each action.
    pub action_deadline_us: u64,
    /// State file under `/data`.
    pub state_path: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    pub ticks: u64,
    /// Ticks where `decide` exhausted its budget and `fallback` ran.
    pub degraded: u64,
    /// Largest work-unit charge of any tick.
    pub max_work: u64,
    pub actions: u64,
    pub plan_requests: u64,
    /// Requests the host refused (budget, grant, duplicate ID); they stage nothing.
    pub plan_refused: u64,
    pub plans_applied: u64,
    pub plans_rejected: u64,
    pub plan_failures: u64,
    pub plan_timeouts: u64,
    pub plans_expired: u64,
    /// Inbox messages that were not the awaited reply: foreign, duplicate, late, other kinds.
    pub ignored: u64,
    pub bad_observations: u64,
    /// Wakes whose inbox lines could not be read or validated (skipped, never fatal).
    pub inbox_faults: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pending {
    pub request_id: String,
    pub issued_tick: u64,
}

/// Everything the loop remembers between steps (one JSON file, declaration order).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "P: Serialize", deserialize = "P: DeserializeOwned"))]
pub struct State<P> {
    pub tick: u64,
    /// Next inbox index to read.
    pub inbox_cursor: u64,
    pub plan: Option<P>,
    pub plan_tick: u64,
    pub pending: Option<Pending>,
    /// No request before this tick (failure backoff).
    pub next_plan_tick: u64,
    pub done: bool,
    pub stats: Stats,
}
impl<P> Default for State<P> {
    fn default() -> Self {
        Self {
            tick: 0,
            inbox_cursor: 0,
            plan: None,
            plan_tick: 0,
            pending: None,
            next_plan_tick: 0,
            done: false,
            stats: Stats::default(),
        }
    }
}
impl<P: Serialize> State<P> {
    pub fn to_bytes(&self) -> Result<Vec<u8>, HostError> {
        serde_json::to_vec(self).map_err(|e| HostError::Encoding(e.to_string()))
    }
    /// SHA-256 of the serialized state, for replay comparison.
    pub fn digest(&self) -> Result<String, HostError> {
        let bytes = self.to_bytes()?;
        Ok(Sha256::digest(bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect())
    }
}

pub struct FastLoop<C: Controller> {
    pub controller: C,
    pub config: Config,
}

impl<C: Controller> FastLoop<C> {
    pub fn new(controller: C, config: Config) -> Self {
        Self { controller, config }
    }

    pub fn load<H: Host>(&self, ctx: &mut Context<'_, H>) -> Result<State<C::Plan>, HostError> {
        match ctx.read_file(&self.config.state_path)? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| HostError::InvalidArgument),
            None => Ok(State::default()),
        }
    }

    /// One runtime step: consume replies first (so a plan that arrived in this batch is
    /// usable immediately), then run one control tick per observation, in batch order.
    pub fn wake<H: Host>(
        &self,
        ctx: &mut Context<'_, H>,
        events: &[Event],
    ) -> Result<(), HostError> {
        let before = ctx.read_file(&self.config.state_path)?;
        let mut state = match &before {
            Some(bytes) => serde_json::from_slice(bytes).map_err(|_| HostError::InvalidArgument)?,
            None => State::default(),
        };
        let upto = events
            .iter()
            .filter(|e| e.kind == "runtime.inbox")
            .filter_map(|e| e.payload.get("index"))
            .filter_map(|v| serde_json::to_value(v).ok()?.as_u64())
            .max();
        if let Some(upto) = upto
            && upto >= state.inbox_cursor
            && self.consume_inbox(ctx, &mut state, upto).is_err()
        {
            // The slow path must never stop the fast path. An unreadable, oversized
            // or damaged inbox is counted, its lines up to the wake are skipped, and
            // any pending request simply times out. The runtime keeps the file.
            state.stats.inbox_faults += 1;
            state.inbox_cursor = upto.saturating_add(1);
        }
        for event in events
            .iter()
            .filter(|e| e.kind == self.controller.observation_kind())
        {
            self.tick(ctx, &mut state, event)?;
        }
        let after = state.to_bytes()?;
        if before.as_deref() != Some(after.as_slice()) {
            ctx.write_file(&self.config.state_path, &after)?;
        }
        Ok(())
    }

    fn consume_inbox<H: Host>(
        &self,
        ctx: &mut Context<'_, H>,
        state: &mut State<C::Plan>,
        upto: u64,
    ) -> Result<(), HostError> {
        let bytes = ctx
            .read_file("/data/inbox.jsonl")?
            .ok_or(HostError::InvalidArgument)?;
        if bytes.len() > MAX_INBOX_BYTES {
            return Err(HostError::ResourceExhausted);
        }
        // Canonical JSONL: LF-terminated lines, no CR, no blank lines.
        if !bytes.is_empty() && bytes.last() != Some(&b'\n') || bytes.contains(&b'\r') {
            return Err(HostError::InvalidArgument);
        }
        let text = std::str::from_utf8(&bytes).map_err(|_| HostError::InvalidArgument)?;
        // Every append produced a wake, so lines up to the highest wake index are all
        // wake-announced; earlier lines were validated when first consumed and are skipped.
        let mut staged = Vec::new();
        let mut next = 0u64;
        for line in text.split_terminator('\n') {
            let index = next;
            next += 1; // bounded by the 1 MiB file cap
            if index < state.inbox_cursor {
                continue;
            }
            if index > upto {
                break;
            }
            let row: Value = serde_json::from_str(line).map_err(|_| HostError::InvalidArgument)?;
            validate_row(&row, index)?;
            staged.push(row);
        }
        if next <= upto {
            return Err(HostError::InvalidArgument); // wake names a line that is not there
        }
        // Validated whole range first: a bad line leaves no partial application.
        for row in &staged {
            self.apply_row(state, row);
        }
        state.inbox_cursor = upto.saturating_add(1);
        Ok(())
    }

    fn apply_row(&self, state: &mut State<C::Plan>, row: &Value) {
        let event = &row["event"];
        let payload = &event["payload"];
        let awaited = state.pending.as_ref().map(|p| p.request_id.as_str());
        let is_reply = event["kind"] == "llm.reply"
            && row["sender"] == self.config.llm_sender.as_str()
            && event["source"] == row["sender"]
            && awaited.is_some()
            && payload["request_id"].as_str() == awaited;
        if !is_reply {
            state.stats.ignored += 1;
            return;
        }
        state.pending = None;
        if payload["status"] != "succeeded" {
            state.stats.plan_failures += 1;
            state.next_plan_tick = state
                .tick
                .saturating_add(1)
                .saturating_add(self.config.backoff_ticks);
            return;
        }
        match self.controller.parse_plan(&payload["output"]) {
            Some(plan) => {
                state.plan = Some(plan);
                state.plan_tick = state.tick;
                state.stats.plans_applied += 1;
            }
            None => {
                state.stats.plans_rejected += 1;
                state.next_plan_tick = state
                    .tick
                    .saturating_add(1)
                    .saturating_add(self.config.backoff_ticks);
            }
        }
    }

    fn tick<H: Host>(
        &self,
        ctx: &mut Context<'_, H>,
        state: &mut State<C::Plan>,
        event: &Event,
    ) -> Result<(), HostError> {
        let payload =
            serde_json::to_value(&event.payload).map_err(|_| HostError::InvalidArgument)?;
        let Some(obs) = self.controller.parse_observation(&payload) else {
            state.stats.bad_observations += 1;
            return Ok(());
        };
        if state.done {
            return Ok(());
        }
        if self.controller.is_terminal(&obs) {
            state.done = true;
            state.plan = None;
            return Ok(());
        }
        state.tick = state.tick.saturating_add(1);
        state.stats.ticks = state.stats.ticks.saturating_add(1);
        let cfg = &self.config;
        if state.plan.is_some() && state.tick.saturating_sub(state.plan_tick) > cfg.plan_ttl_ticks {
            state.plan = None;
            state.stats.plans_expired += 1;
        }
        if let Some(p) = &state.pending
            && state.tick.saturating_sub(p.issued_tick) > cfg.plan_timeout_ticks
        {
            state.pending = None;
            state.stats.plan_timeouts += 1;
            state.next_plan_tick = state.tick.saturating_add(cfg.backoff_ticks);
        }

        // Fast path: local, bounded, never waits.
        let mut budget = Budget::new(cfg.tick_budget);
        let command = match self
            .controller
            .decide(&obs, state.plan.as_mut(), &mut budget)
        {
            Ok(command) => command,
            Err(Exhausted) => {
                state.stats.degraded += 1;
                self.controller.fallback(&obs)
            }
        };
        state.stats.max_work = state.stats.max_work.max(budget.used());
        if let Some(command) = command {
            let deadline = ctx
                .logical_time
                .checked_add(cfg.action_deadline_us)
                .ok_or(HostError::InvalidArgument)?;
            let args = serde_json::from_value(Value::Object(command.args))
                .map_err(|_| HostError::InvalidArgument)?;
            let id = ctx.next_id()?;
            ctx.emit(&Action {
                id,
                coglet: cfg.coglet.clone(),
                capability: command.capability,
                args,
                deadline,
                correlation_id: event.correlation_id.clone(),
            })?;
            state.stats.actions += 1;
        }
        if state
            .plan
            .as_ref()
            .is_some_and(|plan| self.controller.plan_finished(&obs, plan))
        {
            state.plan = None;
        }

        // Slow path: stage at most one request; the reply arrives in a later step.
        if state.pending.is_none()
            && state.tick >= state.next_plan_tick
            && self.controller.wants_plan(&obs, state.plan.as_ref())
        {
            let request_id = ctx.next_id()?;
            let correlation_id = ctx.next_id()?;
            let input = self.controller.plan_request(&obs);
            let request = LlmRequest {
                id: request_id.clone(),
                correlation_id,
                capability: "llm.complete".into(),
                model: cfg.route.clone(),
                input: serde_json::from_value(json!(input))
                    .map_err(|_| HostError::InvalidArgument)?,
            };
            // A refused request stages nothing; it must not cost the tick its action.
            if ctx.llm(&request).is_err() {
                state.stats.plan_refused += 1;
                state.next_plan_tick = state.tick.saturating_add(cfg.backoff_ticks);
                return Ok(());
            }
            state.pending = Some(Pending {
                request_id,
                issued_tick: state.tick,
            });
            state.stats.plan_requests += 1;
        }
        Ok(())
    }
}

/// Schema-validate one inbox line (ABI inbox section): the message, its embedded event
/// and, for `llm.reply`, the reply payload. Also binds index and sender to the event. `id` is the runtime's own message ID and is
/// deliberately not tied to the event correlation_id (the ABI does not require equality).
fn validate_row(row: &Value, index: u64) -> Result<(), HostError> {
    let bad = |_: String| HostError::InvalidArgument;
    let cbor = |v: &Value| coglet_types::Value::serialized(v).map_err(|e| bad(e.to_string()));
    coglet_types::validate("abi::inbox-message", &cbor(row)?).map_err(bad)?;
    let event = &row["event"];
    if row["index"].as_u64() != Some(index) || row["sender"] != event["source"] {
        return Err(HostError::InvalidArgument);
    }
    if event["kind"] == "llm.reply" {
        coglet_types::validate("abi::llm-reply", &cbor(&event["payload"])?).map_err(bad)?;
    }
    Ok(())
}
