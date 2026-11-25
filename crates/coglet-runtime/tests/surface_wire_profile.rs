//! Runtime negotiated wire-minor selection and 0.2->0.1 downgrade fences.
//!
//! The pinned product catalog here is a SYNTHETIC CLOSED fixture (`softmax` names
//! used only because they exercise the three-component grammar). It is not the
//! app catalog, app delivery, or a Rust/Swift conformance adapter registration.
//! In-process sessions drive the same byte API as the C `send/recv`; the daemon
//! test below drives the real production Unix-socket transport.
#![cfg(any(target_os = "linux", target_os = "macos"))]
use coglet_runtime::{
    ContractProfile, DurableStore, RuntimeService, SurfaceBinding, SurfaceSession,
    TrustedExtensionCatalog, cbor, digest,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const FACE: &str = "4444444444444444444444444444444a";
const APP: &str = "6666666666666666666666666666666a";
const READ: &str = "8888888888888888888888888888888a";
const GAME: &[u8] = include_bytes!("../examples/trusted-extension-catalog.json");

fn app_catalog() -> Vec<u8> {
    let uri = |n: &str| format!("https://example.invalid/softmax/1#{n}");
    let int = json!({"op":"range","min":0,"max":1_000_000});
    let text = json!({"op":"primitive","name":"tstr"});
    let field = |n: &str, t: &Value| json!({"name":n,"optional":false,"type":t});
    serde_json::to_vec(&json!({"version":1,"namespace":"softmax",
        "entries":{"softmax.app":["softmax.app_control"],"softmax.app_spare":[]},
        "schemas":{uri("empty"):"empty",uri("view"):"view",uri("nav"):"nav",uri("state"):"state"},
        "rules":{
            "empty":{"op":"record","fields":[]},
            "view":{"op":"record","fields":[field("revision",&int),field("window",&text),field("route",&text)]},
            "nav":{"op":"record","fields":[field("window",&text),field("route",&text)]},
            "state":{"op":"record","fields":[field("revision",&int)]}},
        "operations":{
            "softmax.app.state":{"entry":"softmax.app","direction":"read","input":uri("empty"),"output":uri("state"),"resources":[],"single_owner":true},
            "softmax.app.view_changed":{"entry":"softmax.app","direction":"event","input":uri("view"),"resources":[],"single_owner":true},
            "softmax.app.navigate":{"entry":"softmax.app","direction":"action","input":uri("nav"),"output":uri("empty"),"resources":["softmax.app_control"],"single_owner":true}}}))
    .unwrap()
}
fn catalog(raw: &[u8], profile: ContractProfile) -> TrustedExtensionCatalog {
    TrustedExtensionCatalog::from_pinned_bytes_with_profile(raw, &digest(raw), profile).unwrap()
}
fn setup(profile: ContractProfile, subs: &[&str], actions: &[Value]) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut s = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    s["manifest"]["id"] = json!(C);
    s["manifest"]["subscriptions"] = json!(subs);
    if profile == ContractProfile::Surface02 {
        s["contract_profile"] = json!("surface-0.2");
    }
    s["initial_actions"] = json!(actions);
    s["initial_effect_ids"]["actions"] =
        json!(actions.iter().map(|a| a["id"].clone()).collect::<Vec<_>>());
    s
}
fn face_action() -> Value {
    json!({"coglet":C,"id":FACE,"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":50_000_000,"correlation_id":"5".repeat(32)})
}
fn app_action() -> Value {
    json!({"coglet":C,"id":APP,"capability":"softmax.app.navigate","args":{"window":"main","route":"home"},"deadline":50_000_000,"correlation_id":"7".repeat(32)})
}
fn app_setup() -> Value {
    let mut s = setup(
        ContractProfile::Surface02,
        &["softmax.app.view_changed", "speech.start"],
        &[face_action(), app_action()],
    );
    s["supervisor_reads"] = json!([{"surface":S,"coglet":C,"operation":"softmax.app.state","args":{},"correlation_id":READ,"timeout_us":1_000_000}]);
    s
}
fn binding() -> SurfaceBinding {
    SurfaceBinding::new(S.into(), vec![C.into()]).unwrap()
}
fn dec(raw: &[u8]) -> Value {
    // 0.2 is the superset decoder for inspection; assertions on 0.1 validity are explicit.
    coglet_profile_types::decode(ContractProfile::Surface02.rule("surface-message"), raw)
        .unwrap()
        .deserialized()
        .unwrap()
}
fn frozen_valid(raw: &[u8]) -> bool {
    coglet_types::decode("surface-message", raw).is_ok()
}
fn caps_for(entries: Value) -> Value {
    json!({"revision":1,"clock":1100,"entries":entries,"correlation_id":"2".repeat(32)})
}
fn face_entry() -> Value {
    json!({"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}})
}
fn speech_entry() -> Value {
    json!({"name":"speech.control","events":["speech.start"],"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":{"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start"}})
}
fn app_entry() -> Value {
    let uri = |n: &str| format!("https://example.invalid/softmax/1#{n}");
    json!({"name":"softmax.app","events":["softmax.app.view_changed"],"reads":["softmax.app.state"],"actions":["softmax.app.navigate"],"exclusive":["softmax.app_control"],
        "schemas":{"softmax.app.view_changed":uri("view"),"softmax.app.state":uri("state"),"softmax.app.navigate":uri("nav")}})
}
fn grants(epoch: u64, ops: &[&str], resources: &[&str]) -> Value {
    json!({"epoch":epoch,"entries":[{"coglet":C,"operations":ops,"resources":resources,"expires":100_000_000}],"correlation_id":"3".repeat(32)})
}
const FACE_SPEECH: &[&str] = &["expression.set_expression", "speech.start"];
const FACE_SPEECH_RES: &[&str] = &["display.face", "audio.mic", "audio.speaker"];
fn all_app_grants(epoch: u64) -> Value {
    grants(
        epoch,
        &[
            "expression.set_expression",
            "speech.start",
            "softmax.app.state",
            "softmax.app.view_changed",
            "softmax.app.navigate",
        ],
        &[
            "display.face",
            "audio.mic",
            "audio.speaker",
            "softmax.app_control",
        ],
    )
}
/// Each refusal needs a never-used epoch, or it would pass for the wrong reason.
fn epoch() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(100);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
}
fn view_event(seq: u64, extra: Option<(&str, Value)>) -> Value {
    let mut payload = json!({"revision":1,"window":"main","route":"home"});
    if let Some((k, v)) = extra {
        payload[k] = v;
    }
    json!({"source":S,"kind":"softmax.app.view_changed","seq":seq,"logical_time":1200+seq,"payload":payload,"correlation_id":"9".repeat(32)})
}
fn speech_event(seq: u64) -> Value {
    json!({"source":S,"kind":"speech.start","seq":seq,"logical_time":1200+seq,"payload":{"utterance":format!("{seq:x}").repeat(32),"position_ms":0},"correlation_id":"c".repeat(32)})
}

struct Peer {
    s: SurfaceSession,
    now: u64,
}
impl Peer {
    fn new(s: SurfaceSession, now: u64) -> Self {
        Self { s, now }
    }
    fn send(&mut self, kind: &str, body: Value) -> i32 {
        self.now += 1;
        let raw = cbor(&json!({"type":kind,"body":body})).unwrap();
        self.s.send(&raw, self.now)
    }
    fn raw_out(&mut self) -> Vec<Vec<u8>> {
        std::iter::from_fn(|| self.s.pop()).collect()
    }
    fn out(&mut self) -> Vec<Value> {
        self.raw_out().iter().map(|r| dec(r)).collect()
    }
    fn hello(&mut self, versions: Value) -> Value {
        let body = json!({"surface":S,"offer":{"versions":versions},"clock":1000,"correlation_id":"1".repeat(32)});
        assert_eq!(self.send("hello", body), 0);
        let out = self.out();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["type"], "hello_reply");
        out[0]["body"]["selection"]["version"].clone()
    }
    fn ready(&mut self, version: &str, entries: Value, grant: Value) {
        let offer = if version == "0.1" {
            json!(["0.1"])
        } else {
            json!(["0.1", "0.2"])
        };
        assert_eq!(self.hello(offer), version);
        assert_eq!(self.send("capabilities", caps_for(entries)), 0);
        assert_eq!(self.send("grants", grant), 0);
    }
    fn pump(&mut self) -> Vec<Vec<u8>> {
        self.now += 1;
        self.s.pump_dispatch(self.now).unwrap();
        self.raw_out()
    }
}
struct Env {
    dir: tempfile::TempDir,
}
impl Env {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn wal(&self) -> PathBuf {
        self.dir.path().join("state.wal")
    }
    fn open(
        &self,
        setup: &Value,
        raw: Option<&[u8]>,
        profile: ContractProfile,
    ) -> Arc<Mutex<RuntimeService>> {
        Arc::new(Mutex::new(match raw {
            Some(raw) => {
                RuntimeService::open_with_catalog(&self.wal(), setup, 128, catalog(raw, profile))
                    .unwrap()
            }
            None => RuntimeService::open(&self.wal(), setup, 128).unwrap(),
        }))
    }
    fn state(&self) -> Value {
        let copy = self.dir.path().join("snapshot.wal");
        std::fs::copy(self.wal(), &copy).unwrap();
        DurableStore::open(&copy).unwrap().state().unwrap().clone()
    }
}
fn session(service: &Arc<Mutex<RuntimeService>>, now: u64) -> Peer {
    Peer::new(
        SurfaceSession::with_service(binding(), service.clone()).unwrap(),
        now,
    )
}

