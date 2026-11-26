//! Runtime-side mock peer for the live `SoftmaxAppSurface` wire proof.
//!
//! `app_surface_peer <unix-socket-path>` connects to ONE private Unix socket, plays the
//! runtime side of surface 0.2 (hello_reply 0.1, then 0.2 frames), verifies the app's
//! capabilities and grants against the pinned catalog, and then executes JSON-line
//! commands from stdin. Every validated incoming frame is printed as one JSON line.
//! Any malformed frame/command, EOF, mismatch or silence over 3 s exits non-zero.
//! No reconnect, no other endpoint, no grant is ever invented: grants come from the app.
//!
//! stdin commands (<= 64 KiB, unknown and duplicate keys refused at every depth;
//! `operation` is the FULL catalog name, e.g. `softmax.app.navigate`; `id` is 32 lowercase hex):
//!   {"kind":"act","operation":..,"args":{..},"id":..}   {"kind":"read",..same..}
//!   {"kind":"retry","id":..} (re-sends the EXACT stored act bytes)   {"kind":"close"}
//! `close` waits until every retained command is answered (accept+outcome / read_reply);
//! a command unanswered after 6 s local time fails the peer. stdout is drained by a bounded
//! writer thread: a stalled consumer fails the peer closed instead of stalling the socket loop.
//! `act invoke_sensitive` is an explicit negative control only: it is sent without any
//! grant and the peer fails if the app ever accepts or succeeds it.

use coglet_surface_rust::codec::to_profile;
use coglet_surface_rust::transport::{FrameDecoder, write_frame};
use coglet_types::Value;
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::fs::FileTypeExt;
use std::os::unix::net::UnixStream;
use std::process::ExitCode;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::time::{Duration, Instant};

const CATALOG: &[u8] = include_bytes!("../../../apps/softmax-mac/Surface/catalog.v1.json");
const PIN: &str = "3b2ad502dd3d111b9f6fcf832a51fccff2b8582348de8f9e354e75f2019354f9";
const ENTRY: &str = "softmax.app";
const RESOURCE: &str = "softmax.app_control";
const SENSITIVE: &str = "softmax.app.invoke_sensitive";
const S01: &str = "surface::surface-message";
const S02: &str = "surface02::s02-surface-message";
const MAX_LINE: usize = 64 * 1024;
const MAX_RETAINED: usize = 64;
const MAX_QUEUE: usize = 128;
const DEADLINE_AHEAD_US: u64 = 5_000_000;
const HEALTH_EVERY: Duration = Duration::from_millis(500);
const SILENCE: Duration = Duration::from_secs(3);
const WRITE_STALL: Duration = Duration::from_secs(1);
const PENDING_WINDOW: Duration = Duration::from_secs(6);
const STDOUT_QUEUE: usize = 512;
const STDOUT_DRAIN: Duration = Duration::from_secs(2);

fn owner() -> String {
    format!("coglet:{}", "b".repeat(64))
}

// Use the workspace's existing SHA-256 implementation, not proof-local crypto.
fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(data))
}

// ------------------------------------------------------------------ JSON / CBOR bridge
/// Explicit conversion: integers only as u64 or i64, text keys only, no floats/bytes/tags.
fn to_json(v: &Value) -> Result<Json, String> {
    Ok(match v {
        Value::Null => Json::Null,
        Value::Bool(b) => Json::Bool(*b),
        Value::Text(t) => Json::String(t.clone()),
        Value::Integer(i) => {
            let n = i128::from(*i);
            if let Ok(u) = u64::try_from(n) {
                Json::Number(u.into())
            } else if let Ok(s) = i64::try_from(n) {
                Json::Number(s.into())
            } else {
                return Err("integer outside u64/i64".into());
            }
        }
        Value::Array(a) => Json::Array(a.iter().map(to_json).collect::<Result<_, _>>()?),
        Value::Map(m) => {
            let mut o = serde_json::Map::new();
            for (k, x) in m {
                let Value::Text(k) = k else { return Err("non-text map key".into()) };
                if o.insert(k.clone(), to_json(x)?).is_some() {
                    return Err("duplicate map key".into());
                }
            }
            Json::Object(o)
        }
        _ => return Err("unsupported CBOR value".into()),
    })
}

fn encode_msg(rule: &str, msg: &Json) -> Result<Vec<u8>, String> {
    coglet_types::encode(rule, &to_profile(msg)?)
}
fn get<'a>(j: &'a Json, k: &str) -> Result<&'a Json, String> {
    j.get(k).ok_or_else(|| format!("missing `{k}`"))
}
fn text<'a>(j: &'a Json, k: &str) -> Result<&'a str, String> {
    get(j, k)?.as_str().ok_or_else(|| format!("`{k}` is not text"))
}
fn uint(j: &Json, k: &str) -> Result<u64, String> {
    get(j, k)?.as_u64().ok_or_else(|| format!("`{k}` is not a u64"))
}
fn is_id(s: &str) -> bool {
    s.len() == 32 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

// ------------------------------------------------------------------ pinned catalog
struct Op {
    direction: String,
    entry: String,
    input: String,
    output: Option<String>,
    resources: Vec<String>,
}
struct Catalog {
    ops: BTreeMap<String, Op>,
    rules: String,
    schemas: BTreeMap<String, String>,
}
impl Catalog {
    fn load() -> Result<Self, String> {
        if sha256_hex(CATALOG) != PIN {
            return Err("catalog sha256 does not match the pin".into());
        }
        let j: Json = serde_json::from_slice(CATALOG).map_err(|e| e.to_string())?;
        let strings = |v: &Json| -> Result<Vec<String>, String> {
            v.as_array()
                .ok_or("not an array")?
                .iter()
                .map(|s| s.as_str().map(str::to_owned).ok_or("not text".to_owned()))
                .collect()
        };
        let mut ops = BTreeMap::new();
        for (name, o) in j["operations"].as_object().ok_or("catalog operations")? {
            ops.insert(
                name.clone(),
                Op {
                    direction: text(o, "direction")?.into(),
                    entry: text(o, "entry")?.into(),
                    input: text(o, "input")?.into(),
                    output: o.get("output").and_then(Json::as_str).map(str::to_owned),
                    resources: strings(get(o, "resources")?)?,
                },
            );
        }
        let schemas: BTreeMap<String, String> = j["schemas"]
            .as_object()
            .ok_or("catalog schemas")?
            .iter()
            .map(|(k, v)| Ok((k.clone(), v.as_str().ok_or("schema rule")?.to_owned())))
            .collect::<Result<_, String>>()?;
        let c = Catalog { ops, rules: serde_json::to_string(&j["rules"]).map_err(|e| e.to_string())?, schemas };
        for o in c.ops.values() {
            for uri in std::iter::once(&o.input).chain(o.output.iter()) {
                if !c.schemas.contains_key(uri) {
                    return Err(format!("catalog schema URI {uri} has no rule"));
                }
            }
        }
        Ok(c)
    }
    /// Validate a JSON value (integer profile enforced) against a catalog schema URI.
    fn check(&self, uri: &str, v: &Json) -> Result<(), String> {
        let rule = self.schemas.get(uri).ok_or("unknown schema URI")?;
        coglet_types::validate_with_schema(&self.rules, rule, &to_profile(v)?)
    }
    /// Ordinary (non-sensitive) operations of the `softmax.app` entry with a direction.
    fn ordinary(&self, direction: &str) -> Vec<String> {
        let mut v: Vec<String> = (self.ops.iter())
            .filter(|(_, o)| o.entry == ENTRY && o.direction == direction)
            .map(|(n, _)| n.clone())
            .collect();
        v.sort();
        v
    }
    fn schema_uri(&self, op: &str) -> &str {
        let o = &self.ops[op];
        if o.direction == "read" { o.output.as_deref().unwrap_or(&o.input) } else { &o.input }
    }
    fn expected_caps_entry(&self) -> Json {
        let all: Vec<String> = ["event", "read", "action"].iter().flat_map(|d| self.ordinary(d)).collect();
        let exclusive: BTreeSet<&String> = all.iter().flat_map(|n| &self.ops[n].resources).collect();
        let schemas: serde_json::Map<String, Json> =
            all.iter().map(|n| (n.clone(), json!(self.schema_uri(n)))).collect();
        json!({"name": ENTRY, "events": self.ordinary("event"), "reads": self.ordinary("read"),
               "actions": self.ordinary("action"), "exclusive": exclusive, "schemas": schemas})
    }
    fn resolve(&self, name: &str) -> Result<String, String> {
        // Full catalog names only: the command log must be exact.
        if self.ops.contains_key(name) { Ok(name.to_owned()) } else { Err(format!("unknown operation `{name}`")) }
    }
}

// ------------------------------------------------------------------ commands
#[derive(Debug, PartialEq)]
enum Cmd {
    Act { op: String, args: Json, id: String },
    Read { op: String, args: Json, id: String },
    Retry { id: String },
    Close,
}

/// Validation-only deserializer that refuses a repeated object key at any depth. It parses
/// independently of `serde_json::Value`, so arbitrary-precision numbers (a private one-key
/// map in serde_json) just visit as a map with a single key.
struct NoDup;
struct NoDupVisitor;
impl<'de> serde::Deserialize<'de> for NoDup {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        d.deserialize_any(NoDupVisitor)
    }
}
impl<'de> serde::de::Visitor<'de> for NoDupVisitor {
    type Value = NoDup;
    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("any JSON value")
    }
    fn visit_bool<E>(self, _: bool) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_i64<E>(self, _: i64) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_u64<E>(self, _: u64) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_f64<E>(self, _: f64) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_str<E>(self, _: &str) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_unit<E>(self) -> Result<NoDup, E> { Ok(NoDup) }
    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut a: A) -> Result<NoDup, A::Error> {
        while a.next_element::<NoDup>()?.is_some() {}
        Ok(NoDup)
    }
    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut a: A) -> Result<NoDup, A::Error> {
        let mut seen = BTreeSet::new();
        while let Some(k) = a.next_key::<String>()? {
            if !seen.insert(k.clone()) {
                return Err(serde::de::Error::custom(format!("duplicate key `{k}`")));
            }
            a.next_value::<NoDup>()?;
        }
        Ok(NoDup)
    }
}

