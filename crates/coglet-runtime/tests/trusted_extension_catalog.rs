//! Supervisor-pinned fixture catalog qualification on the production daemon.
//! All game payloads here are labeled test fixtures, not product registration.
#![cfg(any(target_os = "linux", target_os = "macos"))]
use coglet_runtime::{
    DurableStore, RuntimeService, SurfaceBinding, SurfaceSession, TrustedExtensionCatalog, cbor,
    digest,
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
const OTHER: &str = "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RAW: &[u8] = include_bytes!("../examples/trusted-extension-catalog.json");
#[path = "support/trusted_extension_cases.rs"]
mod extra;
fn catalog(raw: &[u8]) -> TrustedExtensionCatalog {
    TrustedExtensionCatalog::from_pinned_bytes(raw, &digest(raw)).unwrap()
}
fn artifact() -> Value {
    serde_json::from_slice(RAW).unwrap()
}
fn setup(actions: bool) -> Value {
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
    s["manifest"]["subscriptions"] = json!(["game.observation"]);
    if actions {
        s["initial_actions"] = json!([{"coglet":C,"id":"4".repeat(32),"capability":"game.move","args":{"tile":7},"deadline":50_000_000,"correlation_id":"5".repeat(32)}]);
        s["initial_effect_ids"]["actions"] = json!(["4".repeat(32)]);
    }
    s
}
fn capabilities(a: &Value) -> Value {
    let mut entries = Vec::new();
    for (name, resources) in a["entries"].as_object().unwrap() {
        let mut e = json!({"name":name,"events":[],"reads":[],"actions":[],"exclusive":resources,"schemas":{}});
        for (op, spec) in a["operations"].as_object().unwrap() {
            if spec["entry"] != *name {
                continue;
            }
            let direction = spec["direction"].as_str().unwrap();
            e[match direction {
                "event" => "events",
                "read" => "reads",
                _ => "actions",
            }]
            .as_array_mut()
            .unwrap()
            .push(json!(op));
            e["schemas"][op] = spec[if direction == "read" {
                "output"
            } else {
                "input"
            }]
            .clone();
        }
        entries.push(e);
    }
    json!({"revision":1,"clock":1100,"entries":entries,"correlation_id":"2".repeat(32)})
}
fn grants(ops: &[&str], owner: &str) -> Value {
    json!({"epoch":1,"entries":[{"coglet":owner,"operations":ops,"resources":["game.control","game.helper","game.overlay"],"expires":100_000_000}],"correlation_id":"3".repeat(32)})
}
fn all_ops() -> Vec<&'static str> {
    vec![
        "game.state",
        "game.tiles",
        "game.observation",
        "game.move",
        "game.helper_spawn",
        "game.highlight",
    ]
}
fn hello() -> Value {
    json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)})
}
fn observation(op: &str, args: Value, value: Value) -> Value {
    json!({"source":S,"kind":"game.observation","seq":1,"logical_time":1200,"payload":{"query_id":"6".repeat(32),"operation":op,"args":args,"value":value,"revision":1,"world_generation":1,"grant_epoch":1},"correlation_id":"7".repeat(32)})
}
fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}
struct Host {
    dir: PathBuf,
    setup: Value,
    raw: Vec<u8>,
    child: Option<Child>,
}
impl Host {
    fn new(label: &str, actions: bool, raw: &[u8], owners: &[&str]) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "tc-{}-{}",
            std::process::id(),
            &digest(label.as_bytes())[..10]
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let setup = setup(actions);
        std::fs::write(
            dir.join("binding.json"),
            serde_json::to_vec(&json!({"surface":S,"permitted_coglets":owners})).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("setup.json"), serde_json::to_vec(&setup).unwrap()).unwrap();
        std::fs::write(dir.join("catalog.json"), raw).unwrap();
        Self {
            dir,
            setup,
            raw: raw.to_vec(),
            child: None,
        }
    }
    fn command(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_cogletd"));
        c.arg("--surface-service-socket")
            .arg(self.dir.join("sock/control"))
            .arg(self.dir.join("binding.json"))
            .arg(unsafe { libc::geteuid() }.to_string())
            .arg(self.dir.join("state.wal"))
            .arg(self.dir.join("setup.json"));
        c
    }
    fn start(&mut self, pinned: bool) {
        let mut c = self.command();
        if pinned {
            c.arg("--trusted-extension-catalog")
                .arg(self.dir.join("catalog.json"))
                .arg(digest(&self.raw));
        }
        self.child = Some(c.stdout(Stdio::null()).spawn().unwrap());
    }
    fn connect(&mut self) -> Link {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(s) = UnixStream::connect(self.dir.join("sock/control")) {
                s.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
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
    fn state(&self) -> Value {
        let store = DurableStore::open(&self.dir.join("state.wal")).unwrap();
        store.state().unwrap().clone()
    }
    fn startup_fails(&self, args: &[String]) {
        let mut child = self
            .command()
            .args(args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(!status.success(), "invalid startup succeeded");
                assert!(
                    !self.dir.join("sock/control").exists(),
                    "invalid startup exposed socket"
                );
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("invalid startup did not exit");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn pin_args(&self, pin: &str) -> Vec<String> {
        vec![
            "--trusted-extension-catalog".into(),
            self.dir.join("catalog.json").to_str().unwrap().into(),
            pin.into(),
        ]
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
struct Link(UnixStream);
impl Link {
    fn send(&mut self, kind: &str, body: Value) {
        let raw = cbor(&json!({"type":kind,"body":body})).unwrap();
        self.0.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
        self.0.write_all(&raw).unwrap();
    }
    fn recv(&mut self, kind: &str) -> Value {
        loop {
            let mut h = [0; 4];
            self.0.read_exact(&mut h).unwrap();
            let mut raw = vec![0; u32::from_be_bytes(h) as usize];
            self.0.read_exact(&mut raw).unwrap();
            let m = decode(&raw);
            if m["type"] == kind {
                return m;
            }
            assert_eq!(
                m["type"], "health",
                "unexpected response waiting for {kind}: {m}"
            );
        }
    }
    fn hello(&mut self) {
        self.send("hello", hello());
        self.recv("hello_reply");
    }
    fn ready(&mut self, a: &Value, ops: &[&str]) {
        self.hello();
        self.send("capabilities", capabilities(a));
        self.send("grants", grants(ops, C));
    }
    fn barrier(&mut self) -> Value {
        self.send(
            "health",
            json!({"state":"ready","clock":1400,"pending":0,"correlation_id":"9".repeat(32)}),
        );
        loop {
            let h = self.recv("health");
            if h["body"]["correlation_id"] == "9".repeat(32) {
                return h;
            }
        }
    }
    fn refused(&mut self) {
        // Production serve_session closes on fatal send before flushing its error queue.
        // EOF/reset is refusal; timeout is NOT evidence of refusal.
        loop {
            let mut header = [0; 4];
            match self.0.read_exact(&mut header) {
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::UnexpectedEof | std::io::ErrorKind::ConnectionReset
                    ) =>
                {
                    return;
                }
                Err(e) => panic!("expected explicit refusal or closed socket: {e}"),
                Ok(()) => (),
            }
            let mut raw = vec![0; u32::from_be_bytes(header) as usize];
            self.0.read_exact(&mut raw).unwrap();
            let m = decode(&raw);
            if m["type"] == "error" {
                assert!(m["body"]["error"]["code"].is_string());
                return;
            }
            assert_eq!(
                m["type"], "health",
                "unexpected response during refusal: {m}"
            );
        }
    }
}

#[test]
#[allow(non_snake_case)]
fn CogletdTrustedExtensionCatalogTest() {
    extra::daemon_reads();
    extra::resource_matrix();
    extra::fenced_results();
    let a = artifact();
    // Real production socket event admission, with exact canonical event bytes in WAL.
    let mut h = Host::new("positive-events", false, RAW, &[C]);
    h.start(true);
    let mut l = h.connect();
    l.ready(&a, &all_ops());
    let e = observation("game.state", json!({}), json!({"paused":false}));
    let mut tiles = observation("game.tiles", json!({"tile":7}), json!({"count":2}));
    tiles["seq"] = json!(2);
    tiles["logical_time"] = json!(1201);
    l.send("event", e.clone());
    l.send("event", tiles.clone());
    l.barrier();
    // Stop before disconnect purges private observations.
    h.stop();
    drop(l);
    let state = h.state();
    let events: Vec<_> = state["private_events"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v["event"].clone())
        .collect();
    assert_eq!(events.len(), 2);
    for event in [&e, &tiles] {
        assert!(
            events
                .iter()
                .any(|saved| cbor(saved).unwrap() == cbor(event).unwrap()),
            "exact admitted event bytes absent"
        );
    }
    assert_eq!(state["surface_sequences"][S][0], 2);
    drop(
        RuntimeService::open_with_catalog(&h.dir.join("state.wal"), &h.setup, 128, catalog(RAW))
            .unwrap(),
    );
    h.start(true);
    let mut l = h.connect();
    l.hello();
    l.barrier();
    h.stop();
    drop(l);

    // Each sibling's args AND value must be bound to the selected operation.
    let mut negative_events = vec![
        (
            "state-sibling-args",
            observation("game.state", json!({"tile":7}), json!({"paused":false})),
            all_ops(),
        ),
        (
            "state-sibling-value",
            observation("game.state", json!({}), json!({"count":2})),
            all_ops(),
        ),
        (
            "tiles-sibling-args",
            observation("game.tiles", json!({}), json!({"count":2})),
            all_ops(),
        ),
        (
            "tiles-sibling-value",
            observation("game.tiles", json!({"tile":7}), json!({"paused":false})),
            all_ops(),
        ),
        ("missing-event-grant", e.clone(), vec!["game.state"]),
        ("missing-read-grant", e.clone(), vec!["game.observation"]),
    ];
    let mut wrong_source = e.clone();
    wrong_source["source"] =
        json!("surface:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd");
    negative_events.push(("wrong-source", wrong_source, all_ops()));
    for (label, event, ops) in negative_events {
        let mut h = Host::new(label, false, RAW, &[C]);
        h.start(true);
        let mut l = h.connect();
        l.ready(&a, &ops);
        l.send("event", event);
        l.refused();
        h.stop();
        let state = h.state();
        assert!(
            state["private_events"].as_object().unwrap().is_empty(),
            "{label}"
        );
        assert!(
            state["surface_sequences"].get(S).is_none(),
            "{label} consumed source sequence"
        );
    }
    let mut h = Host::new("wrong-owner", false, RAW, &[OTHER]);
    h.start(true);
    let mut l = h.connect();
    l.hello();
    l.send("capabilities", capabilities(&a));
    l.send("grants", grants(&all_ops(), OTHER));
    l.send("event", e.clone());
    l.refused();
    h.stop();
    assert!(h.state()["private_events"].as_object().unwrap().is_empty());

    for label in [
        "wrong-entry",
        "wrong-direction",
        "wrong-uri",
        "wrong-resource",
        "multi-owner",
        "default-builtins-only",
    ] {
        let owners: Vec<_> = if label == "multi-owner" {
            vec![C, OTHER]
        } else {
            vec![C]
        };
        let mut h = Host::new(label, false, RAW, &owners);
        h.start(label != "default-builtins-only");
        let mut l = h.connect();
        l.hello();
        let mut caps = capabilities(&a);
        let index = caps["entries"]
            .as_array()
            .unwrap()
            .iter()
            .position(|e| e["name"] == "game.view")
            .unwrap();
        match label {
            "wrong-entry" => caps["entries"][index]["name"] = json!("game.motor"),
            "wrong-direction" => {
                caps["entries"][index]["events"] = json!(["game.state"]);
                caps["entries"][index]["reads"] = json!(["game.tiles"]);
            }
            "wrong-uri" => {
                caps["entries"][index]["schemas"]["game.state"] =
                    json!("https://example.invalid/game/1#tiles")
            }
            "wrong-resource" => caps["entries"][index]["exclusive"] = json!(["game.control"]),
            _ => (),
        }
        l.send("capabilities", caps);
        l.refused();
        h.stop();
        assert!(
            h.state()["surface_epochs"].as_object().unwrap().is_empty(),
            "{label} gained authority"
        );
    }
    let h = Host::new("startup-pins", false, RAW, &[C]);
    for pin in ["", "bad", &"0".repeat(64), &digest(RAW).to_uppercase()] {
        h.startup_fails(&h.pin_args(pin));
    }
    let mut missing_pin = h.pin_args("");
    missing_pin.pop();
    h.startup_fails(&missing_pin);
    let mut missing_file = h.pin_args(&digest(RAW));
    missing_file[1] = h.dir.join("absent.json").to_str().unwrap().into();
    h.startup_fails(&missing_file);

    // Fixture-only sibling action has a distinct output. Original game.move stays empty.
    // This is local synthetic test data, never an update to the example or production catalog.
    let mut sibling = a.clone();
    sibling["operations"]["game.helper_spawn"]["output"] =
        json!("https://example.invalid/game/1#state");
    let sibling_raw = serde_json::to_vec(&sibling).unwrap();
    for (label, result, good) in [
        ("move-sibling-result", json!({"paused":false}), false),
        ("move-valid-result", json!({}), true),
        (
            "helper-valid-sibling-control",
            json!({"paused":false}),
            true,
        ),
        ("move-wrong-correlation", json!({}), false),
    ] {
        let mut h = Host::new(label, true, &sibling_raw, &[C]);
        let operation = if label == "helper-valid-sibling-control" {
            "game.helper_spawn"
        } else {
            "game.move"
        };
        if operation == "game.helper_spawn" {
            h.setup["initial_actions"][0]["capability"] = json!(operation);
            h.setup["initial_actions"][0]["args"] = json!({});
            std::fs::write(
                h.dir.join("setup.json"),
                serde_json::to_vec(&h.setup).unwrap(),
            )
            .unwrap();
        }
        h.start(true);
        let mut l = h.connect();
        l.ready(&sibling, &all_ops());
        let act = l.recv("act")["body"]["action"].clone();
        assert_eq!(act["capability"], operation);
        l.send(
            "accept",
            json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
        );
        assert_eq!(l.barrier()["body"]["pending"], 1);
        let mut outcome = json!({"id":act["id"],"correlation_id":act["correlation_id"],"status":"succeeded","result":result});
        if label == "move-wrong-correlation" {
            outcome["correlation_id"] = json!("8".repeat(32));
        }
        l.send("event", json!({"source":S,"kind":"action.outcome","seq":1,"logical_time":1500,"payload":outcome,"correlation_id":act["correlation_id"]}));
        if good {
            assert_eq!(l.barrier()["body"]["pending"], 0);
        } else {
            l.refused();
        }
        h.stop();
        drop(l);
        let state = h.state();
        let d = &state["dispatches"]["4".repeat(32)];
        if good {
            assert_eq!(d["outcome"], outcome);
            assert_eq!(d["intent"]["capability"], operation);
            drop(
                RuntimeService::open_with_catalog(
                    &h.dir.join("state.wal"),
                    &h.setup,
                    128,
                    catalog(&sibling_raw),
                )
                .unwrap(),
            );
            assert!(RuntimeService::open(&h.dir.join("state.wal"), &h.setup, 128).is_err());
            assert!(
                RuntimeService::open_with_catalog(
                    &h.dir.join("state.wal"),
                    &h.setup,
                    128,
                    catalog(RAW)
                )
                .is_err()
            );
            h.startup_fails(&[]);
            // Changed, internally valid artifact+pin must still fail the WAL pin fence.
            std::fs::write(h.dir.join("catalog.json"), RAW).unwrap();
            h.startup_fails(&h.pin_args(&digest(RAW)));
            std::fs::write(h.dir.join("catalog.json"), &sibling_raw).unwrap();
            h.start(true);
            let mut l = h.connect();
            l.hello();
            assert_eq!(l.barrier()["body"]["pending"], 0);
            h.stop();
        } else {
            assert!(d["outcome"].is_null(), "sibling output committed");
        }
    }
}

#[test]
fn selected_read_reply_uses_original_correlation_operation() {
    for (label, op, args, sibling, valid) in [
        (
            "state-read",
            "game.state",
            json!({}),
            json!({"count":2}),
            json!({"paused":false}),
        ),
        (
            "tiles-read",
            "game.tiles",
            json!({"tile":7}),
            json!({"paused":false}),
            json!({"count":2}),
        ),
    ] {
        for bad in [true, false] {
            let h = Host::new(&format!("{label}-{bad}"), false, RAW, &[C]);
            let service = Arc::new(Mutex::new(
                RuntimeService::open_with_catalog(
                    &h.dir.join("state.wal"),
                    &h.setup,
                    128,
                    catalog(RAW),
                )
                .unwrap(),
            ));
            let mut s = SurfaceSession::with_service(
                SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
                service.clone(),
            )
            .unwrap();
            for (i, (kind, body)) in [
                ("hello", hello()),
                ("capabilities", capabilities(&artifact())),
                ("grants", grants(&all_ops(), C)),
            ]
            .into_iter()
            .enumerate()
            {
                assert_eq!(
                    s.send(
                        &cbor(&json!({"type":kind,"body":body})).unwrap(),
                        100 + i as u64
                    ),
                    0
                );
            }
            s.pop();
            assert!(
                s.request_read(OTHER, op, args.clone(), &"a".repeat(32), 1000, 103)
                    .is_err()
            );
            let wrong_args = if op == "game.state" {
                json!({"tile":7})
            } else {
                json!({})
            };
            assert!(
                s.request_read(C, op, wrong_args, &"b".repeat(32), 1000, 103)
                    .is_err()
            );
            s.request_read(C, op, args.clone(), &"c".repeat(32), 1000, 103)
                .unwrap();
            let request = decode(&s.pop().unwrap());
            assert_eq!(request["body"]["capability"], op);
            assert_eq!(request["body"]["args"], args);
            let reply = json!({"type":"read_reply","body":{"correlation_id":"c".repeat(32),"ok":true,"value":if bad { sibling.clone() } else { valid.clone() },"snapshot":1200}});
            let status = s.send(&cbor(&reply).unwrap(), 104);
            if bad {
                assert_ne!(status, 0);
                assert!(s.take_read_result(C, 105).is_none());
            } else {
                assert_eq!(status, 0);
                assert!(s.take_read_result(OTHER, 105).is_none());
                let r = s.take_read_result(C, 105).unwrap();
                assert_eq!(r["value"], valid);
                assert_eq!(r["correlation_id"], "c".repeat(32));
            }
            drop(s);
            drop(service);
        }
    }
}

#[test]
fn pinned_loader_rejects_untrusted_or_open_catalogs() {
    assert!(TrustedExtensionCatalog::from_pinned_bytes(RAW, "").is_err());
    assert!(TrustedExtensionCatalog::from_pinned_bytes(RAW, &"0".repeat(64)).is_err());
    let base = artifact();
    let mut cases: Vec<(&str, Value)> = Vec::new();
    for namespace in ["speech", "runtime", "action", "test", "sim"] {
        let mut a = base.clone();
        a["namespace"] = json!(namespace);
        cases.push(("reserved namespace", a));
    }
    for (label, path, value) in [
        ("version", "/version", json!(2)),
        (
            "unknown reference",
            "/rules/tile-query/fields/0/type/name",
            json!("absent"),
        ),
        (
            "recursive reference",
            "/rules/tile-query/fields/0/type/name",
            json!("tile-query"),
        ),
        ("open IR", "/rules/empty", json!({"op":"map","rest":true})),
        (
            "unknown IR key",
            "/rules/empty",
            json!({"op":"record","fields":[],"rest":true}),
        ),
        (
            "unbounded array",
            "/rules/empty",
            json!({"op":"array","min":0,"max":9999999,"item":{"op":"primitive","name":"bool"}}),
        ),
        (
            "wrong entry",
            "/operations/game.state/entry",
            json!("game.unknown"),
        ),
        (
            "unknown schema",
            "/operations/game.state/input",
            json!("https://example.invalid/missing"),
        ),
        (
            "wrong direction",
            "/operations/game.state/direction",
            json!("stream"),
        ),
        (
            "missing single owner",
            "/operations/game.state/single_owner",
            json!(false),
        ),
        (
            "unknown resource",
            "/operations/game.move/resources",
            json!(["game.unknown"]),
        ),
        (
            "duplicate resource",
            "/operations/game.move/resources",
            json!(["game.control", "game.control"]),
        ),
        (
            "unknown selection",
            "/operations/game.observation/selection/reads",
            json!(["game.absent"]),
        ),
        (
            "wrong selected direction",
            "/operations/game.observation/selection/reads",
            json!(["game.move"]),
        ),
        (
            "duplicate selection fields",
            "/operations/game.observation/selection/args_field",
            json!("operation"),
        ),
    ] {
        let mut a = base.clone();
        *a.pointer_mut(path).unwrap() = value;
        cases.push((label, a));
    }
    let mut a = base.clone();
    let mut op = a["operations"]
        .as_object_mut()
        .unwrap()
        .remove("game.observation")
        .unwrap();
    op["input"] = json!("https://example.invalid/game/1#empty");
    a["operations"]["speech.start"] = op;
    cases.push(("built-in override", a));
    let mut a = base.clone();
    a["schemas"]["https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start"] =
        json!("empty");
    cases.push(("built-in schema override", a));
    let mut a = base.clone();
    a["rules"]["unknown"] = json!({"op":"ref","name":"absent"});
    cases.push(("unused unknown reference", a));
    let mut a = base.clone();
    a["operations"]["game.observation"]
        .as_object_mut()
        .unwrap()
        .remove("selection");
    cases.push(("observation missing selection", a));
    for (label, a) in cases {
        let raw = serde_json::to_vec(&a).unwrap();
        assert!(
            TrustedExtensionCatalog::from_pinned_bytes(&raw, &digest(&raw)).is_err(),
            "accepted {label}"
        );
        let h = Host::new(&format!("invalid-catalog-{label}"), false, &raw, &[C]);
        h.startup_fails(&h.pin_args(&digest(&raw)));
    }
}