#[test]
fn wire_selection_is_negotiated_minor_not_local_ceiling() {
    // Default constructor: exact 0.1 even when the peer offers 0.2.
    let mut p = Peer::new(SurfaceSession::new(binding()), 0);
    assert_eq!(p.hello(json!(["0.1", "0.2"])), json!("0.1"));
    assert_eq!(p.s.wire_profile(), ContractProfile::Surface01);
    // Local 0.2 ceiling, catalog-free and with catalog: highest common minor.
    let raw = app_catalog();
    for (service_raw, label) in [(None, "bare"), (Some(raw.as_slice()), "pinned")] {
        let env = Env::new();
        let mut setup = app_setup();
        if service_raw.is_none() {
            setup.as_object_mut().unwrap().remove("supervisor_reads");
            setup["manifest"]["subscriptions"] = json!(["speech.start"]);
            setup["initial_actions"] = json!([face_action()]);
            setup["initial_effect_ids"]["actions"] = json!([FACE]);
        }
        let service = env.open(&setup, service_raw, ContractProfile::Surface02);
        for (offer, expected) in [
            (json!(["0.1", "0.2"]), "0.2"),
            (json!(["0.2"]), "0.2"),
            (json!(["0.1"]), "0.1"),
            (json!(["0.2", "0.1", "0.2"]), "0.2"),
        ] {
            let mut p = session(&service, 0);
            assert_eq!(p.hello(offer.clone()), json!(expected), "{label} {offer}");
            assert_eq!(
                p.s.wire_profile(),
                if expected == "0.2" {
                    ContractProfile::Surface02
                } else {
                    ContractProfile::Surface01
                }
            );
        }
        // A peer offering nothing common is refused, never relabeled.
        let mut p = session(&service, 0);
        let body = json!({"surface":S,"offer":{"versions":["0.3"]},"clock":1000,"correlation_id":"1".repeat(32)});
        assert_eq!(p.send("hello", body), 0);
        assert_eq!(p.out()[0]["body"]["error"]["code"], "unsupported_version");
    }
    // A local 0.1 service (default) never selects 0.2, and 0.2-only peers are refused.
    let env = Env::new();
    let service = env.open(
        &setup(ContractProfile::Surface01, &[], &[]),
        None,
        ContractProfile::Surface01,
    );
    let mut p = session(&service, 0);
    assert_eq!(p.hello(json!(["0.1", "0.2"])), json!("0.1"));
    let mut p = session(&service, 0);
    let body = json!({"surface":S,"offer":{"versions":["0.2"]},"clock":1000,"correlation_id":"1".repeat(32)});
    p.send("hello", body);
    assert_eq!(p.out()[0]["body"]["error"]["code"], "unsupported_version");
}

