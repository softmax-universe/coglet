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
pub const INBOX_PATH: &str = "/data/inbox.jsonl";
pub const MAX_INBOX_BYTES: usize = 512 * 1024;
/// Inbox event kind a router uses to ask the Face to speak. Payload: `{text}`.
pub const SAY_KIND: &str = "face.say";
/// Optional installed identity `{"coglet": "coglet:<64 hex>"}`. A router
/// deployment that registers the Face under its own signing key installs this;
/// without it the Face keeps the fixed product-neutral [`ID`].
pub const IDENTITY_PATH: &str = "/code/face/identity.json";
pub const MAX_IDENTITY_BYTES: usize = 256;

#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    sources: BTreeMap<String, Capture>,
    last_step: Option<u64>,
    /// Next unconsumed `/data/inbox.jsonl` index. Absent in states written
    /// before router speech existed.
    #[serde(default, skip_serializing_if = "is_zero")]
    inbox_cursor: u64,
}
fn is_zero(n: &u64) -> bool {
    *n == 0
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
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Identity {
    coglet: String,
}
/// The Face's own coglet id: the installed identity, else [`ID`]. A present but
/// malformed identity fails closed rather than falling back.
fn identity<H: Host>(ctx: &mut Context<'_, H>) -> Result<String, HostError> {
    let Some(bytes) = ctx.read_file(IDENTITY_PATH)? else {
        return Ok(ID.into());
    };
    if bytes.len() > MAX_IDENTITY_BYTES {
        return Err(HostError::ResourceExhausted);
    }
    let identity: Identity =
        serde_json::from_slice(&bytes).map_err(|_| HostError::InvalidArgument)?;
    if !identity
        .coglet
        .strip_prefix("coglet:")
        .is_some_and(|key| hex_id(key, 64))
    {
        return Err(HostError::InvalidArgument);
    }
    Ok(identity.coglet)
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

/// Speak router-authored text. Only host-materialized `runtime.inbox` wakes
/// authorize reading `/data/inbox.jsonl`; rows are consumed once, in order, up
/// to the newest wake. Inbox structure is validated fail-closed; a row that is
/// not a well-formed `face.say` from a router (`hub:` sender) is skipped, so a
/// bad message cannot wedge the cursor.
fn say_from_router<H: Host>(
    ctx: &mut Context<'_, H>,
    own: &str,
    state: &mut State,
    events: &[Event],
) -> Result<(), HostError> {
    let mut upto: Option<u64> = None;
    for e in events
        .iter()
        .filter(|e| e.kind == "runtime.inbox" && e.source == own)
    {
        let n = e
            .payload
            .get("index")
            .and_then(Value::as_integer)
            .and_then(|n| u64::try_from(n).ok())
            .ok_or(HostError::InvalidArgument)?;
        upto = Some(upto.map_or(n, |m| m.max(n)));
    }
    let Some(upto) = upto else {
        return Ok(());
    };
    let raw = ctx.read_file(INBOX_PATH)?.unwrap_or_default();
    if raw.len() > MAX_INBOX_BYTES {
        return Err(HostError::ResourceExhausted);
    }
    if !raw.is_empty() && raw.last() != Some(&b'\n') {
        return Err(HostError::InvalidArgument);
    }
    let end = upto.checked_add(1).ok_or(HostError::InvalidArgument)?;
    let mut index = 0u64;
    for line in raw.split_inclusive(|b| *b == b'\n') {
        let line = &line[..line.len() - 1];
        let row: serde_json::Value =
            serde_json::from_slice(line).map_err(|_| HostError::InvalidArgument)?;
        // Canonical bytes (sorted keys, no spaces) and dense indices.
        if serde_json::to_vec(&row).map_err(|_| HostError::InvalidArgument)? != line
            || row["index"].as_u64() != Some(index)
            || row["sender"] != row["event"]["source"]
        {
            return Err(HostError::InvalidArgument);
        }
        if index >= state.inbox_cursor && index < end {
            say_row(ctx, own, &row)?;
        }
        index += 1;
    }
    if end > index {
        return Err(HostError::InvalidArgument);
    }
    state.inbox_cursor = state.inbox_cursor.max(end);
    Ok(())
}
fn say_row<H: Host>(
    ctx: &mut Context<'_, H>,
    own: &str,
    row: &serde_json::Value,
) -> Result<(), HostError> {
    let event = &row["event"];
    let router = event["source"]
        .as_str()
        .and_then(|s| s.strip_prefix("hub:"))
        .is_some_and(|id| hex_id(id, 64));
    let well_formed = event["kind"] == SAY_KIND
        && event["payload"]
            .as_object()
            .is_some_and(|payload| payload.len() == 1);
    let (true, true, Some(text), Some(correlation)) = (
        router,
        well_formed,
        event["payload"]["text"].as_str(),
        event["correlation_id"].as_str(),
    ) else {
        return Ok(());
    };
    if text.trim().is_empty() || text.len() > MAX_TEXT_BYTES {
        return Ok(());
    }
    let id = ctx.next_id()?;
    let stream = ctx.next_id()?;
    ctx.emit(&Action {
        id,
        coglet: own.into(),
        capability: "speech.speak_chunk".into(),
        args: BTreeMap::from([
            ("stream".into(), Value::Text(stream)),
            ("chunk".into(), Value::Integer(0.into())),
            ("text".into(), Value::Text(text.into())),
            ("final".into(), Value::Bool(true)),
        ]),
        deadline: ctx
            .logical_time
            .checked_add(30_000_000)
            .ok_or(HostError::InvalidArgument)?,
        correlation_id: correlation.into(),
    })
}

#[coglet]
pub fn wake<H: Host>(ctx: &mut Context<'_, H>, events: &[Event]) -> Result<(), HostError> {
    let own = identity(ctx)?;
    let mut state = load(ctx)?;
    // A restored filesystem must not combine with a reset context ID sequence.
    // Replay starts from its archived pre-step state, not this committed state.
    if state.last_step.is_some_and(|step| ctx.step <= step) {
        return Err(HostError::Conflict);
    }
    state.last_step = Some(ctx.step);
    say_from_router(ctx, &own, &mut state, events)?;
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
            coglet: own.clone(),
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
