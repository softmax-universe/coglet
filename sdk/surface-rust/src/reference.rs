//! Reference semantic backend: expression state plus the installed speech configuration.
//!
//! This is a *semantic renderer*, in the sense of expression.md: it keeps the expression
//! state a face would show and reads it back, with the native-clock expiry and fencing the
//! spec requires. It has NO audio and NO display hardware, and it deliberately does not
//! implement `speech.speak_chunk`, `speech.backchannel`, speech events or provider
//! sessions, so those are never advertised and no playback success can be manufactured.
//!
//! `speech.stop` is advertised only because the core's own stream table answers it
//! honestly: with no speak operation advertised no stream can exist, so every stop is
//! `not_found` (unknown stream) and nothing is ever silenced falsely.

use crate::backend::{ActionRequest, Backend, ReadSnapshot, Sink};
use crate::config::{ExpressionState, SpeechConfig};
use crate::error::ApiError;
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[derive(Debug)]
struct Inner {
    expression: String,
    intensity: u64,
    /// Native monotonic expiry instant (surface clock microseconds).
    expires_at: Option<u64>,
    revision: u64,
}

impl Inner {
    fn reset(&mut self) {
        if self.expression != "neutral" || self.intensity != 0 || self.expires_at.is_some() {
            self.expression = "neutral".into();
            self.intensity = 0;
            self.expires_at = None;
            self.revision += 1;
        }
    }
    fn expire(&mut self, now: u64) {
        if self.expires_at.is_some_and(|t| now >= t) {
            self.reset();
        }
    }
}

pub struct SemanticBackend {
    speech: SpeechConfig,
    inner: Mutex<Inner>,
    sink: Mutex<Option<Sink>>,
}

impl SemanticBackend {
    pub fn new(speech: SpeechConfig, initial: ExpressionState) -> Arc<Self> {
        Arc::new(Self {
            speech,
            inner: Mutex::new(Inner {
                expression: initial.expression,
                intensity: initial.intensity,
                expires_at: None,
                revision: 1,
            }),
            sink: Mutex::new(None),
        })
    }

    /// Current state, for tests and diagnostics.
    pub fn state(&self, now: u64) -> (String, u64, u64) {
        let mut g = self.lock();
        g.expire(now);
        let rem = g.expires_at.map_or(0, |t| (t - now).div_ceil(1000));
        (g.expression.clone(), g.intensity, rem)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Backend for SemanticBackend {
    fn operations(&self) -> BTreeSet<String> {
        [
            "speech.config",
            "speech.stop",
            "expression.set_expression",
            "expression.state",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
    fn speech_config(&self) -> Option<Json> {
        Some(self.speech.to_json())
    }
    fn bind(&self, sink: Sink) {
        *self.sink.lock().unwrap_or_else(|e| e.into_inner()) = Some(sink);
    }
    fn execute(&self, req: &ActionRequest, now: u64) -> Result<(), ApiError> {
        match req.op.as_str() {
            "expression.set_expression" => {
                let a = &req.args;
                let dur = a["duration_ms"].as_u64().unwrap_or(0);
                {
                    let mut g = self.lock();
                    req.kill_fence.check()?;
                    g.expire(now);
                    if dur == 0 {
                        g.reset();
                    } else {
                        g.expression = a["expression"].as_str().unwrap_or("neutral").to_owned();
                        g.intensity = a["intensity"].as_u64().unwrap_or(0);
                        g.expires_at = Some(now.saturating_add(dur.saturating_mul(1000)));
                        g.revision += 1;
                    }
                }
                self.done(&req.id);
                Ok(())
            }
            // The core only admits a stop for a stream it knows; none can exist here.
            "speech.stop" => {
                self.done(&req.id);
                Ok(())
            }
            _ => Err(ApiError::unavailable(
                "operation not implemented by backend",
            )),
        }
    }
    fn read(&self, op: &str, _args: &Json, now: u64) -> Result<ReadSnapshot, ApiError> {
        match op {
            "speech.config" => Ok(ReadSnapshot {
                value: self.speech.to_json(),
                snapshot: 1,
            }),
            "expression.state" => {
                let mut g = self.lock();
                g.expire(now);
                let rem = g
                    .expires_at
                    .map_or(0, |t| (t - now).div_ceil(1000))
                    .min(60_000);
                Ok(ReadSnapshot {
                    value: json!({"expression": g.expression, "intensity": g.intensity, "remaining_ms": rem}),
                    snapshot: g.revision,
                })
            }
            _ => Err(ApiError::unavailable("read not implemented by backend")),
        }
    }
    fn tick(&self, now: u64) {
        self.lock().expire(now);
    }
    fn revoke(&self, _coglet: &str) {
        self.lock().reset();
    }
    fn stop_all(&self) {
        self.lock().reset();
    }
}

impl SemanticBackend {
    fn done(&self, id: &str) {
        if let Some(s) = self.sink.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            s.complete(id, Ok(()));
        }
    }
}