#[test]
fn selected_02_app_event_read_action_through_wire() {
    let raw = app_catalog();
    let env = Env::new();
    let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
    let mut p = session(&service, 0);
    p.ready(
        "0.2",
        json!([face_entry(), speech_entry(), app_entry()]),
        all_app_grants(1),
    );
    // Trusted supervisor read: original op is durable and selected output validated.
    let sent = p.pump();
    let msgs: Vec<_> = sent.iter().map(|r| dec(r)).collect();
    let kinds: Vec<_> = msgs.iter().map(|m| m["type"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["read", "act", "act"], "read then ordered actions");
    assert_eq!(msgs[0]["body"]["capability"], "softmax.app.state");
    assert_eq!(
        msgs[1]["body"]["action"]["capability"],
        "expression.set_expression"
    );
    assert_eq!(
        msgs[2]["body"]["action"]["capability"],
        "softmax.app.navigate"
    );
    assert!(
        frozen_valid(&sent[1]),
        "built-in envelope remains 0.1 bytes"
    );
    assert!(!frozen_valid(&sent[2]), "new-name envelope is 0.2-only");
    assert!(!frozen_valid(&sent[0]), "new-name read is 0.2-only");
    // Selected read validation: sibling/other output rejected, valid accepted.
    let bad = |value: Value| {
        let env = Env::new();
        let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
        let mut p = session(&service, 0);
        p.ready(
            "0.2",
            json!([face_entry(), speech_entry(), app_entry()]),
            all_app_grants(1),
        );
        p.pump();
        p.send(
            "read_reply",
            json!({"correlation_id":READ,"ok":true,"value":value,"snapshot":1200}),
        )
    };
    assert_eq!(bad(json!({"revision":1,"extra":true})), -2);
    assert_eq!(bad(json!({"revision":"1"})), -2);
    assert_eq!(bad(json!({"paused":false})), -2);
    assert_eq!(
        p.send(
            "read_reply",
            json!({"correlation_id":READ,"ok":true,"value":{"revision":3},"snapshot":1200})
        ),
        0
    );
    // Event admission: exact canonical bytes retained, built-in speech unchanged.
    let event = view_event(1, None);
    assert_eq!(p.send("event", event.clone()), 0);
    assert_eq!(p.send("event", speech_event(2)), 0);
    let state = env.state();
    let saved: Vec<_> = state["private_events"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v["event"].clone())
        .collect();
    assert!(
        saved
            .iter()
            .any(|e| cbor(e).unwrap() == cbor(&event).unwrap())
    );
    assert!(saved.iter().any(|e| e["kind"] == "speech.start"));
    // Action accept + outcome with selected output validation; extra/non-empty output fails.
    let accept = json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true});
    assert_eq!(p.send("accept", accept), 0);
    let outcome = |result: Value, seq: u64| json!({"source":S,"kind":"action.outcome","seq":seq,"logical_time":1300+seq,"payload":{"id":APP,"correlation_id":"7".repeat(32),"status":"succeeded","result":result},"correlation_id":"7".repeat(32)});
    assert_eq!(
        p.send("event", outcome(json!({"x":1}), 3)),
        -2,
        "non-empty result refused"
    );
    let mut p = session(&service, 5_000);
    p.ready(
        "0.2",
        json!([face_entry(), speech_entry(), app_entry()]),
        all_app_grants(2),
    );
    let sent = p.pump();
    assert!(
        sent.iter().any(|r| dec(r)["body"]["action"]["id"] == APP),
        "retained retry at 0.2"
    );
    assert_eq!(
        p.send(
            "accept",
            json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true})
        ),
        0
    );
    assert_eq!(p.send("event", outcome(json!({}), 4)), 0);
    assert_eq!(
        env.state()["dispatches"][APP]["outcome"]["status"],
        "succeeded"
    );
}

