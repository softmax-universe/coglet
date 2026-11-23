//! Speech stream/session/tool state and the native event normalizer (pure, no I/O).
//!
//! The core owns *bookkeeping and ordering*; the backend owns *physical effect*. Nothing
//! here marks audio as rendered: chunk and backchannel success only ever arrives from the
//! backend sink, and the core merely checks that it arrives in chunk order.
//!
//! Implements speech.md rules: contiguous chunks 0.., at most 64 admitted per stream,
//! UTF-8 4,096-byte text bound, one open speak stream, gap cancellation, stop idempotence
//! and tombstones, idle close, owner-bound provider sessions, tool-call once-only
//! delivery, `tool_result` once-only with conflict, utterance revision filtering and
//! barge-in/position monotonicity. Heard-history and speech.work are NOT implemented.

use crate::backend::Native;
use crate::error::ApiError;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

pub const MAX_CHUNKS_PER_STREAM: u64 = 64;
pub const MAX_CHUNK_TEXT_BYTES: usize = 4096;
pub const MAX_TRANSCRIPT_BYTES: usize = 16_384;
pub const MAX_CALLS_PER_SESSION: usize = 128;
const MAX_UTTERANCES: usize = 1024;

#[derive(Debug, Clone)]
pub struct Stream {
    pub owner: String,
    pub backchannel: bool,
    pub next_chunk: u64,
    pub admitted: u64,
    pub finalized: bool,
    pub closed: bool,
    pub last_admit: u64,
    /// Admitted, non-terminal chunk action ids in chunk order.
    pub pending: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Pending,
    Resulted(String),
}

#[derive(Debug, Clone)]
pub struct Session {
    pub owner: String,
    pub open: bool,
    /// False between admission of `session_open` and its successful completion.
    pub ready: bool,
    calls: BTreeMap<String, Call>,
}

#[derive(Debug, Clone, Default)]
struct Utterance {
    revision: u64,
    ended: bool,
}

/// What the core should do with a normalized native observation.
#[derive(Debug, PartialEq)]
pub enum Normalized {
    /// Deliver `kind` with `payload` to `owner` (or to the grant holder of the resources).
    Event {
        kind: &'static str,
        payload: Json,
        owner: Option<String>,
    },
    /// The reflex stopped this stream natively: cancel its admitted chunks.
    StreamStopped { stream: String, cancel: Vec<String> },
    /// Native item discarded (kept as a counter; never forwards private text).
    Discarded(&'static str),
}

#[derive(Default)]
pub struct SpeechState {
    streams: BTreeMap<String, Stream>,
    sessions: BTreeMap<String, Session>,
    active_session: Option<String>,
    utterances: BTreeMap<String, Utterance>,
    utt_order: VecDeque<String>,
    /// (stream, original wire action item) -> max played_ms. Never native tokens.
    items: BTreeMap<(String, String), u64>,
    stream_position: BTreeMap<String, u64>,
    provider_streams: BTreeMap<String, String>,
    tool_allow: BTreeSet<String>,
    pub discarded: u64,
}

fn s<'a>(a: &'a Json, k: &str) -> &'a str {
    a.get(k).and_then(Json::as_str).unwrap_or_default()
}

impl SpeechState {
    pub fn new(tool_allowlist: &[String]) -> Self {
        Self {
            tool_allow: tool_allowlist.iter().cloned().collect(),
            ..Self::default()
        }
    }

    pub fn stream(&self, id: &str) -> Option<&Stream> {
        self.streams.get(id)
    }
    pub fn session(&self, id: &str) -> Option<&Session> {
        self.sessions.get(id)
    }
    /// Validate immutable callback context BEFORE normalization can create stream
    /// associations. Never associate an unscoped late provider item with whichever
    /// session happens to be active at callback arrival.
    pub(crate) fn captured_native_allowed(
        &self,
        n: &Native,
        owner: Option<&str>,
        session: Option<&str>,
    ) -> bool {
        if let Some(sid) = session
            && (self.active_session.as_deref() != Some(sid)
                || !self
                    .sessions
                    .get(sid)
                    .is_some_and(|s| s.open && s.ready && Some(s.owner.as_str()) == owner))
        {
            return false;
        }
        match n {
            Native::ToolCall { session: sid, .. } | Native::SessionEnded { session: sid } => {
                session == Some(sid.as_str())
            }
            Native::Position { stream, .. } | Native::BargeIn { stream, .. } => {
                if let Some(st) = self.streams.get(stream) {
                    Some(st.owner.as_str()) == owner && session.is_none()
                } else {
                    session.is_some()
                        && self
                            .provider_streams
                            .get(stream)
                            .is_none_or(|sid| Some(sid.as_str()) == session)
                }
            }
            _ => true,
        }
    }

