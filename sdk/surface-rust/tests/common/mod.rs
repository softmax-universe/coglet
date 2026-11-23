#![allow(dead_code)]
//! Shared rig: drives a real `Surface` through real deterministic-CBOR bytes only.

use coglet_surface_rust::testing::ScriptedBackend;
use coglet_surface_rust::{Config, ControlPeer, GrantSpec, SpeechConfig, Surface};
use serde_json::{Value as Json, json};
use std::path::PathBuf;
use std::sync::Arc;

pub const SURFACE: &str =
    "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
pub const B: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
pub const C: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
pub const RULE: &str = "surface::surface-message";
pub const ALL_SPEECH: &[&str] = &[
    "speech.start",
    "speech.partial",
    "speech.end",
    "speech.position",
    "speech.barge_in",
    "speech.config",
    "speech.speak_chunk",
    "speech.stop",
    "speech.backchannel",
    "speech.session_open",
    "speech.session_close",
    "speech.tool_call",
    "speech.tool_result",
    "expression.set_expression",
    "expression.state",
];

pub fn hid(n: u64) -> String {
    format!("{n:032x}")
}

pub fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!(
        "coglet-surface-it-{}-{}-{}",
        std::process::id(),
        tag,
        coglet_surface_rust::ids::random_id().unwrap()
    ));
    let _ = std::fs::remove_dir_all(&d);
    d
}

pub fn speech_config(mode: &str) -> SpeechConfig {
    SpeechConfig {
        stt_placement: "surface".into(),
        stt_route: "local".into(),
        tts_placement: "hub".into(),
        tts_route: "voice".into(),
        barge: "stop".into(),
        session_mode: mode.into(),
    }
}

pub fn grant(coglet: &str, ops: &[&str], res: &[&str], expires: u64) -> GrantSpec {
    GrantSpec {
        coglet: coglet.into(),
        operations: ops.iter().map(|s| s.to_string()).collect(),
        resources: res.iter().map(|s| s.to_string()).collect(),
        expires,
    }
}

pub fn encode(msg: &Json) -> Vec<u8> {
    coglet_surface_rust::codec::encode(RULE, msg).expect("test message must be valid")
}
pub fn decode(raw: &[u8]) -> Json {
    coglet_types::decode_typed(RULE, raw).expect("surface output must be valid")
}

pub struct Rig {
    pub s: Surface,
    pub backend: Arc<ScriptedBackend>,
    pub dir: PathBuf,
    pub now: u64,
    pub epoch: u64,
    pub ops: Vec<String>,
    pub mode: String,
}

pub struct Opts<'a> {
    pub ops: &'a [&'a str],
    pub mode: &'a str,
    pub grant_ops: Option<&'a [&'a str]>,
    pub resources: &'a [&'a str],
    pub expires: u64,
    pub bound_c: bool,
    pub routes: &'a [&'a str],
    pub tools: &'a [&'a str],
    pub max_pending: usize,
}

impl Default for Opts<'_> {
    fn default() -> Self {
        Opts {
            ops: ALL_SPEECH,
            mode: "text",
            grant_ops: None,
            resources: &["audio.mic", "audio.speaker", "display.face"],
            expires: 100_000_000,
            bound_c: false,
            routes: &[],
            tools: &[],
            max_pending: 64,
        }
    }
}

pub fn config(o: &Opts) -> Config {
    let mut cfg = Config::new(SURFACE);
    cfg.bound_coglets.push(B.into());
    if o.bound_c {
        cfg.bound_coglets.push(C.into());
    }
    cfg.hello_correlation = Some(hid(1));
    cfg.limits.max_pending = o.max_pending;
    cfg.grants.push(grant(
        B,
        o.grant_ops.unwrap_or(o.ops),
        o.resources,
        o.expires,
    ));
    cfg
}

