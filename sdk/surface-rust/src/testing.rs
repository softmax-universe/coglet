//! Test doubles for driving the core (not production backends, no hardware semantics).
//!
//! [`ScriptedBackend`] lets a test or a configurable mock declare any set of operations as
//! "implemented", records every call the core makes, and completes actions only when the
//! script says so (immediately, or later through [`ScriptedBackend::complete`]). It never
//! fabricates success on its own: unless `auto_complete` is enabled for an operation the
//! action stays pending, so deadline/stop/revoke paths are observable. Expression and
//! configuration reads delegate to the real [`SemanticBackend`].

use crate::backend::{ActionRequest, Backend, BackendHealth, Native, ReadSnapshot, Sink};
use crate::config::{ExpressionState, SpeechConfig};
use crate::error::ApiError;
use crate::reference::SemanticBackend;
use serde_json::Value as Json;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Script {
    ops: BTreeSet<String>,
    auto_complete: BTreeSet<String>,
    executed: Vec<ActionRequest>,
    cancelled: Vec<String>,
    revoked: Vec<String>,
    stop_all_calls: u32,
    fail_execute: Option<ApiError>,
    routes: Vec<String>,
    tools: Vec<String>,
    degraded: bool,
}

pub struct ScriptedBackend {
    sem: Arc<SemanticBackend>,
    script: Mutex<Script>,
    sink: Mutex<Option<Sink>>,
    speech: SpeechConfig,
}

impl ScriptedBackend {
    pub fn new(speech: SpeechConfig, ops: &[&str]) -> Arc<Self> {
        let s = Script {
            ops: ops.iter().map(|o| (*o).to_owned()).collect(),
            ..Script::default()
        };
        Arc::new(Self {
            sem: SemanticBackend::new(speech.clone(), ExpressionState::default()),
            script: Mutex::new(s),
            sink: Mutex::new(None),
            speech,
        })
    }
    fn g(&self) -> std::sync::MutexGuard<'_, Script> {
        self.script.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn auto_complete(&self, op: &str) {
        self.g().auto_complete.insert(op.to_owned());
    }
    pub fn set_routes(&self, routes: &[&str]) {
        self.g().routes = routes.iter().map(|r| (*r).to_owned()).collect();
    }
    pub fn set_tools(&self, tools: &[&str]) {
        self.g().tools = tools.iter().map(|r| (*r).to_owned()).collect();
    }
    pub fn fail_next_execute(&self, e: ApiError) {
        self.g().fail_execute = Some(e);
    }
    pub fn set_degraded(&self, d: bool) {
        self.g().degraded = d;
    }
    pub fn executed(&self) -> Vec<ActionRequest> {
        self.g().executed.clone()
    }
    pub fn executed_ops(&self) -> Vec<String> {
        self.g().executed.iter().map(|r| r.op.clone()).collect()
    }
    pub fn cancelled(&self) -> Vec<String> {
        let g = self.g();
        g.cancelled
            .iter()
            .map(|id| {
                g.executed
                    .iter()
                    .find(|r| &r.id == id)
                    .map_or_else(|| id.clone(), |r| r.wire_id.clone())
            })
            .collect()
    }
    pub fn revoked(&self) -> Vec<String> {
        self.g().revoked.clone()
    }
    pub fn stop_all_calls(&self) -> u32 {
        self.g().stop_all_calls
    }
    pub fn sink(&self) -> Sink {
        self.sink
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .expect("backend not bound")
    }
    /// Report that the native effect for `id` actually finished.
    pub fn complete(&self, id: &str, result: Result<(), ApiError>) {
        // Single-owner convenience. Ambiguous wire IDs require native tokens.
        let g = self.g();
        let matches: Vec<_> = g.executed.iter().filter(|r| r.wire_id == id).collect();
        let token = if matches.len() == 1 {
            matches[0].id.clone()
        } else {
            id.to_owned()
        };
        drop(g);
        self.sink().complete(&token, result);
    }
    pub fn native(&self, n: Native) {
        let sink = self.sink();
        match &n {
            Native::ToolCall { session, .. } | Native::SessionEnded { session } => {
                let owner = self
                    .g()
                    .executed
                    .iter()
                    .find(|r| r.op == "speech.session_open" && r.args["session"] == *session)
                    .map(|r| r.coglet.clone());
                if let Some(owner) = owner {
                    sink.capture(&owner, Some(session)).native(n);
                }
            }
            _ => sink.native(n),
        }
    }
    pub fn expression(&self, now: u64) -> (String, u64, u64) {
        self.sem.state(now)
    }
}

impl Backend for ScriptedBackend {
    fn operations(&self) -> BTreeSet<String> {
        self.g().ops.clone()
    }
    fn speech_config(&self) -> Option<Json> {
        Some(self.speech.to_json())
    }
    fn session_routes(&self) -> Vec<String> {
        self.g().routes.clone()
    }
    fn tool_allowlist(&self) -> Vec<String> {
        self.g().tools.clone()
    }
    fn bind(&self, sink: Sink) {
        self.sem.bind(sink.clone());
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }
    fn execute(&self, req: &ActionRequest, now: u64) -> Result<(), ApiError> {
        if req.op.starts_with("expression.") {
            return self.sem.execute(req, now);
        }
        let auto = {
            let mut g = self.g();
            req.kill_fence.check()?;
            g.executed.push(req.clone());
            if let Some(e) = g.fail_execute.take() {
                return Err(e);
            }
            g.auto_complete.contains(&req.op)
        };
        if auto {
            self.complete(&req.id, Ok(()));
        }
        Ok(())
    }
    fn cancel(&self, id: &str) {
        self.g().cancelled.push(id.to_owned());
    }
    fn read(&self, op: &str, args: &Json, now: u64) -> Result<ReadSnapshot, ApiError> {
        self.sem.read(op, args, now)
    }
    fn tick(&self, now: u64) {
        self.sem.tick(now);
    }
    fn revoke(&self, coglet: &str) {
        self.g().revoked.push(coglet.to_owned());
        self.sem.revoke(coglet);
    }
    fn stop_all(&self) {
        self.g().stop_all_calls += 1;
        self.sem.stop_all();
    }
    fn health(&self) -> BackendHealth {
        if self.g().degraded {
            BackendHealth::Degraded
        } else {
            BackendHealth::Ready
        }
    }
}