#[test]
fn selected_02_invalid_fields_and_names_fail_closed() {
    let raw = app_catalog();
    let cases: Vec<(&str, Value, i32)> = vec![
        (
            "extra payload field",
            view_event(1, Some(("x", json!(1)))),
            -2,
        ),
        (
            "bad revision type",
            view_event(1, Some(("revision", json!("one")))),
            -2,
        ),
        (
            "unknown product event",
            {
                let mut e = view_event(1, None);
                e["kind"] = json!("softmax.app.unknown");
                e
            },
            -2,
        ),
        (
            "four components",
            {
                let mut e = view_event(1, None);
                e["kind"] = json!("softmax.app.view.changed");
                e
            },
            -2,
        ),
        (
            "prefix without component",
            {
                let mut e = view_event(1, None);
                e["kind"] = json!("softmax.app.");
                e
            },
            -2,
        ),
        (
            "wrong source",
            {
                let mut e = view_event(1, None);
                e["source"] = json!(
                    "surface:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                );
                e
            },
            -2,
        ),
    ];
    for (label, event, expected) in cases {
        let env = Env::new();
        let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
        let mut p = session(&service, 0);
        p.ready(
            "0.2",
            json!([face_entry(), speech_entry(), app_entry()]),
            all_app_grants(1),
        );
        assert_eq!(p.send("event", event), expected, "{label}");
        assert!(p.s.is_closed(), "{label}");
    }
    // Unadvertised/ungranted product names are not authority at 0.2 either.
    let env = Env::new();
    let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
    let mut p = session(&service, 0);
    assert_eq!(p.hello(json!(["0.2"])), json!("0.2"));
    let mut caps = app_entry();
    caps["actions"] = json!(["softmax.app.navigate", "softmax.app.reset"]);
    assert_eq!(p.send("capabilities", caps_for(json!([caps]))), -2);
    // Frozen-grammar negatives stay refused at selected 0.2.
    let mut p = session(&service, 0);
    p.hello(json!(["0.2"]));
    let mut entry = app_entry();
    entry["name"] = json!("softmax.app.extra.deep");
    assert_eq!(p.send("capabilities", caps_for(json!([entry]))), -2);
}

#[test]
fn default_and_ceiling_01_refuse_new_names_even_when_peer_offers_02() {
    // Default constructor, no service/catalog.
    let mut p = Peer::new(SurfaceSession::new(binding()), 0);
    assert_eq!(p.hello(json!(["0.1", "0.2"])), json!("0.1"));
    assert_eq!(p.send("capabilities", caps_for(json!([app_entry()]))), -2);
    // Local 0.1 catalog-free service and event/grant names.
    let env = Env::new();
    let service = env.open(
        &setup(ContractProfile::Surface01, &[], &[]),
        None,
        ContractProfile::Surface01,
    );
    for body_kind in ["caps", "grants", "event"] {
        let mut p = session(&service, 0);
        assert_eq!(p.hello(json!(["0.1", "0.2"])), json!("0.1"));
        let status = match body_kind {
            "caps" => p.send("capabilities", caps_for(json!([app_entry()]))),
            "grants" => {
                assert_eq!(p.send("capabilities", caps_for(json!([face_entry()]))), 0);
                p.send("grants", all_app_grants(1))
            }
            _ => p.send("event", view_event(1, None)),
        };
        assert_eq!(status, -2, "{body_kind}");
    }
}

#[test]
fn local_02_downgrade_keeps_face_speech_and_withholds_every_product_name() {
    let raw = app_catalog();
    let env = Env::new();
    let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
    // Peer offers only 0.1: built-in advertisement/grants/event/action remain usable.
    let mut p = session(&service, 0);
    p.ready(
        "0.1",
        json!([face_entry(), speech_entry()]),
        grants(1, FACE_SPEECH, FACE_SPEECH_RES),
    );
    let sent = p.pump();
    let msgs: Vec<_> = sent.iter().map(|r| dec(r)).collect();
    assert_eq!(
        msgs.len(),
        1,
        "only Face action; app action/read stay unsent"
    );
    assert_eq!(msgs[0]["body"]["action"]["id"], FACE);
    assert!(sent.iter().all(|r| frozen_valid(r)));
    assert_eq!(p.send("event", speech_event(1)), 0);
    assert_eq!(
        p.send(
            "accept",
            json!({"id":FACE,"correlation_id":"5".repeat(32),"accepted":true})
        ),
        0
    );
    // The saved app dispatch/read stay pending but unavailable, and not rewritten.
    let state = env.state();
    assert_eq!(state["contract_profile"], "surface-0.2");
    assert_eq!(state["catalog_pin"], digest(&raw));
    assert!(
        state["dispatches"].get(APP).is_none(),
        "never prepared at 0.1"
    );
    assert_eq!(state["supervisor_reads"][READ]["attempted"], false);
    // Every product-name route is refused individually on downgraded links.
    type Refusal = (&'static str, Box<dyn Fn(&mut Peer) -> i32>);
    let refusals: Vec<Refusal> = vec![
        (
            "app entry+ops",
            Box::new(|p| p.send("capabilities", caps_for(json!([face_entry(), app_entry()])))),
        ),
        (
            "EMPTY app entry",
            Box::new(|p| {
                p.send("capabilities", caps_for(json!([face_entry(), {"name":"softmax.app","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}])))
            }),
        ),
        (
            "EMPTY spare entry",
            Box::new(|p| {
                p.send("capabilities", caps_for(json!([face_entry(), {"name":"softmax.app_spare","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}])))
            }),
        ),
        (
            "namespace-only entry",
            Box::new(|p| {
                p.send("capabilities", caps_for(json!([face_entry(), {"name":"softmax.other","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}])))
            }),
        ),
        (
            "resource on builtin entry",
            Box::new(|p| {
                let mut e = face_entry();
                e["exclusive"] = json!(["display.face", "softmax.app_control"]);
                p.send("capabilities", caps_for(json!([e])))
            }),
        ),
        (
            "grant app op",
            Box::new(|p| {
                assert_eq!(
                    p.send(
                        "capabilities",
                        caps_for(json!([face_entry(), speech_entry()]))
                    ),
                    0
                );
                p.send("grants", all_app_grants(epoch()))
            }),
        ),
        (
            "grant app resource",
            Box::new(|p| {
                assert_eq!(
                    p.send(
                        "capabilities",
                        caps_for(json!([face_entry(), speech_entry()]))
                    ),
                    0
                );
                p.send(
                    "grants",
                    grants(
                        epoch(),
                        FACE_SPEECH,
                        &[
                            "display.face",
                            "audio.mic",
                            "audio.speaker",
                            "softmax.app_control",
                        ],
                    ),
                )
            }),
        ),
        (
            "app event",
            Box::new(|p| {
                assert_eq!(
                    p.send(
                        "capabilities",
                        caps_for(json!([face_entry(), speech_entry()]))
                    ),
                    0
                );
                assert_eq!(
                    p.send("grants", grants(epoch(), FACE_SPEECH, FACE_SPEECH_RES)),
                    0
                );
                p.send("event", view_event(1, None))
            }),
        ),
        (
            "app read reply",
            Box::new(|p| {
                p.send(
                    "read_reply",
                    json!({"correlation_id":READ,"ok":true,"value":{"revision":1},"snapshot":1}),
                )
            }),
        ),
        (
            "app action envelope from peer",
            Box::new(|p| {
                p.send(
                    "accept",
                    json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true}),
                )
            }),
        ),
    ];
    for (label, run) in refusals {
        let mut p = session(&service, 10_000);
        assert_eq!(p.hello(json!(["0.1"])), json!("0.1"), "{label}");
        assert_eq!(run(&mut p), -2, "{label}");
        assert!(p.s.is_closed(), "{label}");
    }
    // A refusal never rewrote the stored identity or consumed the pending app work.
    let after = env.state();
    assert_eq!(after["contract_profile"], "surface-0.2");
    assert_eq!(after["catalog_pin"], digest(&raw));
    assert_eq!(after["supervisor_reads"][READ]["attempted"], false);
}

#[test]
fn downgraded_reconnect_cannot_retry_retained_app_dispatch_but_keeps_face_bytes() {
    let raw = app_catalog();
    let env = Env::new();
    let service = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
    let mut p = session(&service, 0);
    p.ready(
        "0.2",
        json!([face_entry(), speech_entry(), app_entry()]),
        all_app_grants(1),
    );
    let first = p.pump();
    assert_eq!(first.len(), 3);
    assert_eq!(
        p.send("event", view_event(1, None)),
        0,
        "queued app event at 0.2"
    );
    assert_eq!(env.state()["private_events"].as_object().unwrap().len(), 1);
    let (face_bytes, app_bytes) = (first[1].clone(), first[2].clone());
    // Seed a real accepted retained app dispatch at 0.2, so later 0.1 rejection of
    // an outcome can only be the downgrade fence (not "never accepted").
    assert_eq!(
        p.send(
            "accept",
            json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true})
        ),
        0
    );
    assert_eq!(env.state()["dispatches"][APP]["accept"]["accepted"], true);
    let before = env.state();
    drop(p);
    // Reconnect at 0.1 with an app grant only for Face/speech.
    let mut q = session(&service, 2_000_000);
    q.ready(
        "0.1",
        json!([face_entry(), speech_entry()]),
        grants(2, FACE_SPEECH, FACE_SPEECH_RES),
    );
    let retry = q.pump();
    assert_eq!(
        retry,
        vec![face_bytes.clone()],
        "Face retry is the exact retained bytes"
    );
    assert!(
        service
            .lock()
            .unwrap()
            .prepare_dispatch(&q.s, APP, q.now)
            .is_err(),
        "retained app envelope is unavailable at 0.1"
    );
    assert_eq!(
        service
            .lock()
            .unwrap()
            .prepare_dispatch(&q.s, FACE, q.now)
            .unwrap(),
        face_bytes
    );
    assert!(q.s.pop().is_none());
    drop(q);
    // Late app accept/outcome for the retained ID cannot be recorded on 0.1.
    let mut r = session(&service, 3_000_000);
    r.ready(
        "0.1",
        json!([face_entry(), speech_entry()]),
        grants(3, FACE_SPEECH, FACE_SPEECH_RES),
    );
    assert_eq!(
        r.send(
            "accept",
            json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true})
        ),
        -2
    );
    drop(r);
    let mut r = session(&service, 4_000_000);
    r.ready(
        "0.1",
        json!([face_entry(), speech_entry()]),
        grants(4, FACE_SPEECH, FACE_SPEECH_RES),
    );
    // Fresh seq (> admitted 1) so only the profile fence can refuse it.
    let outcome = |seq: u64| json!({"source":S,"kind":"action.outcome","seq":seq,"logical_time":1999+seq,"payload":{"id":APP,"correlation_id":"7".repeat(32),"status":"succeeded","result":{}},"correlation_id":"7".repeat(32)});
    assert_eq!(r.send("event", outcome(9)), -2);
    assert!(r.s.is_closed());
    // Old 0.2 queued event cannot be restored; durable identity/record unchanged.
    let after = env.state();
    assert!(after["private_events"].as_object().unwrap().is_empty());
    assert_eq!(after["dispatches"][APP], before["dispatches"][APP]);
    assert_eq!(
        after["dispatches"][FACE]["envelope_hex"],
        before["dispatches"][FACE]["envelope_hex"]
    );
    assert_eq!(after["contract_profile"], "surface-0.2");
    assert_eq!(after["catalog_pin"], digest(&raw));
    // A fresh 0.2 reconnect needs fresh grants but reconciles the retained bytes.
    let mut s = session(&service, 5_000_000);
    s.ready(
        "0.2",
        json!([face_entry(), speech_entry(), app_entry()]),
        all_app_grants(5),
    );
    let _ = s.pump();
    assert_eq!(
        service
            .lock()
            .unwrap()
            .prepare_dispatch(&s.s, APP, s.now)
            .unwrap(),
        app_bytes,
        "same retained app bytes at 0.2"
    );
    // Positive historical receipt on selected 0.2 with a fresh seq: same ID succeeds.
    assert!(env.state()["dispatches"][APP]["outcome"].is_null());
    assert_eq!(s.send("event", outcome(10)), 0);
    assert_eq!(
        env.state()["dispatches"][APP]["outcome"]["status"],
        "succeeded"
    );
    drop((s, r));
    drop(service);
    // Recovery with the same pair still opens after downgrade history.
    let reopened = env.open(&app_setup(), Some(&raw), ContractProfile::Surface02);
    assert_eq!(
        reopened.lock().unwrap().contract_profile(),
        ContractProfile::Surface02
    );
}

#[test]
fn downgrade_denies_generic_two_component_catalog_but_default_01_keeps_it() {
    // Generic pinned two-component product catalog on default 0.1 keeps behavior.
    let game: Value = serde_json::from_slice(GAME).unwrap();
    let mut entries = Vec::new();
    for (name, resources) in game["entries"].as_object().unwrap() {
        let mut e = json!({"name":name,"events":[],"reads":[],"actions":[],"exclusive":resources,"schemas":{}});
        for (op, spec) in game["operations"].as_object().unwrap() {
            if spec["entry"] != *name {
                continue;
            }
            let d = spec["direction"].as_str().unwrap();
            e[match d {
                "event" => "events",
                "read" => "reads",
                _ => "actions",
            }]
            .as_array_mut()
            .unwrap()
            .push(json!(op));
            e["schemas"][op] = spec[if d == "read" { "output" } else { "input" }].clone();
        }
        entries.push(e);
    }
    let game_caps = json!(entries);
    let mut s01 = setup(ContractProfile::Surface01, &["game.observation"], &[]);
    let env = Env::new();
    let service = env.open(&s01, Some(GAME), ContractProfile::Surface01);
    let mut p = session(&service, 0);
    p.ready(
        "0.1",
        game_caps.clone(),
        grants(1, &["game.state", "game.move"], &["game.control"]),
    );
    // Same catalog provisioned at 0.2 and negotiated down: every game name denied,
    // including empty entries and bare resource names.
    s01["contract_profile"] = json!("surface-0.2");
    let env = Env::new();
    let service = env.open(&s01, Some(GAME), ContractProfile::Surface02);
    for caps in [
        game_caps.clone(),
        json!([{"name":"game.view","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}]),
        json!([{"name":"game.empty","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}]),
        json!([face_entry(), {"name":"speech.control","events":[],"reads":[],"actions":[],"exclusive":["game.control"],"schemas":{}}]),
    ] {
        let mut p = session(&service, 0);
        assert_eq!(p.hello(json!(["0.1"])), json!("0.1"));
        assert_eq!(p.send("capabilities", caps_for(caps)), -2);
    }
    let mut p = session(&service, 0);
    p.ready(
        "0.1",
        json!([face_entry()]),
        grants(1, &["expression.set_expression"], &["display.face"]),
    );
    // The same service/profile at negotiated 0.2 accepts the generic names again.
    let mut p = session(&service, 0);
    p.ready(
        "0.2",
        game_caps,
        grants(2, &["game.state", "game.move"], &["game.control"]),
    );
}

// ------------------------------------------------------------ daemon/socket

struct Host {
    dir: PathBuf,
    child: Option<Child>,
    raw: Vec<u8>,
}
struct Link(UnixStream);
impl Host {
    fn new(label: &str, setup: &Value, raw: Vec<u8>) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "swp-{}-{}",
            std::process::id(),
            &digest(label.as_bytes())[..10]
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("binding.json"),
            serde_json::to_vec(&json!({"surface":S,"permitted_coglets":[C]})).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("setup.json"), serde_json::to_vec(setup).unwrap()).unwrap();
        std::fs::write(dir.join("catalog.json"), &raw).unwrap();
        Self {
            dir,
            child: None,
            raw,
        }
    }
    fn start(&mut self, profile_flag: bool) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cogletd"));
        c.arg("--surface-service-socket")
            .arg(self.dir.join("sock/control"))
            .arg(self.dir.join("binding.json"))
            .arg(unsafe { libc::geteuid() }.to_string())
            .arg(self.dir.join("state.wal"))
            .arg(self.dir.join("setup.json"))
            .arg("--trusted-extension-catalog")
            .arg(self.dir.join("catalog.json"))
            .arg(digest(&self.raw));
        if profile_flag {
            c.args(["--contract-profile", "surface-0.2"]);
        }
        self.child = Some(c.stdout(Stdio::null()).spawn().unwrap());
    }
    fn connect(&mut self) -> Link {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(s) = UnixStream::connect(self.dir.join("sock/control")) {
                s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                return Link(s);
            }
            assert!(
                self.child.as_mut().unwrap().try_wait().unwrap().is_none(),
                "daemon exited before socket readiness"
            );
            assert!(Instant::now() < deadline, "daemon startup timeout");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            child.wait().unwrap();
        }
        let _ = std::fs::remove_file(self.dir.join("sock/control"));
    }
    /// Copy of the WAL taken while the daemon still owns the exclusive lock.
    fn snapshot(&self) -> Value {
        let copy = self.dir.join("snapshot.wal");
        std::fs::copy(self.dir.join("state.wal"), &copy).unwrap();
        DurableStore::open(&copy).unwrap().state().unwrap().clone()
    }
    fn state(&self) -> Value {
        DurableStore::open(&self.dir.join("state.wal"))
            .unwrap()
            .state()
            .unwrap()
            .clone()
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
impl Link {
    fn send(&mut self, kind: &str, body: Value) {
        let raw = cbor(&json!({"type":kind,"body":body})).unwrap();
        self.0.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
        self.0.write_all(&raw).unwrap();
    }
    fn frame(&mut self) -> Option<(Vec<u8>, Value)> {
        let mut h = [0; 4];
        match self.0.read_exact(&mut h) {
            Ok(()) => (),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                ) =>
            {
                return None;
            }
            Err(e) => panic!("socket read: {e}"),
        }
        let mut raw = vec![0; u32::from_be_bytes(h) as usize];
        self.0.read_exact(&mut raw).unwrap();
        let m = dec(&raw);
        Some((raw, m))
    }
    fn recv(&mut self, kind: &str) -> (Vec<u8>, Value) {
        loop {
            let (raw, m) = self
                .frame()
                .unwrap_or_else(|| panic!("closed waiting for {kind}"));
            if m["type"] == kind {
                return (raw, m);
            }
            assert_eq!(m["type"], "health", "unexpected {m} waiting for {kind}");
        }
    }
    fn barrier(&mut self) {
        self.send(
            "health",
            json!({"state":"ready","clock":1400,"pending":0,"correlation_id":"d".repeat(32)}),
        );
        loop {
            let (_, m) = self.recv("health");
            if m["body"]["correlation_id"] == "d".repeat(32) {
                return;
            }
        }
    }
    /// EOF/reset or explicit error is refusal; timeout is not.
    fn refused(&mut self) {
        loop {
            match self.frame() {
                None => return,
                Some((_, m)) if m["type"] == "error" => return,
                // Retained built-in dispatches reconcile on hello, before any
                // capability message; that is existing behavior, not authority.
                Some((_, m)) if m["type"] == "health" || m["type"] == "act" => (),
                Some((_, m)) => panic!("unexpected frame during refusal: {m}"),
            }
        }
    }
}

#[test]
#[allow(non_snake_case)]
fn CogletdWireProfileNegotiationAndDowngradeTest() {
    let raw = app_catalog();
    let mut h = Host::new("wire", &app_setup(), raw.clone());
    let setup_text = serde_json::to_vec(&app_setup()).unwrap();
    assert!(!setup_text.is_empty());
    h.start(false); // setup.contract_profile selects the ceiling; flag omitted
    // Daemon without matching flag must not have started: setup says 0.2, flag absent.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if h.child.as_mut().unwrap().try_wait().unwrap().is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "selector conflict did not fail startup"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    h.child = None;
    // Matching trusted selectors: use the setup without its own field plus the flag.
    let mut plain = app_setup();
    plain.as_object_mut().unwrap().remove("contract_profile");
    std::fs::write(
        h.dir.join("setup.json"),
        serde_json::to_vec(&plain).unwrap(),
    )
    .unwrap();
    h.start(true);
    let hello = |versions: Value| json!({"surface":S,"offer":{"versions":versions},"clock":1000,"correlation_id":"1".repeat(32)});
    let caps = |entries: Value| caps_for(entries);
    let mut l = h.connect();
    l.send("hello", hello(json!(["0.1", "0.2"])));
    assert_eq!(
        l.recv("hello_reply").1["body"]["selection"]["version"],
        "0.2"
    );
    l.send(
        "capabilities",
        caps(json!([face_entry(), speech_entry(), app_entry()])),
    );
    l.send("grants", all_app_grants(1));
    // Native socket delivers the app read and actions (new names on 0.2 only).
    let read = l.recv("read");
    assert_eq!(read.1["body"]["capability"], "softmax.app.state");
    assert!(!frozen_valid(&read.0));
    let (face_raw, face) = l.recv("act");
    assert_eq!(face["body"]["action"]["id"], FACE);
    let (app_raw, app) = l.recv("act");
    assert_eq!(app["body"]["action"]["capability"], "softmax.app.navigate");
    assert!(!frozen_valid(&app_raw));
    l.send(
        "read_reply",
        json!({"correlation_id":READ,"ok":true,"value":{"revision":2},"snapshot":1200}),
    );
    l.send("event", view_event(1, None));
    l.send("event", speech_event(2));
    l.barrier();
    let before = h.snapshot();
    // Invalid field on the same link: socket closes.
    l.send("event", view_event(3, Some(("extra", json!(1)))));
    l.refused();
    h.stop();
    drop(l);
    assert_eq!(before["contract_profile"], "surface-0.2");
    assert_eq!(before["catalog_pin"], digest(&raw));
    assert_eq!(before["private_events"].as_object().unwrap().len(), 2);
    assert!(before["dispatches"][APP]["envelope_hex"].is_string());
    assert_eq!(
        before["supervisor_reads"][READ]["response"]["value"],
        json!({"revision":2})
    );

    // Restart: peer offers only 0.1. Face/speech work; product names and retry fenced.
    h.start(true);
    let mut l = h.connect();
    l.send("hello", hello(json!(["0.1"])));
    assert_eq!(
        l.recv("hello_reply").1["body"]["selection"]["version"],
        "0.1"
    );
    l.send("capabilities", caps(json!([face_entry(), speech_entry()])));
    l.send("grants", grants(2, FACE_SPEECH, FACE_SPEECH_RES));
    let retry = l.recv("act");
    assert_eq!(retry.0, face_raw, "Face retry exact retained bytes");
    assert_eq!(retry.1["body"]["action"]["id"], FACE);
    l.send("event", speech_event(3));
    l.barrier(); // any app act/read would arrive before the barrier reply
    l.send("event", view_event(4, None));
    l.refused();
    for bad in [
        caps(
            json!([face_entry(), {"name":"softmax.app","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}]),
        ),
        caps(json!([app_entry()])),
    ] {
        let mut l = h.connect();
        l.send("hello", hello(json!(["0.1"])));
        l.recv("hello_reply");
        l.send("capabilities", bad);
        l.refused();
    }
    h.stop();
    let after = h.state();
    assert_eq!(after["contract_profile"], "surface-0.2");
    assert_eq!(after["catalog_pin"], digest(&raw));
    assert_eq!(after["dispatches"][APP], before["dispatches"][APP]);
    assert_eq!(
        after["dispatches"][FACE]["envelope_hex"],
        before["dispatches"][FACE]["envelope_hex"]
    );
    assert!(
        after["private_events"]
            .as_object()
            .unwrap()
            .values()
            .all(|e| e["event"]["kind"] != "softmax.app.view_changed")
    );

    // Same app retry is reconciled byte-for-byte on a fresh 0.2 link.
    h.start(true);
    let mut l = h.connect();
    l.send("hello", hello(json!(["0.2"])));
    l.recv("hello_reply");
    l.send(
        "capabilities",
        caps(json!([face_entry(), speech_entry(), app_entry()])),
    );
    l.send("grants", all_app_grants(3));
    let mut seen = Vec::new();
    while seen.len() < 2 {
        seen.push(l.recv("act").0);
    }
    assert!(seen.contains(&app_bytes_from(&before)));
}
fn app_bytes_from(state: &Value) -> Vec<u8> {
    hex::decode(state["dispatches"][APP]["envelope_hex"].as_str().unwrap()).unwrap()
}

// ------------------------------------------------- real native-engine guests

/// WAMR serializes its whole lifetime globally; serialize every guest test here.
#[cfg(feature = "native-engine")]
static NATIVE: Mutex<()> = Mutex::new(());

/// Real WAMR guest whose `handle` emits exactly `action` (host emit import).
#[cfg(feature = "native-engine")]
fn emitting_guest(action: &Value) -> Vec<u8> {
    let raw = cbor(action).unwrap();
    let esc: String = raw.iter().map(|b| format!("\\{b:02x}")).collect();
    wat::parse_str(format!(
        "(module (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))
        (memory (export \"memory\") 1 2) (data (i32.const 0) \"{esc}\")
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32)
          i32.const 0 i32.const {} call $emit drop i32.const 0))",
        raw.len()
    ))
    .unwrap()
}

/// Native setup: 0.2 ceiling, real guest, event-driven (no timer), `required`
/// capabilities/resources in the manifest.
#[cfg(feature = "native-engine")]
fn native_setup(guest: &[u8], subs: &[&str], required: &[&str], outputs: &[&str]) -> Value {
    let mut s = setup(ContractProfile::Surface02, subs, &[]);
    s["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(guest));
    let raw = hex::decode(
        s["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut prov: Value = coglet_types::decode("hub-build-record", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    prov["rewritten_sha256"] = json!(digest(guest));
    s["initial_files"]["/code/build/provenance.cbor"] = json!(hex::encode(cbor(&prov).unwrap()));
    let b = &mut s["manifest"]["budgets"];
    b["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
    b["compute"] = json!(100_000);
    b["memory_pages"] = json!(2);
    b["wall_time_us"] = json!(500_000);
    b["host_bytes"] = json!(1_000_000);
    s["manifest"]["required_capabilities"] = json!(required);
    s["manifest"]["exclusive_outputs"] = json!(outputs);
    s["initial_timers"] = json!([]);
    s["initial_effect_ids"]["timers"] = json!([]);
    s["native_schedule"] = json!({"trace_id":s["inputs"]["trace_id"],"seed":7,"runtime_bytes":8_388_608,"stack_bytes":65_536});
    s
}
#[cfg(feature = "native-engine")]
fn open_native(wal: &std::path::Path, setup: &Value, raw: &[u8]) -> RuntimeService {
    let mut service = RuntimeService::open_with_catalog(
        wal,
        setup,
        128,
        catalog(raw, ContractProfile::Surface02),
    )
    .unwrap();
    service.supervised_now().unwrap();
    service
        .enable_native(serde_json::from_value(setup["native_schedule"].clone()).unwrap())
        .unwrap();
    service
}
#[cfg(feature = "native-engine")]
fn native_env(setup: &Value, raw: &[u8]) -> (Env, Arc<Mutex<RuntimeService>>) {
    let env = Env::new();
    let service = Arc::new(Mutex::new(open_native(&env.wal(), setup, raw)));
    (env, service)
}
#[cfg(feature = "native-engine")]
fn app_only_entry() -> Value {
    app_entry()
}

#[cfg(feature = "native-engine")]
#[test]
fn native_guest_selected_02_app_event_emits_action_persisted_and_replayed() {
    let _g = NATIVE.lock().unwrap_or_else(|e| e.into_inner());
    let raw = app_catalog();
    let guest = emitting_guest(&app_action());
    let s = native_setup(
        &guest,
        &["softmax.app.view_changed"],
        &["softmax.app.navigate"],
        &["softmax.app_control"],
    );
    let (env, service) = native_env(&s, &raw);
    let mut p = session(&service, 0);
    p.ready(
        "0.2",
        json!([app_only_entry()]),
        grants(
            1,
            &["softmax.app.view_changed", "softmax.app.navigate"],
            &["softmax.app_control"],
        ),
    );
    assert!(p.pump().is_empty(), "no guest run without an event");
    assert_eq!(p.send("event", view_event(1, None)), 0);
    let sent = p.pump();
    let acts: Vec<_> = sent
        .iter()
        .map(|r| dec(r))
        .filter(|m| m["type"] == "act")
        .collect();
    assert_eq!(acts.len(), 1, "guest emitted exactly one action");
    assert_eq!(
        acts[0]["body"]["action"]["capability"],
        "softmax.app.navigate"
    );
    assert_eq!(acts[0]["body"]["action"]["id"], APP);
    assert!(
        !frozen_valid(&sent[sent.len() - 1]),
        "app act is 0.2-only bytes"
    );
    assert_eq!(
        p.send(
            "accept",
            json!({"id":APP,"correlation_id":"7".repeat(32),"accepted":true})
        ),
        0
    );
    drop(p);
    drop(service);
    // Persisted WAL: real WAMR step committed with measured compute and the app action.
    let wal = env.wal();
    let reopened = open_native(&wal, &s, &raw);
    assert_eq!(reopened.contract_profile(), ContractProfile::Surface02);
    assert_eq!(reopened.records().len(), 1);
    let rec = &reopened.records()[0];
    assert_eq!(rec["status"], "committed");
    assert_eq!(rec["engine"]["build"], coglet_engine_wamr::BUILD);
    assert!(rec["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(rec["events"][0]["kind"], "softmax.app.view_changed");
    assert_eq!(rec["actions"].as_array().unwrap().len(), 1);
    assert_eq!(rec["actions"][0]["capability"], "softmax.app.navigate");
    let st = env.state();
    assert!(st["dispatches"][APP]["envelope_hex"].is_string());
    assert_eq!(st["dispatches"][APP]["accept"]["accepted"], true);
    // Re-execute the actual guest without any live surface/provider path.
    let mut replay = coglet_runtime::Runtime::from_fixture(&s).unwrap();
    let mut engine = coglet_runtime::NativeEngine::new(
        guest.clone(),
        &s["manifest"]["budgets"],
        8_388_608,
        65_536,
    )
    .unwrap();
    assert_eq!(
        replay
            .replay(std::slice::from_ref(rec), &mut engine)
            .unwrap(),
        vec![rec["output_hash"].as_str().unwrap()]
    );
    let rec_bytes = cbor(rec).unwrap();
    assert!(
        coglet_profile_types::decode(ContractProfile::Surface02.rule("step-record"), &rec_bytes)
            .is_ok()
    );
    assert!(coglet_types::decode("step-record", &rec_bytes).is_err());
}

#[cfg(feature = "native-engine")]
#[test]
fn native_downgraded_02_app_required_manifest_stays_unadmitted_but_builtin_guest_runs() {
    let _g = NATIVE.lock().unwrap_or_else(|e| e.into_inner());
    let raw = app_catalog();
    // App-required manifest, local 0.2 ceiling; old app event queued at 0.2 while the
    // navigate grant is withheld (guest cannot yet be admitted).
    let guest = emitting_guest(&app_action());
    let s = native_setup(
        &guest,
        &["softmax.app.view_changed", "speech.start"],
        &["softmax.app.navigate"],
        &["softmax.app_control"],
    );
    let (env, service) = native_env(&s, &raw);
    let mut p = session(&service, 0);
    p.ready(
        "0.2",
        json!([app_only_entry()]),
        grants(1, &["softmax.app.view_changed"], &[]),
    );
    assert_eq!(p.send("event", view_event(1, None)), 0);
    assert!(p.pump().is_empty());
    assert_eq!(env.state()["private_events"].as_object().unwrap().len(), 1);
    assert!(service.lock().unwrap().records().is_empty());
    drop(p);
    // Downgraded 0.1 links: the manifest needs app names, so it never runs, before
    // or after fresh Face/speech grants; the queued 0.2 event is purged, not run.
    for (i, now) in [(2u64, 1_000_000u64), (3, 2_000_000)] {
        let mut q = session(&service, now);
        q.ready(
            "0.1",
            json!([face_entry(), speech_entry()]),
            grants(i, FACE_SPEECH, FACE_SPEECH_RES),
        );
        assert!(q.pump().iter().all(|r| frozen_valid(r)));
        assert_eq!(q.send("event", speech_event(i)), 0);
        assert!(q.pump().is_empty(), "no guest/action at 0.1 (epoch {i})");
        // Fresh app grants are refused outright at 0.1.
        assert_eq!(q.send("grants", all_app_grants(epoch())), -2);
    }
    assert!(
        service.lock().unwrap().records().is_empty(),
        "guest never admitted"
    );
    assert!(env.state()["dispatches"].get(APP).is_none());
    assert!(
        env.state()["private_events"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    drop(service);

    // Unchanged built-in control: Face guest on speech.start still runs when the
    // same 0.2-ceiling service is downgraded to 0.1.
    let guest = emitting_guest(&face_action());
    let s = native_setup(
        &guest,
        &["speech.start"],
        &["expression.set_expression"],
        &["display.face"],
    );
    let (env, service) = native_env(&s, &raw);
    let mut q = session(&service, 0);
    q.ready(
        "0.1",
        json!([face_entry(), speech_entry()]),
        grants(1, FACE_SPEECH, FACE_SPEECH_RES),
    );
    assert_eq!(q.send("event", speech_event(1)), 0);
    let sent = q.pump();
    assert_eq!(sent.len(), 1);
    assert!(frozen_valid(&sent[0]));
    assert_eq!(dec(&sent[0])["body"]["action"]["id"], FACE);
    drop(q);
    drop(service);
    let st = env.state();
    assert_eq!(st["contract_profile"], "surface-0.2");
    assert!(st["dispatches"][FACE]["envelope_hex"].is_string());
}