impl Rig {
    pub fn new() -> Rig {
        Rig::with(Opts::default())
    }
    pub fn with(o: Opts) -> Rig {
        let dir = tmpdir("rig");
        let backend = ScriptedBackend::new(speech_config(o.mode), o.ops);
        backend.set_routes(o.routes);
        backend.set_tools(o.tools);
        let s = Surface::open(config(&o), &dir, backend.clone()).expect("open");
        Rig {
            s,
            backend,
            dir,
            now: 0,
            epoch: 0,
            ops: o.ops.iter().map(|x| x.to_string()).collect(),
            mode: o.mode.into(),
        }
    }
    /// Reopen the same state directory with a new Surface (simulated restart).
    pub fn restart(mut self, o: Opts) -> Rig {
        let dir = self.dir.clone();
        let backend = ScriptedBackend::new(speech_config(o.mode), o.ops);
        backend.set_routes(o.routes);
        backend.set_tools(o.tools);
        let cfg = config(&o);
        drop(self.s);
        let s = Surface::open(cfg, &dir, backend.clone()).expect("reopen");
        self.s = s;
        self.backend = backend;
        self.epoch = 0;
        self
    }
    pub fn start(&mut self) -> Json {
        let out = self.s.start().expect("start");
        assert_eq!(out.len(), 1);
        decode(&out[0])
    }
    /// Full negotiation. Returns (capabilities, grants).
    pub fn connect(&mut self) -> (Json, Json) {
        let hello = self.start();
        let reply = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},
            "correlation_id": hello["body"]["correlation_id"]}});
        let out = self.send(&reply);
        assert_eq!(out.len(), 2, "capabilities + grants: {out:?}");
        assert_eq!(out[0]["type"], "capabilities");
        assert_eq!(out[1]["type"], "grants");
        self.epoch = out[1]["body"]["epoch"].as_u64().unwrap();
        (out[0].clone(), out[1].clone())
    }
    pub fn send(&mut self, msg: &Json) -> Vec<Json> {
        self.send_raw(&encode(msg))
            .expect("receive")
            .iter()
            .map(|b| decode(b))
            .collect()
    }
    pub fn send_raw(&mut self, raw: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.s.receive(raw, self.now)
    }
    pub fn advance(&mut self, us: u64) -> Vec<Json> {
        self.now += us;
        self.s
            .tick(self.now)
            .expect("tick")
            .iter()
            .map(|b| decode(b))
            .collect()
    }
    pub fn tick(&mut self) -> Vec<Json> {
        self.advance(0)
    }
    pub fn act_body(
        &self,
        coglet: &str,
        id: u64,
        op: &str,
        args: Json,
        deadline: u64,
        epoch: u64,
    ) -> Json {
        json!({"type":"act","body":{"grant_epoch":epoch,"action":{
            "id":hid(id),"coglet":coglet,"capability":op,"args":args,"deadline":deadline,"correlation_id":hid(id)}}})
    }
    /// Act as coglet B with the current epoch and a deadline 5 s ahead.
    pub fn act(&mut self, id: u64, op: &str, args: Json) -> Vec<Json> {
        let m = self.act_body(B, id, op, args, self.now + 5_000_000, self.epoch);
        self.send(&m)
    }
    pub fn read(&mut self, corr: u64, op: &str, args: Json) -> Json {
        let m = json!({"type":"read","body":{"coglet":B,"capability":op,"args":args,
            "grant_epoch":self.epoch,"deadline":self.now+5_000_000,"correlation_id":hid(corr)}});
        let out = self.send(&m);
        assert_eq!(out.len(), 1, "{out:?}");
        out[0].clone()
    }
    pub fn health(&mut self) -> Vec<Json> {
        let m = json!({"type":"health","body":{"state":"ready","clock":self.now,"pending":0,"correlation_id":hid(99)}});
        self.send(&m)
    }
}

pub fn accepted(m: &Json) -> Option<bool> {
    (m["type"] == "accept").then(|| m["body"]["accepted"].as_bool().unwrap())
}
pub fn code(m: &Json) -> &str {
    m["body"]["error"]["code"].as_str().unwrap_or("")
}
pub fn outcomes(out: &[Json]) -> Vec<&Json> {
    out.iter()
        .filter(|m| m["type"] == "event" && m["body"]["kind"] == "action.outcome")
        .collect()
}
pub fn status(m: &Json) -> &str {
    m["body"]["payload"]["status"].as_str().unwrap()
}
pub fn events<'a>(out: &'a [Json], kind: &str) -> Vec<&'a Json> {
    out.iter()
        .filter(|m| m["type"] == "event" && m["body"]["kind"] == kind)
        .collect()
}
