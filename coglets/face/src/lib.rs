//! Product-neutral text-only Face. No transcript or replay queue persists.
use ciborium::Value;
use coglet_guest_rust::{Action, Context, Event, Host, HostError, coglet};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const ID: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
pub const STATE_PATH: &str = "/data/face-state.json";
pub const MAX_STATE_BYTES: usize = 8192;
pub const MAX_SOURCES: usize = 8;
pub const MAX_TEXT_BYTES: usize = 4096;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    sources: BTreeMap<String, Capture>,
    last_step: Option<u64>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    seq: Option<u64>,
    utterance: Option<String>,
    revision: u64,
    closed: bool,
}
fn hex_id(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn text<'a>(payload: &'a BTreeMap<String, Value>, key: &str) -> Result<&'a str, HostError> {
    payload
        .get(key)
        .and_then(Value::as_text)
        .ok_or(HostError::InvalidArgument)
}
fn revision(payload: &BTreeMap<String, Value>) -> Result<u64, HostError> {
    payload
        .get("revision")
        .and_then(Value::as_integer)
        .and_then(|v| u64::try_from(v).ok())
        .filter(|n| *n > 0)
        .ok_or(HostError::InvalidArgument)
}
fn valid_source(source: &str) -> bool {
    source
        .strip_prefix("surface:")
        .is_some_and(|s| hex_id(s, 64))
}
fn load<H: Host>(ctx: &mut Context<'_, H>) -> Result<State, HostError> {
    let Some(bytes) = ctx.read_file(STATE_PATH)? else {
        return Ok(State::default());
    };
    if bytes.len() > MAX_STATE_BYTES {
        return Err(HostError::ResourceExhausted);
    }
    let state: State = serde_json::from_slice(&bytes).map_err(|_| HostError::InvalidArgument)?;
    if state.sources.len() > MAX_SOURCES
        || state.sources.iter().any(|(source, c)| {
            !valid_source(source) || c.utterance.as_ref().is_some_and(|id| !hex_id(id, 32))
        })
    {
        return Err(HostError::InvalidArgument);
    }
    Ok(state)
}

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    let mut state = load(ctx)?;
    // A restored filesystem must not combine with a reset context ID sequence.
    // Replay starts from its archived pre-step state, not this committed state.
    if state.last_step.is_some_and(|step| ctx.step <= step) {
        return Err(HostError::Conflict);
    }
    state.last_step = Some(ctx.step);
    for event in events {
        if !matches!(
            event.kind.as_str(),
            "speech.start" | "speech.partial" | "speech.end" | "speech.barge_in"
        ) {
            continue;
        }
        if !valid_source(&event.source) {
            return Err(HostError::InvalidArgument);
        }
        if !state.sources.contains_key(&event.source) && state.sources.len() >= MAX_SOURCES {
            // Never evict sequence fences and accidentally reopen an old source.
            return Err(HostError::ResourceExhausted);
        }
        let capture = state.sources.entry(event.source.clone()).or_default();
        if capture.seq.is_some_and(|seq| event.seq <= seq) {
            continue;
        }
        capture.seq = Some(event.seq);
        // Native reflex has already stopped/ducked output. Keep no output text,
        // issue no replacement stream, and never infer heard text from playback.
        if event.kind == "speech.barge_in" {
            continue;
        }
        let utterance = text(&event.payload, "utterance")?;
        if !hex_id(utterance, 32) {
            return Err(HostError::InvalidArgument);
        }
        if event.kind == "speech.start" {
            if capture.utterance.as_deref() != Some(utterance) {
                capture.utterance = Some(utterance.into());
                capture.revision = 0;
                capture.closed = false;
            }
            continue;
        }
        // Only the active, started utterance can finish. Late old finals cannot
        // replace an in-progress new utterance, even with a higher source seq.
        if capture.closed || capture.utterance.as_deref() != Some(utterance) {
            continue;
        }
        let rev = revision(&event.payload)?;
        if rev <= capture.revision {
            continue;
        }
        capture.revision = rev;
        if event.kind == "speech.partial" {
            continue;
        }
        capture.closed = true; // Empty terminal transcripts also close capture.
        let transcript = text(&event.payload, "text")?;
        if transcript.len() > 16384 {
            return Err(HostError::InvalidArgument);
        }
        if transcript.trim().is_empty() {
            continue;
        }
        let mut end = transcript.len().min(MAX_TEXT_BYTES);
        while !transcript.is_char_boundary(end) {
            end -= 1;
        }
        let id = ctx.next_id()?;
        let stream = ctx.next_id()?;
        ctx.emit(&Action {
            id,
            coglet: ID.into(),
            capability: "speech.speak_chunk".into(),
            args: BTreeMap::from([
                ("stream".into(), Value::Text(stream)),
                ("chunk".into(), Value::Integer(0.into())),
                ("text".into(), Value::Text(transcript[..end].into())),
                ("final".into(), Value::Bool(true)),
            ]),
            deadline: ctx
                .logical_time
                .checked_add(30_000_000)
                .ok_or(HostError::InvalidArgument)?,
            correlation_id: event.correlation_id.clone(),
        })?;
    }
    let bytes = serde_json::to_vec(&state).map_err(|e| HostError::Encoding(e.to_string()))?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(HostError::ResourceExhausted);
    }
    // Host must commit state and staged emits atomically, or roll both back.
    ctx.write_file(STATE_PATH, &bytes)
}