    pub fn active_session(&self) -> Option<&str> {
        self.active_session.as_deref()
    }
    fn open_stream(&self) -> bool {
        self.streams.values().any(|s| !s.closed)
    }

    /// Rebuild closed tombstones from the durable ledger after a restart. Nothing is
    /// open after a restart (uncertain work was failed and the backend stopped).
    pub fn restore_action(&mut self, coglet: &str, op: &str, args: &Json) {
        match op {
            "speech.speak_chunk" | "speech.backchannel" => {
                let id = s(args, "stream").to_owned();
                let e = self.streams.entry(id).or_insert_with(|| Stream {
                    owner: coglet.to_owned(),
                    backchannel: op == "speech.backchannel",
                    next_chunk: 0,
                    admitted: 0,
                    finalized: true,
                    closed: true,
                    last_admit: 0,
                    pending: vec![],
                });
                e.admitted += 1;
                e.next_chunk = e
                    .next_chunk
                    .max(args.get("chunk").and_then(Json::as_u64).unwrap_or(0) + 1);
            }
            "speech.session_open" => {
                self.sessions.insert(
                    s(args, "session").to_owned(),
                    Session {
                        owner: coglet.to_owned(),
                        open: false,
                        ready: false,
                        calls: BTreeMap::new(),
                    },
                );
            }
            _ => {}
        }
    }