fn parse_cmd(line: &[u8]) -> Result<Cmd, String> {
    if line.len() > MAX_LINE {
        return Err("command line over 64 KiB".into());
    }
    let mut de = serde_json::Deserializer::from_slice(line);
    serde::Deserialize::deserialize(&mut de).map(|_: NoDup| ()).map_err(|e| format!("bad command JSON: {e}"))?;
    de.end().map_err(|e| format!("bad command JSON: {e}"))?;
    let j: Json = serde_json::from_slice(line).map_err(|e| format!("bad command JSON: {e}"))?;
    let obj = j.as_object().ok_or("command must be an object")?;
    let allowed: &[&str] = match text(&j, "kind")? {
        "act" | "read" => &["kind", "operation", "args", "id"],
        "retry" => &["kind", "id"],
        "close" => &["kind"],
        k => return Err(format!("unknown command kind `{k}`")),
    };
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(format!("unknown command key `{k}`"));
    }
    if obj.len() != allowed.len() {
        return Err("command has missing keys".into());
    }
    let kind = text(&j, "kind")?;
    if kind == "close" {
        return Ok(Cmd::Close);
    }
    let id = text(&j, "id")?.to_owned();
    if !is_id(&id) {
        return Err("id must be 32 lowercase hex".into());
    }
    if kind == "retry" {
        return Ok(Cmd::Retry { id });
    }
    let (op, args) = (text(&j, "operation")?.to_owned(), get(&j, "args")?.clone());
    if !args.is_object() {
        return Err("args must be an object".into());
    }
    Ok(if kind == "act" { Cmd::Act { op, args, id } } else { Cmd::Read { op, args, id } })
}

/// Reads commands (one JSON line each) until `close`, EOF or error; never blocks the socket loop.
fn read_commands(mut input: impl BufRead, tx: SyncSender<Result<Cmd, String>>) {
    loop {
        let mut line = Vec::new();
        let n = (&mut input).take(MAX_LINE as u64 + 2).read_until(b'\n', &mut line);
        let res = match n {
            Err(e) => Err(format!("stdin: {e}")),
            Ok(0) => Err("stdin EOF without close".into()),
            Ok(_) if line.last() != Some(&b'\n') => {
                Err(if line.len() > MAX_LINE { "command line over 64 KiB" } else { "stdin EOF inside command" }.into())
            }
            Ok(_) => parse_cmd(&line[..line.len() - 1]),
        };
        let stop = !matches!(res, Ok(Cmd::Act { .. } | Cmd::Read { .. } | Cmd::Retry { .. }));
        if tx.send(res).is_err() || stop {
            return;
        }
    }
}

// ------------------------------------------------------------------ peer state machine
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    AwaitHello,
    AwaitCaps,
    AwaitGrants,
    Ready,
}
struct Retained {
    op: String,
    is_read: bool,
    negative: bool,
    raw: Vec<u8>,
    retries: u32,
    accepts: u32,
    finals: u32,
    sent_at: Instant,
    accepted: Option<bool>,
    accept_body: Option<Json>,
    first_final: Option<Json>,
}
impl Retained {
    /// Answered = every send (original + retries) has its accept/outcome (or read_reply);
    /// a rejected act gets no outcome.
    fn answered(&self) -> bool {
        let sends = self.retries + 1;
        if self.is_read {
            return self.finals >= 1;
        }
        self.accepts >= sends && (self.accepted == Some(false) || self.finals >= sends)
    }
}
struct Incoming {
    json: Json,
    out: Vec<Vec<u8>>,
    fatal: Option<String>,
}
struct Peer {
    cat: Catalog,
    phase: Phase,
    surface: String,
    hello_corr: String,
    hello_clock: u64,
    clock: u64,
    epoch: u64,
    expires: u64,
    granted: BTreeSet<String>,
    retained: BTreeMap<String, Retained>,
    last_seq: Option<u64>,
    last_logical: u64,
    last_revision: u64,
    pending_window: Duration,
    health_clock: u64,
    health_n: u64,
}

impl Peer {
    fn new(cat: Catalog, pending_window: Duration) -> Self {
        Peer {
            cat,
            phase: Phase::AwaitHello,
            surface: String::new(),
            hello_corr: String::new(),
            hello_clock: 0,
            clock: 0,
            epoch: 0,
            expires: 0,
            granted: BTreeSet::new(),
            retained: BTreeMap::new(),
            last_seq: None,
            last_logical: 0,
            last_revision: 0,
            pending_window,
            health_clock: 0,
            health_n: 0,
        }
    }
    fn ready(&self) -> bool {
        self.phase == Phase::Ready
    }
    fn advance_clock(&mut self, c: u64) -> Result<(), String> {
        if c < self.clock {
            return Err("surface clock went backwards".into());
        }
        self.clock = c;
        Ok(())
    }

    /// Validate one incoming frame; returns the line to print and frames to send.
    fn on_frame(&mut self, raw: &[u8]) -> Result<Incoming, String> {
        let rule = if self.phase == Phase::AwaitHello { S01 } else { S02 };
        let msg = to_json(&coglet_types::decode(rule, raw)?)?;
        let (ty, b) = (text(&msg, "type")?.to_owned(), get(&msg, "body")?.clone());
        let (mut out, mut fatal) = (Vec::new(), None);
        match (self.phase, ty.as_str()) {
            (Phase::AwaitHello, "hello") => {
                out.push(self.hello(&b)?);
                self.phase = Phase::AwaitCaps;
            }
            (Phase::AwaitHello, t) => return Err(format!("first frame must be hello, got {t}")),
            (_, "health") => self.advance_clock(uint(&b, "clock")?)?,
            (_, "error") => fatal = Some(format!("surface sent error: {b}")),
            (Phase::AwaitCaps, "capabilities") => {
                self.caps(&b)?;
                self.phase = Phase::AwaitGrants;
            }
            (Phase::AwaitGrants, "grants") => {
                self.grants(&b)?;
                self.phase = Phase::Ready;
            }
            (Phase::Ready, "event") => self.event(&b)?,
            (Phase::Ready, "accept") => self.accept(&b)?,
            (Phase::Ready, "read_reply") => self.read_reply(&b)?,
            (p, t) => return Err(format!("{t} is invalid in phase {p:?}")),
        }
        Ok(Incoming { json: msg, out, fatal })
    }

    fn hello(&mut self, b: &Json) -> Result<Vec<u8>, String> {
        let offered = get(get(b, "offer")?, "versions")?.as_array().ok_or("versions")?;
        if !offered.iter().any(|v| v == "0.2") {
            return Err("surface did not offer 0.2".into());
        }
        self.surface = text(b, "surface")?.into();
        self.hello_corr = text(b, "correlation_id")?.into();
        self.hello_clock = uint(b, "clock")?;
        self.clock = self.hello_clock;
        self.health_clock = self.hello_clock;
        // The hello_reply is still decoded by the surface under the 0.1 profile.
        encode_msg(
            S01,
            &json!({"type": "hello_reply", "body": {"selection": {"version": "0.2"}, "correlation_id": self.hello_corr}}),
        )
    }

