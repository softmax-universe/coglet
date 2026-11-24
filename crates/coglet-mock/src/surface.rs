//! Real SDK core with an optional bounded, absolute-microsecond native timeline.
//!
//! The default fixture uses SemanticBackend directly. Supplying `mock_timeline`
//! selects ScriptedBackend: execute records admission, while `complete` actually
//! applies the recorded semantic effect and reports its result. No expected wire
//! replies or conformance-case names are interpreted here.
use coglet_surface_rust::backend::{ActionRequest, Backend, Native, ReadSnapshot, Sink};
use coglet_surface_rust::config::{AdapterFixture, Config, ExpressionState, SpeechConfig};
use coglet_surface_rust::error::ApiError;
use coglet_surface_rust::reference::SemanticBackend;
use coglet_surface_rust::surface::Surface;
use coglet_surface_rust::transport::ControlPeer;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;
use std::sync::{Arc, Mutex};

pub const MAX_TIMELINE_EVENTS: usize = 4096;
pub const MAX_TIMELINE_US: u64 = 60_000_000;
pub const MAX_CONFIG_BYTES: usize = 1_048_576;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Timeline {
    pub events: Vec<TimedCommand>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TimedCommand {
    pub at_us: u64,
    #[serde(flatten)]
    pub command: Command,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "command", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Apply the actual pending request's semantic effect, not a supplied result.
    Complete {
        id: String,
    },
    Kill {
        reason: String,
    },
    Revoke {
        coglet: String,
    },
    /// Epoch is captured at native acquisition, NEVER stamped at arrival.
    Native {
        captured_epoch: u64,
        event: NativeEvent,
    },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeEvent {
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
    SessionEnded {
        session: String,
    },
    ToolCall {
        session: String,
        call: String,
        name: String,
        arguments: Value,
    },
}
impl NativeEvent {
    fn validate(&self) -> Result<(), String> {
        let id = match self {
            Self::SpeechStart { utterance, .. }
            | Self::SpeechPartial { utterance, .. }
            | Self::SpeechEnd { utterance, .. } => utterance,
            Self::SessionEnded { session } | Self::ToolCall { session, .. } => session,
        };
        if !coglet_surface_rust::ids::is_correlation_id(id) {
            return Err("invalid captured native id".into());
        }
        if let Self::ToolCall { call, name, .. } = self
            && (!coglet_surface_rust::ids::is_correlation_id(call)
                || name.is_empty()
                || name.len() > 256)
        {
            return Err("invalid captured tool call".into());
        }
        Ok(())
    }
    fn native(self) -> Native {
        match self {
            Self::SpeechStart {
                utterance,
                position_ms,
            } => Native::SpeechStart {
                utterance,
                position_ms,
            },
            Self::SpeechPartial {
                utterance,
                revision,
                text,
            } => Native::SpeechPartial {
                utterance,
                revision,
                text,
            },
            Self::SpeechEnd {
                utterance,
                revision,
                text,
                position_ms,
            } => Native::SpeechEnd {
                utterance,
                revision,
                text,
                position_ms,
            },
            Self::SessionEnded { session } => Native::SessionEnded { session },
            Self::ToolCall {
                session,
                call,
                name,
                arguments,
            } => Native::ToolCall {
                session,
                call,
                name,
                arguments,
            },
        }
    }
}
impl Timeline {
    pub fn parse(src: &str) -> Result<Self, String> {
        if src.len() > MAX_CONFIG_BYTES {
            return Err("timeline config too large".into());
        }
        let t: Self = serde_json::from_str(src).map_err(|e| e.to_string())?;
        t.validate()?;
        Ok(t)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.events.len() > MAX_TIMELINE_EVENTS {
            return Err("too many timeline events".into());
        }
        let mut prev = 0;
        for e in &self.events {
            if e.at_us < prev || e.at_us > MAX_TIMELINE_US {
                return Err("timeline must be sorted and within 60 seconds".into());
            }
            prev = e.at_us;
            match &e.command {
                Command::Complete { id } if !coglet_surface_rust::ids::is_correlation_id(id) => {
                    return Err("invalid completion id".into());
                }
                Command::Revoke { coglet } if !coglet_surface_rust::ids::is_coglet_id(coglet) => {
                    return Err("invalid revoke coglet".into());
                }
                Command::Kill { reason } if reason.is_empty() || reason.len() > 1024 => {
                    return Err("invalid kill reason".into());
                }
                Command::Native {
                    captured_epoch: 0, ..
                } => return Err("native epoch must be positive".into()),
                Command::Native { event, .. } => event.validate()?,
                _ => {}
            }
            if serde_json::to_vec(e).map_err(|e| e.to_string())?.len() > 65_536 {
                return Err("timeline event too large".into());
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct Records {
    executed: Vec<ActionRequest>,
    pending: BTreeMap<String, ActionRequest>,
    cancelled: Vec<String>,
    revoked: Vec<String>,
    stop_all_calls: u64,
}
/// Semantic mock renderer. Delays effect application until an explicit completion.
/// Advertises semantic operations and scripted text-capture observations only.
pub struct ScriptedBackend {
    semantic: Arc<SemanticBackend>,
    records: Mutex<Records>,
    sink: Mutex<Option<Sink>>,
}
impl ScriptedBackend {
    pub fn new(speech: SpeechConfig, initial: ExpressionState) -> Arc<Self> {
        Arc::new(Self {
            semantic: SemanticBackend::new(speech, initial),
            records: Mutex::new(Records::default()),
            sink: Mutex::new(None),
        })
    }
    pub fn executed(&self) -> Vec<ActionRequest> {
        self.records.lock().unwrap().executed.clone()
    }
    pub fn cancelled(&self) -> Vec<String> {
        self.records.lock().unwrap().cancelled.clone()
    }
    pub fn revoked(&self) -> Vec<String> {
        self.records.lock().unwrap().revoked.clone()
    }
    pub fn stop_all_calls(&self) -> u64 {
        self.records.lock().unwrap().stop_all_calls
    }
    pub fn expression(&self, now: u64) -> (String, u64, u64) {
        self.semantic.state(now)
    }
    /// False means unknown/cancelled/already completed: never fabricate an effect.
    pub fn complete(&self, id: &str, now: u64) -> Result<bool, ApiError> {
        let mut records = self.records.lock().unwrap();
        let matches: Vec<String> = records
            .pending
            .iter()
            .filter(|(_, req)| req.wire_id == id)
            .map(|(token, _)| token.clone())
            .collect();
        if matches.len() > 1 {
            return Err(ApiError::conflict("ambiguous wire ID across owners"));
        }
        let Some(token) = matches.first() else {
            return Ok(false);
        };
        let req = records.pending.remove(token).unwrap();
        drop(records);
        if now >= req.deadline {
            return Ok(false);
        }
        self.semantic.execute(&req, now)?;
        Ok(true)
    }
    fn native(&self, event: NativeEvent) {
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            sink.native(event.native());
        }
    }
}
impl Backend for ScriptedBackend {
    fn operations(&self) -> BTreeSet<String> {
        let mut ops = self.semantic.operations();
        // Scripted text capture is an actual mock observation, not simulated playback.
        ops.extend(["speech.start", "speech.partial", "speech.end"].map(str::to_owned));
        ops
    }
    fn speech_config(&self) -> Option<Value> {
        self.semantic.speech_config()
    }
    fn bind(&self, sink: Sink) {
        self.semantic.bind(sink.clone());
        *self.sink.lock().unwrap() = Some(sink);
    }
    fn preflight(&self, req: &ActionRequest, now: u64) -> Result<(), ApiError> {
        self.semantic.preflight(req, now)
    }
    fn execute(&self, req: &ActionRequest, _: u64) -> Result<(), ApiError> {
        let mut r = self.records.lock().unwrap();
        if r.executed.len() >= MAX_TIMELINE_EVENTS {
            return Err(ApiError::exhausted("mock request log full"));
        }
        r.executed.push(req.clone());
        r.pending.insert(req.id.clone(), req.clone());
        Ok(())
    }
    fn cancel(&self, id: &str) {
        let mut r = self.records.lock().unwrap();
        r.pending.remove(id);
        if let Some(req) = r.executed.iter().find(|req| req.id == id) {
            let wire_id = req.wire_id.clone();
            r.cancelled.push(wire_id);
        }
    }
    fn read(&self, op: &str, args: &Value, now: u64) -> Result<ReadSnapshot, ApiError> {
        self.semantic.read(op, args, now)
    }
    fn tick(&self, now: u64) {
        self.semantic.tick(now);
    }
    fn revoke(&self, coglet: &str) {
        let mut r = self.records.lock().unwrap();
        r.pending.retain(|_, p| p.coglet != coglet);
        r.revoked.push(coglet.into());
        self.semantic.revoke(coglet);
    }
    fn stop_all(&self) {
        let mut r = self.records.lock().unwrap();
        r.pending.clear();
        r.stop_all_calls += 1;
        self.semantic.stop_all();
    }
}

pub struct MockSurface {
    core: Surface,
    scripted: Option<Arc<ScriptedBackend>>,
    timeline: VecDeque<TimedCommand>,
    now: u64,
    pub discarded_native: u64,
}
impl MockSurface {
    /// Explicit ADR-0014 harness constructor; from_fixture/open remain production.
    pub fn open_echo_fixture(
        config: Config,
        state_dir: &Path,
        backend: Arc<dyn Backend>,
    ) -> Result<Self, String> {
        let now = config.start_clock;
        Ok(Self {
            core: Surface::open_echo_fixture(config, state_dir, backend)?,
            scripted: None,
            timeline: VecDeque::new(),
            now,
            discarded_native: 0,
        })
    }
    pub fn emit_echo_fixture(
        &mut self,
        now: u64,
        payload: Value,
        owner: &str,
    ) -> Result<Vec<Vec<u8>>, String> {
        let mut out = self.tick_at(now)?;
        out.extend(self.core.emit_echo_fixture(now, payload, owner)?);
        Ok(out)
    }
    pub fn open(
        config: Config,
        state_dir: &Path,
        backend: Arc<dyn Backend>,
    ) -> Result<Self, String> {
        let now = config.start_clock;
        Ok(Self {
            core: Surface::open(config, state_dir, backend)?,
            scripted: None,
            timeline: VecDeque::new(),
            now,
            discarded_native: 0,
        })
    }
    pub fn scripted(
        config: Config,
        state_dir: &Path,
        backend: Arc<ScriptedBackend>,
        timeline: Timeline,
    ) -> Result<Self, String> {
        let mut me = Self::open(config, state_dir, backend.clone())?;
        me.scripted = Some(backend);
        me.install_timeline(timeline)?;
        Ok(me)
    }
    /// Fixture case influences only the SDK's explicit startup_epoch_by_case map.
    pub fn from_fixture(src: &str, case: Option<&str>, state_dir: &Path) -> Result<Self, String> {
        if src.len() > MAX_CONFIG_BYTES {
            return Err("fixture too large".into());
        }
        let fixture = AdapterFixture::parse(src, case)?;
        let value: Value = serde_json::from_str(src).map_err(|e| e.to_string())?;
        if let Some(t) = value.get("mock_timeline") {
            let timeline = Timeline::parse(&t.to_string())?;
            Self::scripted(
                fixture.config,
                state_dir,
                ScriptedBackend::new(fixture.speech, fixture.expression),
                timeline,
            )
        } else {
            Self::open(
                fixture.config,
                state_dir,
                SemanticBackend::new(fixture.speech, fixture.expression),
            )
        }
    }
    pub fn core(&self) -> &Surface {
        &self.core
    }
    pub fn install_timeline(&mut self, timeline: Timeline) -> Result<(), String> {
        timeline.validate()?;
        if timeline.events.first().is_some_and(|e| e.at_us < self.now) {
            return Err("timeline starts in past".into());
        }
        if self.scripted.is_none()
            && timeline
                .events
                .iter()
                .any(|e| matches!(e.command, Command::Complete { .. } | Command::Native { .. }))
        {
            return Err("completion/native timeline requires ScriptedBackend".into());
        }
        self.timeline = timeline.events.into();
        Ok(())
    }
    pub fn tick_at(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String> {
        if now < self.now {
            return Err("clock rollback".into());
        }
        let mut out = Vec::new();
        while self.timeline.front().is_some_and(|e| e.at_us <= now) {
            let event = self.timeline.pop_front().unwrap();
            // Expiry/deadline processing precedes effects at the same exclusive bound.
            out.extend(self.core.tick(event.at_us)?);
            match event.command {
                Command::Complete { id } => {
                    self.scripted
                        .as_ref()
                        .ok_or("not scripted")?
                        .complete(&id, event.at_us)
                        .map_err(|e| e.message)?;
                }
                Command::Kill { reason } => out.extend(self.core.local_kill(event.at_us, &reason)?),
                Command::Revoke { coglet } => out.extend(self.core.revoke(event.at_us, &coglet)?),
                Command::Native {
                    captured_epoch,
                    event,
                } => {
                    if self.core.is_negotiated()
                        && captured_epoch == self.core.grant_epoch()
                        && !self.core.kill_switch().is_latched()
                    {
                        self.scripted.as_ref().ok_or("not scripted")?.native(event);
                    } else {
                        self.discarded_native += 1;
                    }
                }
            }
            out.extend(self.core.tick(event.at_us)?);
        }
        out.extend(self.core.tick(now)?);
        self.now = now;
        Ok(out)
    }
}
impl ControlPeer for MockSurface {
    fn start(&mut self) -> Result<Vec<Vec<u8>>, String> {
        self.core.start()
    }
    fn receive(&mut self, bytes: &[u8], now: u64) -> Result<Vec<Vec<u8>>, String> {
        let mut out = self.tick_at(now)?;
        out.extend(self.core.receive(bytes, now)?);
        Ok(out)
    }
    fn tick(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String> {
        self.tick_at(now)
    }
    fn disconnect(&mut self, now: u64) -> Result<(), String> {
        self.timeline.clear();
        self.core.disconnect(now)
    }
}