    /// Semantic admission check; pure. Generic checks (grant, resource ownership, schema,
    /// deadline, queue) have already passed.
    pub fn check(
        &self,
        op: &str,
        coglet: &str,
        args: &Json,
        session_mode: Option<&str>,
        routes: &[String],
    ) -> Result<(), ApiError> {
        match op {
            "speech.speak_chunk" => {
                let stream = s(args, "stream");
                let chunk = args.get("chunk").and_then(Json::as_u64).unwrap_or(u64::MAX);
                if s(args, "text").len() > MAX_CHUNK_TEXT_BYTES {
                    return Err(ApiError::invalid("chunk text exceeds 4096 UTF-8 bytes"));
                }
                if self.active_session.is_some() {
                    return Err(ApiError::conflict("provider session owns the speaker"));
                }
                match self.streams.get(stream) {
                    Some(st) => {
                        if st.owner != coglet || st.backchannel {
                            return Err(ApiError::conflict("stream id is not reusable"));
                        }
                        if st.closed || st.finalized {
                            return Err(ApiError::conflict("stream is closed to new chunks"));
                        }
                        if chunk != st.next_chunk {
                            return Err(ApiError::conflict(
                                "chunk is not the next contiguous index",
                            ));
                        }
                        if st.admitted >= MAX_CHUNKS_PER_STREAM {
                            return Err(ApiError::exhausted(
                                "stream already has 64 admitted chunks",
                            ));
                        }
                    }
                    None => {
                        if chunk != 0 {
                            return Err(ApiError::conflict("first chunk of a stream must be 0"));
                        }
                        if self.open_stream() {
                            return Err(ApiError::conflict("another speak stream is open"));
                        }
                    }
                }
            }
            "speech.backchannel" => {
                if self.streams.contains_key(s(args, "stream")) {
                    return Err(ApiError::conflict("stream id is not reusable"));
                }
                if self.open_stream() || self.active_session.is_some() {
                    return Err(ApiError::conflict("speaker is busy"));
                }
            }
            "speech.stop" => match self.streams.get(s(args, "stream")) {
                Some(st) if st.owner == coglet => {}
                _ => return Err(ApiError::not_found("unknown stream")),
            },
            "speech.session_open" => {
                if session_mode != Some("speech_to_speech") {
                    return Err(ApiError::conflict(
                        "speech config session_mode is not speech_to_speech",
                    ));
                }
                if !routes.iter().any(|r| r == s(args, "route")) {
                    return Err(ApiError::invalid(
                        "route is not an allowlisted provider preset",
                    ));
                }
                if self.sessions.contains_key(s(args, "session")) {
                    return Err(ApiError::conflict("session id already used"));
                }
                if self.active_session.is_some() || self.open_stream() {
                    return Err(ApiError::conflict("microphone/speaker pair is busy"));
                }
            }
            "speech.session_close" => match self.sessions.get(s(args, "session")) {
                Some(x) if x.owner == coglet => {}
                _ => return Err(ApiError::not_found("unknown session")),
            },
            "speech.tool_result" => {
                let sess = self
                    .sessions
                    .get(s(args, "session"))
                    .filter(|x| x.owner == coglet && x.open)
                    .ok_or_else(|| ApiError::not_found("unknown or closed session"))?;
                match sess.calls.get(s(args, "call")) {
                    None => return Err(ApiError::not_found("unknown call")),
                    Some(Call::Pending) => {}
                    Some(Call::Resulted(d)) => {
                        if *d != result_digest(args) {
                            return Err(ApiError::conflict("conflicting duplicate tool result"));
                        }
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Apply an admitted action. Returns pending chunk action ids that a `speech.stop`
    /// cancels (the core records their `cancelled` outcomes).
    pub fn admit(
        &mut self,
        op: &str,
        coglet: &str,
        id: &str,
        args: &Json,
        now: u64,
    ) -> Vec<String> {
        self.admit_bound(op, coglet, id, id, args, now)
    }

    /// Native completion tokens must never replace persisted wire media identities.
    #[allow(clippy::too_many_arguments)]
    pub fn admit_bound(
        &mut self,
        op: &str,
        coglet: &str,
        id: &str,
        wire_id: &str,
        args: &Json,
        now: u64,
    ) -> Vec<String> {
        match op {
            "speech.speak_chunk" => {
                let stream = s(args, "stream").to_owned();
                let st = self
                    .streams
                    .entry(stream.clone())
                    .or_insert_with(|| Stream {
                        owner: coglet.to_owned(),
                        backchannel: false,
                        next_chunk: 0,
                        admitted: 0,
                        finalized: false,
                        closed: false,
                        last_admit: now,
                        pending: vec![],
                    });
                st.next_chunk += 1;
                st.admitted += 1;
                st.finalized |= args.get("final").and_then(Json::as_bool).unwrap_or(false);
                st.last_admit = now;
                st.pending.push(id.to_owned());
                self.items.insert((stream, wire_id.to_owned()), 0);
            }
            "speech.backchannel" => {
                let stream = s(args, "stream").to_owned();
                self.streams.insert(
                    stream.clone(),
                    Stream {
                        owner: coglet.to_owned(),
                        backchannel: true,
                        next_chunk: 1,
                        admitted: 1,
                        finalized: true,
                        closed: false,
                        last_admit: now,
                        pending: vec![id.to_owned()],
                    },
                );
                self.items.insert((stream, wire_id.to_owned()), 0);
            }
            "speech.stop" => {
                if let Some(st) = self.streams.get_mut(s(args, "stream")) {
                    st.closed = true;
                    return std::mem::take(&mut st.pending);
                }
            }
            "speech.session_open" => {
                let sid = s(args, "session").to_owned();
                self.sessions.insert(
                    sid.clone(),
                    Session {
                        owner: coglet.to_owned(),
                        open: true,
                        ready: false,
                        calls: BTreeMap::new(),
                    },
                );
                self.active_session = Some(sid);
            }
            "speech.session_close" => self.close_session(s(args, "session")),
            "speech.tool_result" => {
                if let Some(x) = self.sessions.get_mut(s(args, "session"))
                    && let Some(c) = x.calls.get_mut(s(args, "call"))
                {
                    *c = Call::Resulted(result_digest(args));
                }
            }
            _ => {}
        }
        Vec::new()
    }

    fn close_session(&mut self, sid: &str) {
        if let Some(x) = self.sessions.get_mut(sid) {
            x.open = false;
            x.ready = false;
            // Closed calls are unknown to later results; late provider callbacks fence out.
            x.calls.clear();
        }
        if self.active_session.as_deref() == Some(sid) {
            self.active_session = None;
        }
        self.provider_streams.retain(|_, v| v != sid);
    }

    /// Is `id` the oldest pending chunk of its stream? (Chunks play, so complete, in order.)
    pub fn chunk_is_head(&self, op: &str, args: &Json, id: &str) -> bool {
        if !matches!(op, "speech.speak_chunk" | "speech.backchannel") {
            return true;
        }
        self.streams
            .get(s(args, "stream"))
            .is_none_or(|st| st.pending.first().is_none_or(|h| h == id))
    }

    /// Record a terminal result. Returns later chunk ids to cancel (gap rule).
    pub fn terminal(&mut self, op: &str, args: &Json, id: &str, ok: bool) -> Vec<String> {
        match op {
            "speech.speak_chunk" | "speech.backchannel" => {
                if let Some(st) = self.streams.get_mut(s(args, "stream")) {
                    st.pending.retain(|p| p != id);
                    if !ok {
                        st.closed = true;
                        return std::mem::take(&mut st.pending);
                    }
                    if st.finalized && st.pending.is_empty() {
                        st.closed = true;
                    }
                }
            }
            "speech.session_open" => {
                let sid = s(args, "session");
                if ok {
                    if let Some(x) = self.sessions.get_mut(sid) {
                        x.ready = x.open;
                    }
                } else {
                    self.close_session(sid);
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// Close unfinalized streams idle for `idle_us`. Returns the closed stream ids.
    pub fn close_idle(&mut self, now: u64, idle_us: u64) -> Vec<String> {
        let mut out = Vec::new();
        for (id, st) in &mut self.streams {
            if !st.closed && !st.finalized && now.saturating_sub(st.last_admit) >= idle_us {
                st.closed = true;
                out.push(id.clone());
            }
        }
        out
    }

    /// Owner lost its grant/resources (or kill/disconnect): close everything it owns and
    /// fence the sessions. Returns pending chunk ids the core must cancel.
    pub fn release_owner(&mut self, coglet: Option<&str>) -> Vec<String> {
        let mut cancel = Vec::new();
        for st in self.streams.values_mut() {
            if !st.closed && coglet.is_none_or(|c| c == st.owner) {
                st.closed = true;
                cancel.append(&mut st.pending);
            }
        }
        let sessions: Vec<String> = self
            .sessions
            .iter()
            .filter(|(_, x)| x.open && coglet.is_none_or(|c| c == x.owner))
            .map(|(k, _)| k.clone())
            .collect();
        for sid in sessions {
            self.close_session(&sid);
        }
        cancel
    }

    pub fn remove_pending(&mut self, id: &str) {
        for st in self.streams.values_mut() {
            st.pending.retain(|p| p != id);
        }
    }

    /// Normalize one native observation. Pure bookkeeping; the core applies delivery gating.
    pub fn normalize(&mut self, n: Native) -> Normalized {
        match n {
            Native::SpeechStart {
                utterance,
                position_ms,
            } => {
                if self.utterances.contains_key(&utterance) {
                    return self.discard("duplicate utterance id");
                }
                self.utterances
                    .insert(utterance.clone(), Utterance::default());
                self.utt_order.push_back(utterance.clone());
                while self.utt_order.len() > MAX_UTTERANCES {
                    if let Some(old) = self.utt_order.pop_front() {
                        // Evicting an id would allow reuse; only evict ended utterances.
                        if self.utterances.get(&old).is_some_and(|u| u.ended) {
                            self.utterances.remove(&old);
                        } else {
                            self.utt_order.push_back(old);
                            break;
                        }
                    }
                }
                Normalized::Event {
                    kind: "speech.start",
                    payload: json!({"utterance": utterance, "position_ms": position_ms}),
                    owner: None,
                }
            }
            Native::SpeechPartial {
                utterance,
                revision,
                text,
            } => {
                if text.len() > MAX_TRANSCRIPT_BYTES {
                    return self.discard("oversize transcript");
                }
                match self.utterances.get_mut(&utterance) {
                    Some(u) if !u.ended && revision > u.revision => {
                        u.revision = revision;
                        Normalized::Event {
                            kind: "speech.partial",
                            payload: json!({"utterance": utterance, "revision": revision, "text": text}),
                            owner: None,
                        }
                    }
                    _ => self.discard("late or stale partial"),
                }
            }
            Native::SpeechEnd {
                utterance,
                revision,
                text,
                position_ms,
            } => {
                if text.len() > MAX_TRANSCRIPT_BYTES {
                    return self.discard("oversize transcript");
                }
                match self.utterances.get_mut(&utterance) {
                    Some(u) if !u.ended && revision > u.revision => {
                        u.ended = true;
                        u.revision = revision;
                        Normalized::Event {
                            kind: "speech.end",
                            payload: json!({"utterance": utterance, "revision": revision, "text": text, "position_ms": position_ms}),
                            owner: None,
                        }
                    }
                    _ => self.discard("end without start, repeated end or stale revision"),
                }
            }
            Native::Position {
                stream,
                chunk,
                item,
                played_ms,
                position_ms,
                state,
            } => {
                let Some(owner) = self.stream_owner(&stream) else {
                    return self.discard("position for unknown stream");
                };
                if self
                    .stream_position
                    .get(&stream)
                    .is_some_and(|p| position_ms < *p)
                {
                    return self.discard("stream position regressed");
                }
                match self.items.get_mut(&(stream.clone(), item.clone())) {
                    Some(p) => {
                        if played_ms < *p {
                            return self.discard("item played_ms regressed");
                        }
                        *p = played_ms;
                    }

                    None => {
                        // Only provider-originated items may be introduced by native code.
                        if !self.provider_streams.contains_key(&stream) {
                            return self.discard("position for unknown item");
                        }
                        self.items.insert((stream.clone(), item.clone()), played_ms);
                    }
                }
                self.stream_position.insert(stream.clone(), position_ms);
                Normalized::Event {
                    kind: "speech.position",
                    payload: json!({"stream": stream, "chunk": chunk, "item": item, "played_ms": played_ms, "position_ms": position_ms, "state": state}),
                    owner: Some(owner),
                }
            }
            Native::BargeIn {
                utterance,
                stream,
                item,
                played_ms,
                position_ms,
                reflex,
            } => {
                if !self.utterances.contains_key(&utterance) {
                    return self.discard("barge-in without a started utterance");
                }
                let Some(owner) = self.stream_owner(&stream) else {
                    return self.discard("barge-in without an active stream");
                };
                let active = self.streams.get(&stream).is_some_and(|st| !st.closed)
                    || self.provider_streams.contains_key(&stream);
                if !active {
                    return self.discard("barge-in without an active stream");
                }
                let provider = self.provider_streams.contains_key(&stream);
                match self.items.get_mut(&(stream.clone(), item.clone())) {
                    Some(p) if played_ms >= *p => *p = played_ms,
                    None if provider => {
                        self.items.insert((stream.clone(), item.clone()), played_ms);
                    }
                    _ => return self.discard("barge-in item unknown or regressed"),
                }
                self.stream_position
                    .entry(stream.clone())
                    .and_modify(|p| *p = (*p).max(position_ms))
                    .or_insert(position_ms);
                Normalized::Event {
                    kind: "speech.barge_in",
                    payload: json!({"utterance": utterance, "stream": stream, "item": item, "played_ms": played_ms, "position_ms": position_ms, "reflex": reflex}),
                    owner: Some(owner),
                }
            }
            Native::ToolCall {
                session,
                call,
                name,
                arguments,
            } => {
                let allowed = self.tool_allow.contains(&name);
                match self.sessions.get_mut(&session) {
                    Some(x) if x.open && x.ready && allowed => {
                        if x.calls.contains_key(&call) {
                            return self.discard("duplicate tool call");
                        }
                        if x.calls.len() >= MAX_CALLS_PER_SESSION {
                            return self.discard("tool-call table full");
                        }
                        x.calls.insert(call.clone(), Call::Pending);
                        let owner = x.owner.clone();
                        Normalized::Event {
                            kind: "speech.tool_call",
                            payload: json!({"session": session, "call": call, "name": name, "arguments": arguments}),
                            owner: Some(owner),
                        }
                    }
                    _ => self.discard("tool call for closed/unknown session or unlisted tool"),
                }
            }
            Native::SessionEnded { session } => {
                self.close_session(&session);
                Normalized::Discarded("session ended")
            }
        }
    }

    /// Called by the core when the reflex applied to the stream is a stop.
    pub fn reflex_stopped(&mut self, stream: &str) -> Vec<String> {
        match self.streams.get_mut(stream) {
            Some(st) => {
                st.closed = true;
                std::mem::take(&mut st.pending)
            }
            None => Vec::new(),
        }
    }

    pub fn register_provider_stream(&mut self, stream: &str) {
        if let Some(sid) = self.active_session.clone() {
            self.provider_streams
                .entry(stream.to_owned())
                .or_insert(sid);
        }
    }

    fn stream_owner(&mut self, stream: &str) -> Option<String> {
        if let Some(st) = self.streams.get(stream) {
            return Some(st.owner.clone());
        }
        self.register_provider_stream(stream);
        self.provider_streams
            .get(stream)
            .and_then(|sid| self.sessions.get(sid))
            .map(|x| x.owner.clone())
    }

    fn discard(&mut self, why: &'static str) -> Normalized {
        self.discarded += 1;
        Normalized::Discarded(why)
    }
}

fn result_digest(args: &Json) -> String {
    // serde_json maps are ordered (BTreeMap), so this is canonical for equal values.
    serde_json::to_string(&json!([args.get("ok"), args.get("value")])).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    fn sid(n: u8) -> String {
        format!("{n:02x}").repeat(16)
    }
    fn chunk(stream: &str, n: u64, fin: bool) -> Json {
        json!({"stream": stream, "chunk": n, "text": "hi", "final": fin})
    }

    #[test]
    fn contiguous_chunks_limit_and_final() {
        let mut sp = SpeechState::default();
        let st = sid(1);
        for n in 0..64 {
            let a = chunk(&st, n, false);
            sp.check("speech.speak_chunk", C, &a, None, &[]).unwrap();
            sp.admit("speech.speak_chunk", C, &format!("{n:032x}"), &a, 0);
        }
        let e = sp
            .check("speech.speak_chunk", C, &chunk(&st, 64, false), None, &[])
            .unwrap_err();
        assert_eq!(e.code, crate::error::Code::ResourceExhausted);
        // gap/reuse
        let e = sp
            .check("speech.speak_chunk", C, &chunk(&st, 70, false), None, &[])
            .unwrap_err();
        assert_eq!(e.code, crate::error::Code::Conflict);
    }

    #[test]
    fn utf8_bound_is_bytes_not_code_points() {
        let sp = SpeechState::default();
        let a = json!({"stream": sid(1), "chunk": 0, "text": "é".repeat(2049), "final": true});
        assert!(sp.check("speech.speak_chunk", C, &a, None, &[]).is_err());
        let a = json!({"stream": sid(1), "chunk": 0, "text": "é".repeat(2048), "final": true});
        sp.check("speech.speak_chunk", C, &a, None, &[]).unwrap();
    }

    #[test]
    fn utterance_filter_discards_stale_and_late() {
        let mut sp = SpeechState::default();
        let u = sid(9);
        let ev = |n| matches!(n, Normalized::Event { .. });
        assert!(ev(sp.normalize(Native::SpeechStart {
            utterance: u.clone(),
            position_ms: 0
        })));
        assert!(ev(sp.normalize(Native::SpeechPartial {
            utterance: u.clone(),
            revision: 2,
            text: "a".into()
        })));
        assert!(!ev(sp.normalize(Native::SpeechPartial {
            utterance: u.clone(),
            revision: 2,
            text: "b".into()
        })));
        assert!(!ev(sp.normalize(Native::SpeechEnd {
            utterance: u.clone(),
            revision: 2,
            text: "x".into(),
            position_ms: 1
        })));
        assert!(ev(sp.normalize(Native::SpeechEnd {
            utterance: u.clone(),
            revision: 3,
            text: "x".into(),
            position_ms: 1
        })));
        assert!(!ev(sp.normalize(Native::SpeechPartial {
            utterance: u.clone(),
            revision: 9,
            text: "late".into()
        })));
        assert!(!ev(sp.normalize(Native::SpeechEnd {
            utterance: u.clone(),
            revision: 10,
            text: "again".into(),
            position_ms: 2
        })));
        assert!(!ev(sp.normalize(Native::SpeechStart {
            utterance: u,
            position_ms: 0
        })));
        assert_eq!(sp.discarded, 5);
    }
}