    fn caps(&mut self, b: &Json) -> Result<(), String> {
        self.advance_clock(uint(b, "clock")?)?;
        if get(b, "entries")? != &json!([self.cat.expected_caps_entry()]) {
            return Err(format!("capabilities differ from the ordinary pinned catalog: {}", get(b, "entries")?));
        }
        Ok(())
    }

    fn grants(&mut self, b: &Json) -> Result<(), String> {
        let entries = get(b, "entries")?.as_array().ok_or("entries")?;
        let [g] = entries.as_slice() else { return Err("grants must hold exactly one entry".into()) };
        if text(g, "coglet")? != owner() {
            return Err("grant owner mismatch".into());
        }
        if get(g, "resources")? != &json!([RESOURCE]) {
            return Err("grant resources must be exactly [softmax.app_control]".into());
        }
        let ops: Vec<&str> =
            get(g, "operations")?.as_array().ok_or("operations")?.iter().map(|o| o.as_str().ok_or("op")).collect::<Result<_, _>>()?;
        let set: BTreeSet<String> = ops.iter().map(|s| s.to_string()).collect();
        let ordinary: BTreeSet<String> = ["event", "read", "action"].iter().flat_map(|d| self.cat.ordinary(d)).collect();
        if set.len() != ops.len() || set != ordinary {
            return Err("grant operations are not the ordinary catalog operations".into());
        }
        let expires = uint(g, "expires")?;
        if expires <= self.clock {
            return Err("grant already expired".into());
        }
        (self.epoch, self.expires, self.granted) = (uint(b, "epoch")?, expires, set);
        Ok(())
    }

    fn event(&mut self, b: &Json) -> Result<(), String> {
        if text(b, "source")? != self.surface {
            return Err("event source is not the surface".into());
        }
        let seq = uint(b, "seq")?;
        if self.last_seq.is_some_and(|l| seq <= l) {
            return Err("event seq not increasing".into());
        }
        let logical = uint(b, "logical_time")?;
        if logical < self.last_logical {
            return Err("event logical_time went backwards".into());
        }
        self.last_seq = Some(seq);
        self.last_logical = logical;
        let (kind, p) = (text(b, "kind")?, get(b, "payload")?);
        if kind == "action.outcome" {
            return self.outcome(p, text(b, "correlation_id")?);
        }
        match self.cat.ops.get(kind) {
            Some(o) if o.direction == "event" && o.entry == ENTRY => {
                self.cat.check(&o.input, p)?;
                if let Some(rev) = p.get("revision").and_then(Json::as_u64) {
                    if rev < self.last_revision {
                        return Err("event revision went backwards".into());
                    }
                    self.last_revision = rev;
                }
                Ok(())
            }
            _ => Err(format!("unexpected event kind `{kind}`")),
        }
    }

    fn outcome(&mut self, p: &Json, event_corr: &str) -> Result<(), String> {
        coglet_types::validate("common::action-outcome", &to_profile(p)?)?;
        let id = text(p, "id")?;
        if text(p, "correlation_id")? != id || event_corr != id {
            return Err("outcome correlation differs from the action".into());
        }
        let r = self.retained.get_mut(id).filter(|r| !r.is_read).ok_or("outcome for an unknown action")?;
        let status = text(p, "status")?;
        if r.accepted != Some(true) {
            return Err("outcome without a prior accept{accepted:true}".into());
        }
        if r.finals + 1 > r.retries + 1 || r.first_final.as_ref().is_some_and(|f| f != p) {
            return Err("unexpected repeated or changed outcome".into());
        }
        if status == "succeeded" {
            if r.negative {
                return Err("sensitive operation succeeded without a grant".into());
            }
            let uri = self.cat.ops[&r.op].output.as_deref().ok_or("no output schema")?;
            self.cat.check(uri, get(p, "result")?)?;
        }
        r.finals += 1;
        r.first_final.get_or_insert_with(|| p.clone());
        Ok(())
    }

    fn accept(&mut self, b: &Json) -> Result<(), String> {
        let id = text(b, "id")?;
        let r = self.retained.get_mut(id).filter(|r| !r.is_read).ok_or("accept for an unknown action")?;
        let next = r.accepts + 1;
        if text(b, "correlation_id")? != id || next > r.retries + 1 {
            return Err("accept correlation or count mismatch".into());
        }
        let accepted = get(b, "accepted")?.as_bool().ok_or("`accepted` is not a bool")?;
        if r.negative && accepted {
            return Err("sensitive operation accepted without a grant".into());
        }
        if r.accept_body.as_ref().is_some_and(|prev| prev != b) {
            return Err("repeated accept differs from the first accept".into());
        }
        r.accepts = next;
        r.accepted = Some(accepted);
        r.accept_body.get_or_insert_with(|| b.clone());
        r.sent_at = Instant::now();
        Ok(())
    }

    fn read_reply(&mut self, b: &Json) -> Result<(), String> {
        let id = text(b, "correlation_id")?;
        let r = self.retained.get_mut(id).filter(|r| r.is_read).ok_or("read_reply for an unknown read")?;
        if r.finals > 0 {
            return Err("duplicate read_reply".into());
        }
        if get(b, "ok")? == &json!(true) {
            let uri = self.cat.ops[&r.op].output.as_deref().ok_or("no output schema")?;
            self.cat.check(uri, get(b, "value")?)?;
        }
        r.finals = 1;
        Ok(())
    }

    /// Build the act/read frame (or exact retry bytes) for a stdin command.
    fn command(&mut self, cmd: Cmd) -> Result<Vec<u8>, String> {
        if !self.ready() {
            return Err("commands are refused before capabilities and grants are verified".into());
        }
        let (op, args, id, is_read) = match cmd {
            Cmd::Retry { id } => {
                let r = self.retained.get_mut(&id).filter(|r| !r.is_read).ok_or("retry of an unknown act")?;
                r.retries += 1;
                r.sent_at = Instant::now();
                return Ok(r.raw.clone());
            }
            Cmd::Close => return Err("close is handled by the loop".into()),
            Cmd::Act { op, args, id } => (op, args, id, false),
            Cmd::Read { op, args, id } => (op, args, id, true),
        };
        if self.retained.len() >= MAX_RETAINED {
            return Err("retained command limit reached".into());
        }
        if self.retained.contains_key(&id) {
            return Err("id already used by a retained command".into());
        }
        let op = self.cat.resolve(&op)?;
        let want = if is_read { "read" } else { "action" };
        if self.cat.ops[&op].direction != want {
            return Err(format!("{op} is not a {want}"));
        }
        let negative = !is_read && op == SENSITIVE;
        if !negative && (!self.granted.contains(&op) || self.clock >= self.expires) {
            return Err(format!("{op} is not granted (or the grant expired); refusing locally"));
        }
        self.cat.check(&self.cat.ops[&op].input, &args)?;
        let deadline = self.clock.checked_add(DEADLINE_AHEAD_US).ok_or("deadline overflows u64")?;
        let body = if is_read {
            json!({"coglet": owner(), "capability": op, "args": args, "grant_epoch": self.epoch,
                   "deadline": deadline, "correlation_id": id})
        } else {
            json!({"grant_epoch": self.epoch, "action": {"id": id, "coglet": owner(), "capability": op,
                   "args": args, "deadline": deadline, "correlation_id": id}})
        };
        let raw = encode_msg(S02, &json!({"type": if is_read { "read" } else { "act" }, "body": body}))?;
        self.retained.insert(id, Retained { op, is_read, negative, raw: raw.clone(), retries: 0, accepts: 0, finals: 0, sent_at: Instant::now(), accepted: None, accept_body: None, first_final: None });
        Ok(raw)
    }

    fn unanswered(&self) -> Vec<&str> {
        self.retained.iter().filter(|(_, r)| !r.answered()).map(|(id, _)| id.as_str()).collect()
    }
    /// Fails if any command has had no answer for longer than the pending window.
    fn check_pending(&self, now: Instant) -> Result<(), String> {
        for (id, r) in &self.retained {
            if !r.answered() && now.saturating_duration_since(r.sent_at) > self.pending_window {
                return Err(format!("no answer to command {id} within {:?}", self.pending_window));
            }
        }
        Ok(())
    }

    /// Health clock: surface hello clock + local elapsed, never decreasing.
    fn health(&mut self, elapsed_us: u64) -> Result<Vec<u8>, String> {
        if self.phase == Phase::AwaitHello {
            return Err("health before hello".into());
        }
        let c = self.hello_clock.checked_add(elapsed_us).ok_or("health clock overflows u64")?;
        self.health_clock = self.health_clock.max(c);
        self.health_n += 1;
        let corr = format!("ffff{:028x}", self.health_n);
        encode_msg(S02, &json!({"type": "health", "body": {"state": "ready", "clock": self.health_clock, "pending": 0, "correlation_id": corr}}))
    }
}

// ------------------------------------------------------------------ socket loop
fn enqueue(q: &mut VecDeque<Vec<u8>>, body: &[u8]) -> Result<(), String> {
    if q.len() >= MAX_QUEUE {
        return Err("outgoing queue full".into());
    }
    let mut framed = Vec::new();
    write_frame(&mut framed, body).map_err(|e| e.to_string())?;
    q.push_back(framed);
    Ok(())
}

fn emit(tx: &SyncSender<String>, line: String) -> Result<(), String> {
    tx.try_send(line).map_err(|e| match e {
        TrySendError::Full(_) => "stdout consumer stalled (output queue full)".to_owned(),
        TrySendError::Disconnected(_) => "stdout writer stopped".to_owned(),
    })
}

/// Stdout is owned by a writer thread behind a bounded queue, so a blocked consumer can
/// never stall the socket loop. Clean exit waits (bounded) for the queue to drain; a failure
/// exit waits only briefly and never joins a thread that may be blocked in `write`.
fn run(
    s: UnixStream,
    cmds: Receiver<Result<Cmd, String>>,
    out: impl Write + Send + 'static,
    pending: Duration,
) -> Result<(), String> {
    let (tx, rx) = sync_channel::<String>(STDOUT_QUEUE);
    let (done_tx, done_rx) = sync_channel::<Result<(), String>>(1);
    std::thread::spawn(move || {
        let mut out = out;
        let res = rx.iter().try_for_each(|l| out.write_all(l.as_bytes()).and_then(|()| out.flush())).map_err(|e| format!("stdout: {e}"));
        let _ = done_tx.send(res);
    });
    let result = run_loop(s, cmds, &tx, pending);
    drop(tx);
    match result {
        Ok(()) => done_rx.recv_timeout(STDOUT_DRAIN).map_err(|_| "stdout consumer stalled at close".to_owned())?,
        Err(m) => {
            let _ = done_rx.recv_timeout(Duration::from_millis(300));
            Err(m)
        }
    }
}

/// Single-owner nonblocking loop: partial writes stay queued, health runs regardless of stdin.
fn run_loop(mut s: UnixStream, cmds: Receiver<Result<Cmd, String>>, out: &SyncSender<String>, pending: Duration) -> Result<(), String> {
    let e = |e: io::Error| e.to_string();
    s.set_nonblocking(true).map_err(e)?;
    let mut peer = Peer::new(Catalog::load()?, pending);
    let (mut dec, mut q, mut off) = (FrameDecoder::default(), VecDeque::<Vec<u8>>::new(), 0usize);
    let (mut hello_at, mut last_health, mut last_rx) = (None::<Instant>, Instant::now(), Instant::now());
    let (mut stalled, mut closing) = (None::<Instant>, false);
    loop {
        let mut busy = false;
        let mut buf = [0u8; 16384];
        match s.read(&mut buf) {
            Ok(0) => {
                dec.finish().map_err(e)?;
                return Err("surface closed the connection".into());
            }
            Ok(n) => {
                busy = true;
                for f in dec.push(&buf[..n]).map_err(e)? {
                    let inc = peer.on_frame(&f)?;
                    emit(out, serde_json::to_string(&inc.json).map_err(|x| x.to_string())? + "\n")?;
                    if let Some(m) = inc.fatal {
                        return Err(m);
                    }
                    for b in &inc.out {
                        enqueue(&mut q, b)?;
                        hello_at.get_or_insert(Instant::now());
                    }
                    last_rx = Instant::now();
                }
            }
            Err(x) if matches!(x.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted) => {}
            Err(x) => return Err(x.to_string()),
        }
        if last_rx.elapsed() > SILENCE {
            return Err("health timeout: surface silent for 3 s".into());
        }
        peer.check_pending(Instant::now())?;
        if peer.ready() && q.len() < MAX_QUEUE / 2 && !closing {
            match cmds.try_recv() {
                Ok(Ok(Cmd::Close)) => closing = true,
                Ok(Ok(c)) => {
                    enqueue(&mut q, &peer.command(c)?)?;
                    busy = true;
                }
                Ok(Err(m)) => return Err(m),
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => return Err("command reader stopped".into()),
            }
        }
        if let Some(t) = hello_at.filter(|_| last_health.elapsed() >= HEALTH_EVERY) {
            let micros = u64::try_from(t.elapsed().as_micros()).map_err(|_| "elapsed overflow")?;
            enqueue(&mut q, &peer.health(micros)?)?;
            last_health = Instant::now();
        }
        while let Some(front) = q.front() {
            match s.write(&front[off..]) {
                Ok(0) => return Err("socket write returned 0".into()),
                Ok(n) => {
                    (busy, stalled, off) = (true, None, off + n);
                    if off == front.len() {
                        q.pop_front();
                        off = 0;
                    }
                }
                Err(x) if x.kind() == io::ErrorKind::WouldBlock => {
                    if stalled.get_or_insert_with(Instant::now).elapsed() > WRITE_STALL {
                        return Err("socket write stalled for 1 s".into());
                    }
                    break;
                }
                Err(x) if x.kind() == io::ErrorKind::Interrupted => {}
                Err(x) => return Err(x.to_string()),
            }
        }
        if q.is_empty() {
            stalled = None;
            if closing && peer.unanswered().is_empty() {
                let _ = s.shutdown(std::net::Shutdown::Both);
                return Ok(());
            }
        }
        if !busy {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let [_, path] = args.as_slice() else {
        eprintln!("usage: app_surface_peer <unix-socket-path>");
        return ExitCode::from(2);
    };
    let result = (|| {
        let meta = std::fs::symlink_metadata(path).map_err(|e| format!("{path}: {e}"))?;
        if !meta.file_type().is_socket() {
            return Err(format!("{path} is not a Unix socket"));
        }
        let stream = UnixStream::connect(path).map_err(|e| format!("connect {path}: {e}"))?;
        let (tx, rx) = sync_channel(8);
        std::thread::spawn(move || read_commands(io::stdin().lock(), tx));
        run(stream, rx, io::stdout(), PENDING_WINDOW)
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(m) => {
            eprintln!("app_surface_peer: {m}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use coglet_surface_rust::transport::read_frame;
    use std::os::unix::net::UnixListener;

    const HELLO_CLOCK: u64 = 1_000_000;
    const EPOCH: u64 = 7;
    const ID1: &str = "11111111111111111111111111111111";
    const ID2: &str = "22222222222222222222222222222222";
    const OWNER: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const WINDOW: Duration = Duration::from_secs(6);
    // Literal expectations (not derived from the catalog code under test).
    const EVENTS: [(&str, &str); 4] = [
        ("control_changed", "app-control-changed"),
        ("focus_changed", "app-focus-changed"),
        ("view_changed", "app-view-changed"),
        ("window_changed", "app-window-changed"),
    ];
    const READS: [(&str, &str); 2] = [("controls", "app-controls-result"), ("state", "app-state-result")];
    const ACTIONS: [(&str, &str); 6] = [
        ("close_window", "app-window-args"),
        ("invoke", "app-invoke-args"),
        ("navigate", "app-location"),
        ("open_window", "app-window-args"),
        ("set_focus", "app-focus-args"),
        ("set_setting", "app-setting-args"),
    ];
    fn full(n: &str) -> String {
        format!("softmax.app.{n}")
    }
    fn names(l: &[(&str, &str)]) -> Vec<String> {
        l.iter().map(|(n, _)| full(n)).collect()
    }
    fn surface_id() -> String {
        format!("surface:{}", "a".repeat(64))
    }
    fn s02(m: Json) -> Vec<u8> {
        encode_msg(S02, &m).unwrap()
    }
    fn hello_frame(clock: u64, versions: Json) -> Vec<u8> {
        let b = json!({"surface": surface_id(), "offer": {"versions": versions}, "clock": clock, "correlation_id": "0123456789abcdef0123456789abcdef"});
        encode_msg(S01, &json!({"type": "hello", "body": b})).unwrap()
    }
    fn caps_json() -> Json {
        let schemas: serde_json::Map<String, Json> = EVENTS.iter().chain(&READS).chain(&ACTIONS)
            .map(|(n, leaf)| (full(n), json!(format!("https://softmax.com/schemas/app/v1/{leaf}")))).collect();
        json!({"name": "softmax.app", "events": names(&EVENTS), "reads": names(&READS), "actions": names(&ACTIONS),
               "exclusive": ["softmax.app_control"], "schemas": schemas})
    }
    fn caps_frame(entry: Json) -> Vec<u8> {
        caps_frame_at(HELLO_CLOCK, entry)
    }
    fn caps_frame_at(clock: u64, entry: Json) -> Vec<u8> {
        s02(json!({"type": "capabilities", "body": {"revision": 1, "clock": clock, "entries": [entry], "correlation_id": "c".repeat(32)}}))
    }
    fn grant_json() -> Json {
        let ops: Vec<String> = [&EVENTS[..], &READS[..], &ACTIONS[..]].concat().iter().map(|(n, _)| full(n)).collect();
        json!({"coglet": OWNER, "operations": ops, "resources": ["softmax.app_control"], "expires": HELLO_CLOCK + 600_000_000})
    }
    fn grants_frame(entries: Json) -> Vec<u8> {
        s02(json!({"type": "grants", "body": {"epoch": EPOCH, "entries": entries, "correlation_id": "d".repeat(32)}}))
    }
    fn peer_at(hello_clock: u64) -> Peer {
        let mut p = Peer::new(Catalog::load().unwrap(), WINDOW);
        let out = p.on_frame(&hello_frame(hello_clock, json!(["0.2", "0.1"]))).unwrap().out;
        assert_eq!(out.len(), 1);
        p.on_frame(&caps_frame_at(hello_clock, caps_json())).unwrap();
        p
    }
    fn ready_peer() -> Peer {
        let mut p = peer_at(HELLO_CLOCK);
        p.on_frame(&grants_frame(json!([grant_json()]))).unwrap();
        assert!(p.ready());
        p
    }
    fn act(op: &str, args: Json, id: &str) -> Cmd {
        Cmd::Act { op: op.into(), args, id: id.into() }
    }
    fn nav() -> Json {
        json!({"window": "settings", "route": "general"})
    }
    fn decode_s02(raw: &[u8]) -> Json {
        to_json(&coglet_types::decode(S02, raw).unwrap()).unwrap()
    }
    fn outcome_event(seq: u64, id: &str, status: &str) -> Vec<u8> {
        let p = if status == "succeeded" { json!({"id": id, "correlation_id": id, "status": status, "result": {}}) }
        else { json!({"id": id, "correlation_id": id, "status": status, "error": {"code": "permission_denied", "message": "no", "retryable": false}}) };
        s02(json!({"type": "event", "body": {"source": surface_id(), "kind": "action.outcome", "seq": seq, "logical_time": 1, "payload": p, "correlation_id": id}}))
    }
    fn accept_frame(id: &str, accepted: bool) -> Vec<u8> {
        let mut b = json!({"id": id, "correlation_id": id, "accepted": accepted});
        if !accepted { b["error"] = json!({"code": "permission_denied", "message": "no", "retryable": false}); }
        s02(json!({"type": "accept", "body": b}))
    }
    fn view_event(seq: u64, payload: Json) -> Vec<u8> {
        view_event_at(seq, 1, &surface_id(), payload)
    }
    fn view_event_at(seq: u64, logical: u64, source: &str, payload: Json) -> Vec<u8> {
        s02(json!({"type": "event", "body": {"source": source, "kind": "softmax.app.view_changed", "seq": seq, "logical_time": logical, "payload": payload, "correlation_id": "e".repeat(32)}}))
    }

    #[test]
    fn sha256_vector_and_catalog_pin() {
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
        assert_eq!(sha256_hex(&[b'a'; 1000]), "41edece42d63e8d9bf515a9ba6932e1c20cbc9f5a5d134645adb5db1b9737ea3");
        assert_eq!(sha256_hex(CATALOG), PIN);
        assert!(Catalog::load().is_ok());
    }

    #[test]
    fn handshake_replies_0_2_with_hello_correlation_and_withholds_commands_until_grants() {
        let mut p = Peer::new(Catalog::load().unwrap(), WINDOW);
        assert!(p.command(act(&full("navigate"), nav(), ID1)).is_err());
        let inc = p.on_frame(&hello_frame(HELLO_CLOCK, json!(["0.2", "0.1"]))).unwrap();
        let reply = to_json(&coglet_types::decode(S01, &inc.out[0]).unwrap()).unwrap();
        assert_eq!(reply, json!({"type": "hello_reply", "body": {"selection": {"version": "0.2"}, "correlation_id": "0123456789abcdef0123456789abcdef"}}));
        assert!(p.command(act(&full("navigate"), nav(), ID1)).is_err(), "no commands before caps/grants");
        p.on_frame(&caps_frame(caps_json())).unwrap();
        assert!(p.command(act(&full("navigate"), nav(), ID1)).is_err());
        p.on_frame(&grants_frame(json!([grant_json()]))).unwrap();
        assert!(p.command(act(&full("navigate"), nav(), ID1)).is_ok());
        assert!(Peer::new(Catalog::load().unwrap(), WINDOW).on_frame(&hello_frame(1, json!(["0.1"]))).is_err(), "0.2 must be offered");
        assert!(Peer::new(Catalog::load().unwrap(), WINDOW).on_frame(&caps_frame(caps_json())).is_err(), "hello first");
    }

    #[test]
    fn caps_and_grants_mismatches_are_refused() {
        let mutations: Vec<Box<dyn Fn(&mut Json)>> = vec![
            Box::new(|c| { c["actions"].as_array_mut().unwrap().pop(); }),
            Box::new(|c| c["actions"].as_array_mut().unwrap().push(json!(SENSITIVE))),
            Box::new(|c| c["exclusive"] = json!(["softmax.app_control", "audio.mic"])),
            Box::new(|c| c["exclusive"] = json!([])),
            Box::new(|c| c["schemas"]["softmax.app.navigate"] = json!("https://softmax.com/schemas/app/v1/app-empty")),
            Box::new(|c| c["name"] = json!("softmax.app_sensitive")),
        ];
        for m in mutations {
            let mut c = caps_json();
            m(&mut c);
            let mut p = Peer::new(Catalog::load().unwrap(), WINDOW);
            p.on_frame(&hello_frame(HELLO_CLOCK, json!(["0.2"]))).unwrap();
            assert!(p.on_frame(&caps_frame(c)).is_err());
        }
        let mut p = Peer::new(Catalog::load().unwrap(), WINDOW);
        p.on_frame(&hello_frame(HELLO_CLOCK, json!(["0.2"]))).unwrap();
        assert!(p.on_frame(&caps_frame(json!({"name": "x.y", "events": [], "reads": [], "actions": [], "exclusive": [], "schemas": {}}))).is_err());
        let g = grant_json();
        let with = |k: &str, v: Json| {
            let mut x = grant_json();
            x[k] = v;
            json!([x])
        };
        let mut ops = names(&[&EVENTS[..], &READS[..], &ACTIONS[..]].concat());
        let mut dup = ops.clone();
        dup.push(full("navigate"));
        ops.push(SENSITIVE.into());
        let bad: Vec<Json> = vec![
            json!([]),
            json!([g.clone(), g.clone()]),
            with("coglet", json!(format!("coglet:{}", "c".repeat(64)))),
            with("resources", json!(["softmax.app_control", "audio.speaker"])),
            with("resources", json!([])),
            with("expires", json!(HELLO_CLOCK)),
            with("operations", json!(ops)),
            with("operations", json!([full("navigate")])),
            with("operations", json!(dup)),
        ];
        for entries in bad {
            let mut p = peer_at(HELLO_CLOCK);
            assert!(p.on_frame(&grants_frame(entries.clone())).is_err(), "{entries}");
        }
    }

    #[test]
    fn act_and_read_encoding_use_surface_clock_epoch_and_same_correlation() {
        let mut p = ready_peer();
        let raw = p.command(act(&full("navigate"), nav(), ID1)).unwrap();
        let expected = json!({"type": "act", "body": {"grant_epoch": EPOCH, "action": {"id": ID1, "coglet": OWNER,
            "capability": "softmax.app.navigate", "args": nav(), "deadline": HELLO_CLOCK + 5_000_000, "correlation_id": ID1}}});
        assert_eq!(decode_s02(&raw), expected);
        // Latest surface clock (health) moves the next deadline.
        p.on_frame(&s02(json!({"type": "health", "body": {"state": "ready", "clock": HELLO_CLOCK + 900, "pending": 0, "correlation_id": "f".repeat(32)}}))).unwrap();
        let r = p.command(Cmd::Read { op: "softmax.app.controls".into(), args: json!({"window": "main", "after": null, "limit": 5, "revision": null}), id: ID2.into() }).unwrap();
        assert_eq!(decode_s02(&r), json!({"type": "read", "body": {"coglet": OWNER, "capability": "softmax.app.controls",
            "args": {"window": "main", "after": null, "limit": 5, "revision": null}, "grant_epoch": EPOCH,
            "deadline": HELLO_CLOCK + 900 + 5_000_000, "correlation_id": ID2}}));
        assert!(p.on_frame(&s02(json!({"type": "health", "body": {"state": "ready", "clock": HELLO_CLOCK, "pending": 0, "correlation_id": "f".repeat(32)}}))).is_err(), "clock must not decrease");
    }

    #[test]
    fn retry_resends_exact_bytes_and_conflicting_ids_are_refused() {
        let mut p = ready_peer();
        let first = p.command(act(&full("set_setting"), json!({"key": "floating_orb", "value": true}), ID1)).unwrap();
        p.on_frame(&s02(json!({"type": "health", "body": {"state": "ready", "clock": HELLO_CLOCK + 2_000_000, "pending": 0, "correlation_id": "f".repeat(32)}}))).unwrap();
        assert_eq!(p.command(Cmd::Retry { id: ID1.into() }).unwrap(), first, "exact stored bytes, not rebuilt with a new deadline");
        assert_eq!(p.command(Cmd::Retry { id: ID1.into() }).unwrap(), first);
        assert!(p.command(act(&full("set_setting"), json!({"key": "floating_orb", "value": false}), ID1)).is_err(), "same id, other bytes");
        assert!(p.command(act(&full("set_setting"), json!({"key": "floating_orb", "value": true}), ID1)).is_err(), "same id never replaces");
        assert_eq!(p.command(Cmd::Retry { id: ID1.into() }).unwrap(), first);
        assert!(p.command(Cmd::Retry { id: ID2.into() }).is_err());
        p.command(Cmd::Read { op: full("state"), args: json!({}), id: ID2.into() }).unwrap();
        assert!(p.command(Cmd::Retry { id: ID2.into() }).is_err(), "reads are not retried");
    }

    #[test]
    fn wrong_schema_sibling_float_ungranted_and_direction_are_refused() {
        let mut p = ready_peer();
        let bad = [
            ("softmax.app.set_setting", json!({"key": "floating_orb", "value": true, "extra": 1})),
            ("softmax.app.set_setting", json!({"key": "floating_orb", "value": "Dark"})),
            ("softmax.app.set_setting", json!({"key": "appearance", "value": true})),
            ("softmax.app.navigate", json!({"window": "settings", "route": "main"})),
            ("softmax.app.navigate", json!({"window": "main"})),
            ("softmax.app.set_focus", json!({"window": "main", "control": 5})),
            ("softmax.app.state", json!({})),
            ("softmax.app.view_changed", json!({})),
            ("softmax.app.nonexistent", json!({})),
        ];
        for (i, (op, args)) in bad.iter().enumerate() {
            assert!(p.command(act(op, args.clone(), &format!("{i:032x}"))).is_err(), "{op} {args}");
        }
        assert!(p.command(Cmd::Read { op: full("navigate"), args: nav(), id: ID2.into() }).is_err());
        let float = br#"{"kind":"act","operation":"set_setting","args":{"key":"floating_orb","value":1.5},"id":"11111111111111111111111111111111"}"#;
        let Cmd::Act { op, args, id } = parse_cmd(float).unwrap() else { panic!() };
        assert!(p.command(Cmd::Act { op, args, id }).is_err(), "float refused by the integer profile");
        assert!(p.retained.is_empty(), "refused commands retain nothing");
        // Expired grant (surface clock reaches `expires`) refuses even granted operations.
        let mut q = ready_peer();
        let hc = HELLO_CLOCK + 600_000_000;
        q.on_frame(&s02(json!({"type": "health", "body": {"state": "ready", "clock": hc, "pending": 0, "correlation_id": "f".repeat(32)}}))).unwrap();
        assert!(q.command(act(&full("close_window"), json!({"window": "about"}), ID1)).is_err());
    }

    #[test]
    fn sensitive_negative_control_is_sent_ungranted_and_never_counted_authorized() {
        let mut p = ready_peer();
        let args = json!({"verb": "press", "window": "settings", "control": "x"});
        let raw = p.command(act(&full("invoke_sensitive"), args.clone(), ID1)).unwrap();
        assert_eq!(decode_s02(&raw)["body"]["action"]["capability"], SENSITIVE);
        assert!(!p.granted.contains(SENSITIVE));
        p.on_frame(&accept_frame(ID1, false)).unwrap();
        assert!(p.on_frame(&outcome_event(1, ID1, "failed")).is_err(), "a rejected act gets no outcome");
        let mut q = ready_peer();
        q.command(act(&full("invoke_sensitive"), args.clone(), ID1)).unwrap();
        assert!(q.on_frame(&accept_frame(ID1, true)).is_err());
        let mut r = ready_peer();
        r.command(act(&full("invoke_sensitive"), args, ID1)).unwrap();
        assert!(r.on_frame(&outcome_event(1, ID1, "succeeded")).is_err());
    }

    #[test]
    fn incoming_events_outcomes_and_replies_are_validated_against_retained_commands() {
        let mut p = ready_peer();
        p.command(act(&full("navigate"), nav(), ID1)).unwrap();
        p.on_frame(&view_event(1, json!({"revision": 2, "window": "settings", "route": "general"}))).unwrap();
        assert!(p.on_frame(&view_event(1, json!({"revision": 2, "window": "settings", "route": "general"}))).is_err(), "seq must increase");
        assert!(p.on_frame(&view_event(2, json!({"revision": 2, "window": "settings", "route": "main"}))).is_err());
        assert!(p.on_frame(&view_event(3, json!({"revision": 2, "window": "settings", "route": "general", "x": 1}))).is_err());
        // Outcome and accept: unknown id, wrong correlation, and over-repeat are refused.
        assert!(p.on_frame(&accept_frame(ID2, true)).is_err());
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        assert!(p.on_frame(&accept_frame(ID1, true)).is_err(), "no retry sent: a second accept is unexpected");
        p.on_frame(&outcome_event(4, ID1, "succeeded")).unwrap();
        assert!(p.on_frame(&outcome_event(5, ID1, "succeeded")).is_err());
        p.command(Cmd::Retry { id: ID1.into() }).unwrap();
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        p.on_frame(&outcome_event(6, ID1, "succeeded")).unwrap();
        assert!(p.on_frame(&outcome_event(7, ID1, "failed")).is_err(), "outcome status may not change");
        let wrong = s02(json!({"type": "event", "body": {"source": surface_id(), "kind": "action.outcome", "seq": 8, "logical_time": 1,
            "payload": {"id": ID1, "correlation_id": ID1, "status": "succeeded", "result": {"x": 1}}, "correlation_id": ID1}}));
        let mut q = ready_peer();
        q.command(act(&full("navigate"), nav(), ID1)).unwrap();
        q.on_frame(&accept_frame(ID1, true)).unwrap();
        assert!(q.on_frame(&wrong).is_err(), "result validated against app-empty");
        // read_reply: retained operation selects the output schema.
        let rd = |ok: Json| s02(json!({"type": "read_reply", "body": ok}));
        let mut r = ready_peer();
        r.command(Cmd::Read { op: full("controls"), args: json!({"window": "main", "after": null, "limit": 1, "revision": null}), id: ID2.into() }).unwrap();
        assert!(r.on_frame(&rd(json!({"correlation_id": ID1, "ok": true, "value": {}, "snapshot": 1}))).is_err());
        assert!(r.on_frame(&rd(json!({"correlation_id": ID2, "ok": true, "value": {"windows": []}, "snapshot": 1}))).is_err());
        r.on_frame(&rd(json!({"correlation_id": ID2, "ok": true, "value": {"revision": 1, "window": "main", "controls": [], "next": null}, "snapshot": 1}))).unwrap();
        assert!(r.on_frame(&rd(json!({"correlation_id": ID2, "ok": true, "value": {"revision": 1, "window": "main", "controls": [], "next": null}, "snapshot": 1}))).is_err());
        // Surface `error` is surfaced as fatal after being printed.
        let err = s02(json!({"type": "error", "body": {"error": {"code": "conflict", "message": "m", "retryable": false}, "correlation_id": "9".repeat(32)}}));
        assert!(ready_peer().on_frame(&err).unwrap().fatal.is_some());
    }

    #[test]
    fn retained_limit_duplicate_ids_and_clock_overflow_are_checked() {
        let mut p = ready_peer();
        for i in 0..MAX_RETAINED {
            p.command(act(&full("navigate"), nav(), &format!("{i:032x}"))).unwrap();
        }
        assert!(p.command(act(&full("navigate"), nav(), &format!("{:032x}", 99))).is_err(), "65th command refused");
        assert!(p.command(act(&full("navigate"), nav(), &format!("{:032x}", 3))).is_err(), "duplicate id");
        let mut o = peer_at(u64::MAX - 1);
        let mut g = grant_json();
        g["expires"] = json!(u64::MAX);
        o.on_frame(&grants_frame(json!([g]))).unwrap();
        assert!(o.command(act(&full("navigate"), nav(), ID1)).is_err(), "deadline addition overflows");
        assert!(o.health(5).is_err(), "health clock overflows");
        assert!(o.health(1).is_ok());
        let mut h = ready_peer();
        let a = decode_s02(&h.health(10).unwrap())["body"]["clock"].as_u64().unwrap();
        let b = decode_s02(&h.health(5).unwrap())["body"]["clock"].as_u64().unwrap();
        assert_eq!((a, b), (HELLO_CLOCK + 10, HELLO_CLOCK + 10), "health never decreases");
        assert!(Peer::new(Catalog::load().unwrap(), WINDOW).health(1).is_err());
    }

    #[test]
    fn command_parsing_is_strict_and_bounded() {
        assert_eq!(parse_cmd(br#"{"kind":"close"}"#).unwrap(), Cmd::Close);
        assert_eq!(parse_cmd(format!(r#"{{"kind":"retry","id":"{ID1}"}}"#).as_bytes()).unwrap(), Cmd::Retry { id: ID1.into() });
        for bad in [
            r#"{"kind":"close","x":1}"#.to_string(),
            r#"{"kind":"retry"}"#.to_string(),
            format!(r#"{{"kind":"act","operation":"navigate","args":{{}},"id":"{ID1}","deadline":1}}"#),
            r#"{"kind":"act","operation":"navigate","args":{},"id":"short"}"#.to_string(),
            format!(r#"{{"kind":"act","operation":"navigate","args":[],"id":"{ID1}"}}"#),
            r#"{"kind":"wat"}"#.into(),
            "[1]".into(),
            "{".into(),
            format!(r#"{{"kind":"retry","id":"{}"}}"#, ID1.replace('1', "A")),
            format!(r#"{{"kind":"act","operation":"navigate","args":{{"a":"{}"}},"id":"{ID1}"}}"#, "x".repeat(MAX_LINE)),
        ] {
            assert!(parse_cmd(bad.as_bytes()).is_err(), "{}", &bad[..bad.len().min(80)]);
        }
        let run_reader = |input: Vec<u8>| {
            let (tx, rx) = sync_channel(8);
            read_commands(io::Cursor::new(input), tx);
            rx.into_iter().collect::<Vec<_>>()
        };
        assert_eq!(run_reader(b"{\"kind\":\"close\"}\n".to_vec()), vec![Ok(Cmd::Close)]);
        assert!(run_reader(b"".to_vec())[0].is_err(), "EOF without close");
        assert!(run_reader(b"{\"kind\":\"close\"}".to_vec())[0].is_err(), "EOF inside a command");
        assert!(run_reader(vec![b'x'; MAX_LINE + 10])[0].as_ref().is_err_and(|e| e.contains("64 KiB")));
    }

    struct Sink(SyncSender<Vec<u8>>);
    impl Write for Sink {
        fn write(&mut self, b: &[u8]) -> io::Result<usize> {
            self.0.send(b.to_vec()).map_err(|_| io::ErrorKind::BrokenPipe)?;
            Ok(b.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    /// stdout whose consumer never reads: every write blocks forever.
    struct Blocked;
    impl Write for Blocked {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    type Joined = std::thread::JoinHandle<Result<(), String>>;
    /// True external socket fixture: the surface side is driven only through the public
    /// SDK frame functions over a real Unix socket; `run` owns the peer side.
    fn socket_pair_with(out: impl Write + Send + 'static, pending: Duration) -> (Joined, UnixStream, SyncSender<Result<Cmd, String>>, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let listener = UnixListener::bind(dir.path().join("s")).unwrap();
        let client = UnixStream::connect(dir.path().join("s")).unwrap();
        let (server, _) = listener.accept().unwrap();
        server.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        server.set_write_timeout(Some(Duration::from_secs(5))).unwrap();
        let (ctx, crx) = sync_channel(8);
        (std::thread::spawn(move || run(client, crx, out, pending)), server, ctx, dir)
    }
    fn socket_pair() -> (Joined, UnixStream, Receiver<Vec<u8>>, SyncSender<Result<Cmd, String>>, tempfile::TempDir) {
        let (otx, orx) = sync_channel::<Vec<u8>>(64);
        let (h, srv, ctx, dir) = socket_pair_with(Sink(otx), WINDOW);
        (h, srv, orx, ctx, dir)
    }
    fn handshake(srv: &mut UnixStream) {
        write_frame(srv, &hello_frame(HELLO_CLOCK, json!(["0.2"]))).unwrap();
        read_frame(srv).unwrap().unwrap();
        write_frame(srv, &caps_frame(caps_json())).unwrap();
        write_frame(srv, &grants_frame(json!([grant_json()]))).unwrap();
    }
    fn finished_within(h: &Joined, secs: u64) -> bool {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end && !h.is_finished() {
            std::thread::sleep(Duration::from_millis(20));
        }
        h.is_finished()
    }

    #[test]
    fn real_socket_handshake_command_retry_health_and_clean_close() {
        let (h, mut srv, orx, ctx, _d) = socket_pair();
        write_frame(&mut srv, &hello_frame(HELLO_CLOCK, json!(["0.2", "0.1"]))).unwrap();
        let reply = read_frame(&mut srv).unwrap().unwrap();
        assert_eq!(to_json(&coglet_types::decode(S01, &reply).unwrap()).unwrap()["body"]["selection"], json!({"version": "0.2"}));
        write_frame(&mut srv, &caps_frame(caps_json())).unwrap();
        write_frame(&mut srv, &grants_frame(json!([grant_json()]))).unwrap();
        ctx.send(Ok(act(&full("navigate"), nav(), ID1))).unwrap();
        let (mut acts, mut healths) = (Vec::new(), Vec::new());
        let next = |srv: &mut UnixStream| read_frame(srv).unwrap().unwrap();
        while acts.is_empty() {
            let raw = next(&mut srv);
            let m = decode_s02(&raw);
            if m["type"] == "act" { acts.push(raw) } else { assert_eq!(m["type"], "health"); healths.push(m) }
        }
        write_frame(&mut srv, &accept_frame(ID1, true)).unwrap();
        write_frame(&mut srv, &view_event(1, json!({"revision": 2, "window": "settings", "route": "general"}))).unwrap();
        write_frame(&mut srv, &outcome_event(2, ID1, "succeeded")).unwrap();
        ctx.send(Ok(Cmd::Retry { id: ID1.into() })).unwrap();
        while acts.len() < 2 || healths.len() < 2 {
            let raw = next(&mut srv);
            let m = decode_s02(&raw);
            if m["type"] == "act" { acts.push(raw) } else { assert_eq!(m["type"], "health"); healths.push(m) }
        }
        assert_eq!(acts[0], acts[1], "retry body is byte-identical on the wire");
        // A clean close needs the retry answered too: identical accept, identical outcome.
        write_frame(&mut srv, &accept_frame(ID1, true)).unwrap();
        write_frame(&mut srv, &outcome_event(3, ID1, "succeeded")).unwrap();
        let clocks: Vec<u64> = healths.iter().map(|m| m["body"]["clock"].as_u64().unwrap()).collect();
        assert!(clocks.windows(2).all(|w| w[0] <= w[1]) && clocks[0] >= HELLO_CLOCK, "{clocks:?}");
        ctx.send(Ok(Cmd::Close)).unwrap();
        assert_eq!(h.join().unwrap(), Ok(()));
        let lines: Vec<Json> = orx.try_iter().filter(|l| l != b"\n").map(|l| serde_json::from_slice(&l).unwrap()).collect();
        let kinds: Vec<&str> = lines.iter().map(|l| l["type"].as_str().unwrap()).collect();
        assert_eq!(kinds, ["hello", "capabilities", "grants", "accept", "event", "event", "accept", "event"]);
    }

    #[test]
    fn real_socket_partial_frame_eof_and_wire_violation_exit_with_error() {
        let (h, mut srv, _o, _c, _d) = socket_pair();
        write_frame(&mut srv, &hello_frame(HELLO_CLOCK, json!(["0.2"]))).unwrap();
        read_frame(&mut srv).unwrap().unwrap();
        srv.write_all(&[0, 0, 1, 0, 1, 2, 3]).unwrap();
        drop(srv);
        assert!(h.join().unwrap().unwrap_err().contains("EOF inside control frame"));
        let (h, mut srv, _o, _c, _d) = socket_pair();
        write_frame(&mut srv, &hello_frame(HELLO_CLOCK, json!(["0.2"]))).unwrap();
        read_frame(&mut srv).unwrap().unwrap();
        write_frame(&mut srv, b"\xa0").unwrap(); // CBOR {} is not a surface message
        assert!(h.join().unwrap().is_err());
        let (h, srv, _o, _c, _d) = socket_pair();
        drop(srv);
        assert!(h.join().unwrap().unwrap_err().contains("closed"));
    }

    #[test]
    fn outcome_needs_prior_true_accept_and_repeats_are_identical() {
        let navigate = || full("navigate");
        // outcome before any accept
        let mut p = ready_peer();
        p.command(act(&navigate(), nav(), ID1)).unwrap();
        assert!(p.on_frame(&outcome_event(1, ID1, "succeeded")).is_err(), "no accept yet");
        // reject then success / any outcome
        let mut p = ready_peer();
        p.command(act(&navigate(), nav(), ID1)).unwrap();
        p.on_frame(&accept_frame(ID1, false)).unwrap();
        assert!(p.on_frame(&outcome_event(1, ID1, "succeeded")).is_err(), "reject then succeeded");
        // accept flip after a retry
        let mut p = ready_peer();
        p.command(act(&navigate(), nav(), ID1)).unwrap();
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        p.command(Cmd::Retry { id: ID1.into() }).unwrap();
        assert!(p.on_frame(&accept_frame(ID1, false)).is_err(), "accept flip");
        // differing repeat outcome payload (same status, different error)
        let failed = |seq: u64, msg: &str| {
            s02(json!({"type": "event", "body": {"source": surface_id(), "kind": "action.outcome", "seq": seq, "logical_time": 1, "correlation_id": ID1,
                "payload": {"id": ID1, "correlation_id": ID1, "status": "failed", "error": {"code": "permission_denied", "message": msg, "retryable": false}}}}))
        };
        let mut p = ready_peer();
        p.command(act(&navigate(), nav(), ID1)).unwrap();
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        p.on_frame(&failed(1, "no")).unwrap();
        p.command(Cmd::Retry { id: ID1.into() }).unwrap();
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        assert!(p.on_frame(&failed(2, "other")).is_err(), "retry outcome must equal the first");
        // identical repeat after retry is accepted and the command is then answered
        let mut p = ready_peer();
        p.command(act(&navigate(), nav(), ID1)).unwrap();
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        p.on_frame(&failed(1, "no")).unwrap();
        p.command(Cmd::Retry { id: ID1.into() }).unwrap();
        assert_eq!(p.unanswered(), [ID1]);
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        p.on_frame(&failed(2, "no")).unwrap();
        assert!(p.unanswered().is_empty());
    }

    #[test]
    fn pending_commands_fail_after_the_local_deadline() {
        let mut p = ready_peer();
        p.command(act(&full("navigate"), nav(), ID1)).unwrap();
        p.command(Cmd::Read { op: full("controls"), args: json!({"window": "main", "after": null, "limit": 1, "revision": null}), id: ID2.into() }).unwrap();
        let now = Instant::now();
        assert!(p.check_pending(now).is_ok());
        assert!(p.check_pending(now + Duration::from_secs(5)).is_ok());
        assert!(p.check_pending(now + Duration::from_secs(7)).unwrap_err().contains(ID1));
        // accepted but no outcome: still pending; a fresh window starts at the accept
        p.on_frame(&accept_frame(ID1, true)).unwrap();
        assert!(p.check_pending(Instant::now() + Duration::from_secs(7)).is_err());
        p.on_frame(&outcome_event(1, ID1, "succeeded")).unwrap();
        assert_eq!(p.unanswered(), [ID2], "the read still waits for read_reply");
        assert!(p.check_pending(Instant::now() + Duration::from_secs(7)).unwrap_err().contains(ID2));
        p.on_frame(&s02(json!({"type": "read_reply", "body": {"correlation_id": ID2, "ok": true, "value": {"revision": 1, "window": "main", "controls": [], "next": null}, "snapshot": 1}}))).unwrap();
        assert!(p.unanswered().is_empty());
        // a retry re-opens the wait for one more accept + outcome
        let mut q = ready_peer();
        q.command(act(&full("navigate"), nav(), ID1)).unwrap();
        q.on_frame(&accept_frame(ID1, false)).unwrap();
        assert!(q.unanswered().is_empty(), "a rejected act is answered");
        q.command(Cmd::Retry { id: ID1.into() }).unwrap();
        assert_eq!(q.unanswered(), [ID1]);
    }

    #[test]
    fn operation_names_are_full_catalog_names_only() {
        let mut p = ready_peer();
        for op in ["navigate", "state", "softmax.app.nope", "softmax.app_control", ""] {
            assert!(p.command(act(op, nav(), ID1)).is_err(), "{op}");
        }
        assert!(p.command(act(SENSITIVE, json!({"verb": "press", "window": "settings", "control": "x"}), ID1)).is_ok());
        assert!(p.command(act(&full("navigate"), nav(), ID2)).is_ok());
    }

    #[test]
    fn duplicate_json_keys_are_refused_at_every_depth() {
        let id = |k: &str| format!(r#""{k}":"{ID1}""#);
        let ok = format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"window":"settings"}},{}}}"#, id("id"));
        assert!(parse_cmd(ok.as_bytes()).is_ok());
        for bad in [
            format!(r#"{{"kind":"close","kind":"retry",{}}}"#, id("id")),
            format!(r#"{{"kind":"retry",{},{}}}"#, id("id"), id("id")),
            r#"{"kind":"close","kind":"close"}"#.to_string(),
            format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"window":"settings","window":"main"}},{}}}"#, id("id")),
            format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"a":[{{"b":1,"b":2}}]}},{}}}"#, id("id")),
            format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"a":{{"b":{{"c":1,"c":1}}}}}},{}}}"#, id("id")),
        ] {
            let e = parse_cmd(bad.as_bytes()).unwrap_err();
            assert!(e.contains("duplicate key"), "{bad}: {e}");
        }
        // Arbitrary-precision numbers (private serde map) neither trip nor mask the check.
        let big = format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"n":123456789012345678901234567890,"m":1.5e3}},{}}}"#, id("id"));
        assert!(parse_cmd(big.as_bytes()).is_ok());
        let big_dup = format!(r#"{{"kind":"act","operation":"softmax.app.navigate","args":{{"n":123456789012345678901234567890,"n":1}},{}}}"#, id("id"));
        assert!(parse_cmd(big_dup.as_bytes()).unwrap_err().contains("duplicate key"));
        assert!(parse_cmd(format!(r#"{{"kind":"close"}} {}"#, "{}").as_bytes()).is_err(), "trailing data");
    }

    #[test]
    fn logical_time_and_revision_never_go_backwards_and_bad_sources_do_not_advance_state() {
        let pl = |rev: u64| json!({"revision": rev, "window": "settings", "route": "general"});
        let mut p = ready_peer();
        p.on_frame(&view_event_at(1, 5, &surface_id(), pl(3))).unwrap();
        p.on_frame(&view_event_at(2, 5, &surface_id(), pl(3))).unwrap();
        assert!(p.on_frame(&view_event_at(3, 4, &surface_id(), pl(3))).is_err(), "logical_time 5 -> 4");
        let mut p = ready_peer();
        p.on_frame(&view_event_at(1, 5, &surface_id(), pl(3))).unwrap();
        assert!(p.on_frame(&view_event_at(2, 6, &surface_id(), pl(2))).is_err(), "revision 3 -> 2");
        let mut p = ready_peer();
        p.on_frame(&view_event_at(1, 5, &surface_id(), pl(3))).unwrap();
        let other = format!("surface:{}", "c".repeat(64));
        assert!(p.on_frame(&view_event_at(9, 99, &other, pl(50))).is_err());
        assert_eq!((p.last_seq, p.last_logical, p.last_revision), (Some(1), 5, 3), "wrong source advances nothing");
    }

    #[test]
    fn blocked_stdout_fails_closed_without_stalling_the_socket_loop() {
        let (h, mut srv, _ctx, _d) = socket_pair_with(Blocked, WINDOW);
        handshake(&mut srv);
        for seq in 1..=(STDOUT_QUEUE as u64 + 100) {
            let f = view_event_at(seq, seq, &surface_id(), json!({"revision": seq, "window": "settings", "route": "general"}));
            if write_frame(&mut srv, &f).is_err() {
                break;
            }
        }
        assert!(finished_within(&h, 3), "run must not hang on a blocked stdout");
        assert!(h.join().unwrap().unwrap_err().contains("stdout consumer stalled"));
    }

    #[test]
    fn real_socket_close_with_unanswered_command_fails_naming_the_id() {
        let (otx, _orx) = sync_channel::<Vec<u8>>(64);
        let (h, mut srv, ctx, _d) = socket_pair_with(Sink(otx), Duration::from_millis(300));
        handshake(&mut srv);
        ctx.send(Ok(act(&full("navigate"), nav(), ID1))).unwrap();
        ctx.send(Ok(Cmd::Close)).unwrap();
        assert!(finished_within(&h, 3));
        let e = h.join().unwrap().unwrap_err();
        assert!(e.contains(ID1) && e.contains("no answer"), "{e}");
    }
}
