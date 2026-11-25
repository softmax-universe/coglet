//! ADR-0022 minimal embedded trusted-extension catalog opt-in through the actual C
//! pointer ABI. All game payloads are the generic synthetic fixture, not a product.
//!
//! The FFI binding/runtime/WAMR globals are process-immutable, so every named
//! scenario runs in a fresh `current_exe` subprocess (`scenario_child`). The two
//! ADR-named tests below only orchestrate those subprocesses with finite deadlines.
#![cfg(any(target_os = "linux", target_os = "macos"))]
use coglet_runtime::{DurableStore, RuntimeService, TrustedExtensionCatalog, cbor, digest, ffi::*};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const RAW: &[u8] = include_bytes!("../examples/trusted-extension-catalog.json");
const ENV: &str = "COGLET_EMBEDDED_CATALOG_SCENARIO";
const MAX_CONFIG: usize = 16 * 1024 * 1024;
const MAX_CATALOG: usize = 1024 * 1024;

// ---------------------------------------------------------------- orchestration

fn run_scenario(name: &str, timeout: Duration) {
    let out = std::env::temp_dir().join(format!("etc-{}-{name}.out", std::process::id()));
    let file = std::fs::File::create(&out).unwrap();
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "scenario_child",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(ENV, name)
        .stdout(file)
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("scenario {name} exceeded {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = std::fs::read_to_string(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    assert!(status.success(), "scenario {name} failed:\n{stdout}");
    // A filtered-out child exits 0 with zero tests: require the explicit marker.
    assert!(
        stdout.contains(&format!("scenario-ok:{name}")),
        "scenario {name} did not run:\n{stdout}"
    );
}

#[test]
fn scenario_child() {
    let Ok(name) = std::env::var(ENV) else {
        return; // ordinary suite run: only a parent-spawned child executes a scenario
    };
    match name.as_str() {
        "default_builtins_refusal" => default_builtins_refusal(),
        "pinned_negotiation_event_wal" => pinned_negotiation_event_wal(),
        "pinned_action_outcome_wal" => pinned_action_outcome_wal(),
        "size_boundaries" => size_boundaries(),
        "native_event_guest" => native_event_guest(),
        "envelope_errors" => envelope_errors(),
        "loader_errors" => loader_errors(),
        "live_link_stop_fence" => live_link_stop_fence(),
        "reopen_pin_fence" => reopen_pin_fence(),
        "profile_provisioning" => profile_provisioning(),
        "wire_profile_downgrade" => wire_profile_downgrade(),
        other => panic!("unknown scenario {other}"),
    }
    println!("scenario-ok:{name}");
}

#[test]
fn embedded_trusted_catalog_start() {
    let t = Duration::from_secs(120);
    run_scenario("default_builtins_refusal", t);
    run_scenario("pinned_negotiation_event_wal", t);
    run_scenario("pinned_action_outcome_wal", t);
    run_scenario("size_boundaries", Duration::from_secs(240));
    #[cfg(feature = "native-engine")]
    run_scenario("native_event_guest", Duration::from_secs(300));
}

#[test]
fn embedded_trusted_catalog_failure_fences() {
    let t = Duration::from_secs(120);
    run_scenario("envelope_errors", t);
    run_scenario("loader_errors", t);
    run_scenario("live_link_stop_fence", t);
    run_scenario("reopen_pin_fence", t);
}

#[test]
fn embedded_local_contract_profile_provisioning() {
    run_scenario("profile_provisioning", Duration::from_secs(120));
}

#[test]
fn embedded_wire_profile_negotiation_and_downgrade() {
    run_scenario("wire_profile_downgrade", Duration::from_secs(120));
}

/// Actual C pointer ABI: selected 0.2 product wire, then a live 0.1 downgrade of
/// the SAME provisioned 0.2 WAL. SYNTHETIC CLOSED catalog, not the app catalog.
fn wire_profile_downgrade() {
    let face = json!({"coglet":C,"id":"4".repeat(32),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":50_000_000,"correlation_id":"5".repeat(32)});
    let app = json!({"coglet":C,"id":"6".repeat(32),"capability":"softmax.app.navigate","args":{},"deadline":50_000_000,"correlation_id":"7".repeat(32)});
    let mut s = setup(false);
    s["contract_profile"] = json!("surface-0.2");
    s["manifest"]["subscriptions"] = json!(["softmax.app.view_changed"]);
    s["initial_actions"] = json!([face, app]);
    s["initial_effect_ids"]["actions"] = json!(["4".repeat(32), "6".repeat(32)]);
    let uri = "https://example.invalid/softmax/1#empty";
    let raw = serde_json::to_vec(&json!({"version":1,"namespace":"softmax",
        "entries":{"softmax.app":["softmax.app_control"]},
        "schemas":{uri:"empty"},"rules":{"empty":{"op":"record","fields":[]}},
        "operations":{
            "softmax.app.view_changed":{"entry":"softmax.app","direction":"event","input":uri,"resources":[],"single_owner":true},
            "softmax.app.navigate":{"entry":"softmax.app","direction":"action","input":uri,"output":uri,"resources":["softmax.app_control"],"single_owner":true}}}))
    .unwrap();
    let env = Env::new("wire", s);
    assert_eq!(env.start_with(Some(Env::pinned(&raw))), 0);
    let face_entry = json!({"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}});
    let app_entry = json!({"name":"softmax.app","events":["softmax.app.view_changed"],"reads":[],"actions":["softmax.app.navigate"],"exclusive":["softmax.app_control"],"schemas":{"softmax.app.view_changed":uri,"softmax.app.navigate":uri}});
    let caps = |entries: Value| json!({"revision":1,"clock":1100,"entries":entries,"correlation_id":"2".repeat(32)});
    let grants = |epoch: u64, ops: Value, resources: Value| json!({"epoch":epoch,"entries":[{"coglet":C,"operations":ops,"resources":resources,"expires":100_000_000}],"correlation_id":"3".repeat(32)});
    let hello = |versions: Value| json!({"surface":S,"offer":{"versions":versions},"clock":1000,"correlation_id":"1".repeat(32)});
    let event = json!({"source":S,"kind":"softmax.app.view_changed","seq":1,"logical_time":1200,"payload":{},"correlation_id":"9".repeat(32)});

    let mut link = Link::open();
    link.send_ok("hello", hello(json!(["0.1", "0.2"])));
    assert_eq!(
        link.recv_until("hello_reply")["body"]["selection"]["version"],
        "0.2"
    );
    link.send_ok("capabilities", caps(json!([face_entry, app_entry])));
    link.send_ok(
        "grants",
        grants(
            1,
            json!([
                "expression.set_expression",
                "softmax.app.view_changed",
                "softmax.app.navigate"
            ]),
            json!(["display.face", "softmax.app_control"]),
        ),
    );
    let first = link.recv_until("act")["body"]["action"].clone();
    let second = link.recv_until("act")["body"]["action"].clone();
    assert_eq!(first["capability"], "expression.set_expression");
    assert_eq!(second["capability"], "softmax.app.navigate");
    link.send_ok("event", event.clone());
    link.barrier();
    let before = env.snapshot("before");
    assert_eq!(private_events(&before).len(), 1);
    let retained = before.state().unwrap()["dispatches"].clone();
    link.close();

    // Same live process/WAL, peer offers only 0.1: Face retry remains, app is unavailable.
    let mut link = Link::open();
    link.send_ok("hello", hello(json!(["0.1"])));
    assert_eq!(
        link.recv_until("hello_reply")["body"]["selection"]["version"],
        "0.1"
    );
    link.send_ok("capabilities", caps(json!([face_entry])));
    link.send_ok(
        "grants",
        grants(
            2,
            json!(["expression.set_expression"]),
            json!(["display.face"]),
        ),
    );
    assert_eq!(
        link.recv_until("act")["body"]["action"],
        first,
        "retained Face retry"
    );
    link.barrier(); // an app retry would precede this barrier and fail recv_until
    let mut fresh = event.clone();
    fresh["seq"] = json!(2); // fresh seq: refusal must be the profile fence, not rollback
    fresh["logical_time"] = json!(1300);
    link.assert_refused("event", fresh);
    drop(link);
    for bad in [
        caps(
            json!([face_entry, {"name":"softmax.app","events":[],"reads":[],"actions":[],"exclusive":[],"schemas":{}}]),
        ),
        caps(json!([app_entry])),
    ] {
        let mut link = Link::open();
        link.send_ok("hello", hello(json!(["0.1"])));
        link.recv_until("hello_reply");
        link.assert_refused("capabilities", bad);
    }
    let after = env.snapshot("after");
    let state = after.state().unwrap();
    assert_eq!(state["contract_profile"], "surface-0.2");
    assert_eq!(state["catalog_pin"], digest(&raw));
    assert_eq!(
        state["dispatches"], retained,
        "downgrade never rewrites retained dispatches"
    );
    assert!(
        private_events(&after).is_empty(),
        "0.2 queued event not restored on 0.1"
    );
    assert_eq!(coglet_runtime_stop(), 0);
}

fn profile_provisioning() {
    let mut s = setup(false);
    s["contract_profile"] = json!("surface-0.2");
    s["manifest"]["subscriptions"] = json!(["softmax.app.view_changed"]);
    let env = Env::new("profile", s);
    // Synthetic profile fixture, not the production app catalog. Closed v1;
    // only trusted setup selects the naming dialect.
    let artifact = json!({"version":1,"namespace":"softmax",
        "entries":{"softmax.app":["softmax.app_control"]},
        "schemas":{"https://example.invalid/softmax/1#empty":"empty"},
        "rules":{"empty":{"op":"record","fields":[]}},
        "operations":{"softmax.app.invoke":{"entry":"softmax.app","direction":"action",
            "input":"https://example.invalid/softmax/1#empty",
            "output":"https://example.invalid/softmax/1#empty",
            "resources":["softmax.app_control"],"single_owner":true}}});
    let raw = serde_json::to_vec(&artifact).unwrap();
    assert_eq!(env.start_with(Some(Env::pinned(&raw))), 0);
    let state = env.snapshot("profile").state().unwrap().clone();
    assert_eq!(state["contract_profile"], "surface-0.2");
    assert_eq!(state["catalog_pin"], digest(&raw));
    assert_eq!(coglet_runtime_stop(), 0);
    // A genuinely profile-neutral catalog/manifest isolates the recovery pair
    // fence: rejection must not merely be the frozen loader rejecting app names.
    let mut neutral = artifact.clone();
    let operation = neutral["operations"]
        .as_object_mut()
        .unwrap()
        .remove("softmax.app.invoke")
        .unwrap();
    neutral["operations"]["softmax.invoke"] = operation;
    let neutral_raw = serde_json::to_vec(&neutral).unwrap();
    let mut pair = env.config(Some(Env::pinned(&neutral_raw)));
    pair["wal"] = json!(env.dir.join("pair.wal"));
    pair["setup"]["manifest"]["subscriptions"] = json!([]);
    assert_eq!(start_value(&pair), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    pair["setup"]["contract_profile"] = json!("surface-0.1");
    assert_ne!(start_value(&pair), 0);
    let mut fresh = pair.clone();
    fresh["wal"] = json!(env.dir.join("neutral-fresh.wal"));
    assert_eq!(
        start_value(&fresh),
        0,
        "same supplied default pair valid on fresh WAL"
    );
    assert_eq!(coglet_runtime_stop(), 0);
    pair["setup"]["contract_profile"] = json!("surface-0.2");
    assert_eq!(start_value(&pair), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    // Invalid explicit selectors refuse before any service is published.
    for profile in [json!(null), json!(""), json!("surface-0.3"), json!([])] {
        let mut invalid = env.config(None);
        invalid["setup"]["contract_profile"] = profile;
        assert_ne!(start_value(&invalid), 0);
    }
    assert_eq!(env.start_with(Some(Env::pinned(&raw))), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    // Catalog-free startup persists the selected profile as well.
    let mut bare = env.config(None);
    bare["wal"] = json!(env.dir.join("bare.wal"));
    assert_eq!(start_value(&bare), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    let saved = DurableStore::open(&env.dir.join("bare.wal")).unwrap();
    assert_eq!(saved.state().unwrap()["contract_profile"], "surface-0.2");
    assert!(saved.state().unwrap()["catalog_pin"].is_null());
}

// ---------------------------------------------------------------- fixtures

fn artifact() -> Value {
    serde_json::from_slice(RAW).unwrap()
}
fn catalog(raw: &[u8]) -> TrustedExtensionCatalog {
    TrustedExtensionCatalog::from_pinned_bytes(raw, &digest(raw)).unwrap()
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
fn game_capabilities(a: &Value) -> Value {
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
fn builtin_capabilities() -> Value {
    json!({"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)})
}
fn game_grants() -> Value {
    let ops = [
        "game.state",
        "game.tiles",
        "game.observation",
        "game.move",
        "game.helper_spawn",
        "game.highlight",
    ];
    json!({"epoch":1,"entries":[{"coglet":C,"operations":ops,"resources":["game.control","game.helper","game.overlay"],"expires":100_000_000}],"correlation_id":"3".repeat(32)})
}
fn observation(op: &str, args: Value, value: Value) -> Value {
    json!({"source":S,"kind":"game.observation","seq":1,"logical_time":1200,"payload":{"query_id":"6".repeat(32),"operation":op,"args":args,"value":value,"revision":1,"world_generation":1,"grant_epoch":1},"correlation_id":"7".repeat(32)})
}
fn decode(raw: &[u8]) -> Value {
    // 0.2 is the superset inspection rule; wire validity is asserted elsewhere.
    coglet_types::decode(
        coglet_runtime::ContractProfile::Surface02.rule("surface-message"),
        raw,
    )
    .unwrap()
    .deserialized()
    .unwrap()
}

// ---------------------------------------------------------------- C pointer helpers

struct Env {
    dir: PathBuf,
    setup: Value,
    native: Option<Value>,
}
impl Env {
    fn new(name: &str, setup: Value) -> Self {
        let dir = std::env::temp_dir().join(format!("etc-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let binding = serde_json::to_vec(&json!({"surface":S,"permitted_coglets":[C]})).unwrap();
        assert_eq!(
            unsafe { coglet_runtime_configure(binding.as_ptr(), binding.len()) },
            0
        );
        Self {
            dir,
            setup,
            native: None,
        }
    }
    fn wal(&self) -> PathBuf {
        self.dir.join("state.wal")
    }
    fn config(&self, catalog: Option<Value>) -> Value {
        let mut config = json!({"wal":self.wal(),"setup":self.setup});
        if let Some(native) = &self.native {
            config["native_schedule"] = native.clone();
        }
        if let Some(catalog) = catalog {
            config["trusted_extension_catalog"] = catalog;
        }
        config
    }
    /// Pin for exactly the supplied decoded bytes.
    fn pinned(raw: &[u8]) -> Value {
        json!({"json":String::from_utf8(raw.to_vec()).unwrap(),"sha256":digest(raw)})
    }
    fn start_with(&self, catalog: Option<Value>) -> i32 {
        start_value(&self.config(catalog))
    }
    fn start(&self) {
        assert_eq!(self.start_with(Some(Self::pinned(RAW))), 0, "pinned start");
    }
    /// Copy of the durable bytes while the live service still owns the exclusive lock.
    fn snapshot(&self, label: &str) -> DurableStore {
        let copy = self.dir.join(format!("snapshot-{label}.wal"));
        std::fs::copy(self.wal(), &copy).unwrap();
        DurableStore::open(&copy).unwrap()
    }
}
impl Drop for Env {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
fn start_bytes(raw: &[u8]) -> i32 {
    unsafe { coglet_runtime_start(raw.as_ptr(), raw.len()) }
}
fn start_value(config: &Value) -> i32 {
    start_bytes(&serde_json::to_vec(config).unwrap())
}

struct Link(*mut CogletSurfaceLink);
impl Link {
    fn open() -> Self {
        let mut link = std::ptr::null_mut();
        assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
        assert!(!link.is_null());
        Self(link)
    }
    fn send(&mut self, kind: &str, body: Value) -> i32 {
        let raw = cbor(&json!({"type":kind,"body":body})).unwrap();
        unsafe { coglet_surface_send(self.0, raw.as_ptr(), raw.len()) }
    }
    fn send_ok(&mut self, kind: &str, body: Value) {
        assert_eq!(self.send(kind, body), 0, "C ABI send {kind}");
    }
    /// Bounded recv: Ok(message), or Err(status) for closed/invalid/would-block timeout.
    fn try_recv(&mut self, timeout: Duration) -> Result<Value, i32> {
        let deadline = Instant::now() + timeout;
        loop {
            let mut n = 0;
            match unsafe { coglet_surface_recv(self.0, std::ptr::null_mut(), 0, &mut n) } {
                2 => {
                    let mut raw = vec![0; n];
                    assert_eq!(
                        unsafe { coglet_surface_recv(self.0, raw.as_mut_ptr(), raw.len(), &mut n) },
                        0
                    );
                    return Ok(decode(&raw));
                }
                1 if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(1)),
                status => return Err(status),
            }
        }
    }
    fn recv_until(&mut self, kind: &str) -> Value {
        loop {
            let m = self
                .try_recv(Duration::from_secs(20))
                .unwrap_or_else(|s| panic!("waiting for {kind}: status {s}"));
            if m["type"] == kind {
                return m;
            }
            assert_eq!(
                m["type"], "health",
                "unexpected {m} while waiting for {kind}"
            );
        }
    }
    fn hello(&mut self) {
        self.send_ok("hello", json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}));
        self.recv_until("hello_reply");
    }
    fn ready(&mut self) {
        self.ready_epoch(1);
    }
    fn ready_epoch(&mut self, epoch: u64) {
        self.hello();
        self.send_ok("capabilities", game_capabilities(&artifact()));
        let mut grants = game_grants();
        grants["epoch"] = json!(epoch);
        self.send_ok("grants", grants);
    }
    fn barrier(&mut self) -> Value {
        self.send_ok(
            "health",
            json!({"state":"ready","clock":1400,"pending":0,"correlation_id":"9".repeat(32)}),
        );
        loop {
            let h = self.recv_until("health");
            if h["body"]["correlation_id"] == "9".repeat(32) {
                return h;
            }
        }
    }
    /// A refusal is a nonzero send status, an error message, or a closed link.
    /// Silence/timeout is NOT refusal.
    fn assert_refused(&mut self, kind: &str, body: Value) {
        let status = self.send(kind, body);
        if status != 0 {
            assert!(
                matches!(status, -1 | -2),
                "unexpected refusal status {status}"
            );
            return;
        }
        loop {
            match self.try_recv(Duration::from_secs(5)) {
                Ok(m) if m["type"] == "error" => return,
                Ok(m) if m["type"] == "health" => continue,
                Ok(m) => panic!("expected refusal of {kind}, got {m}"),
                Err(-2) => return,
                Err(s) => panic!("expected explicit refusal of {kind}, recv status {s}"),
            }
        }
    }
    fn close(mut self) {
        unsafe { coglet_surface_close(self.0) };
        self.0 = std::ptr::null_mut();
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { coglet_surface_close(self.0) };
        }
    }
}

fn private_events(store: &DurableStore) -> Vec<Value> {
    store.state().unwrap()["private_events"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v["event"].clone())
        .collect()
}
fn assert_reopens_with_original_pin(env: &Env) {
    drop(RuntimeService::open_with_catalog(&env.wal(), &env.setup, 128, catalog(RAW)).unwrap());
}

// ---------------------------------------------------------------- start scenarios

fn default_builtins_refusal() {
    let env = Env::new("builtins", setup(false));
    // Ordinary absent-catalog config: unchanged RuntimeService::open.
    assert_eq!(env.start_with(None), 0);
    let mut control = Link::open();
    control.hello();
    // Positive control: built-in operations negotiate without any catalog.
    control.send_ok("capabilities", builtin_capabilities());
    control.barrier();
    control.close();
    let mut link = Link::open();
    link.hello();
    link.assert_refused("capabilities", game_capabilities(&artifact()));
    link.close();
    let store = env.snapshot("builtins");
    assert!(
        store.state().unwrap()["surface_epochs"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    assert_eq!(coglet_runtime_stop(), 0);
    assert!(private_events(&store).is_empty());
}

fn pinned_negotiation_event_wal() {
    let env = Env::new("event", setup(false));
    env.start();
    assert_eq!(coglet_runtime_status(), 0);
    let mut link = Link::open();
    link.ready();
    let e = observation("game.state", json!({}), json!({"paused":false}));
    let mut tiles = observation("game.tiles", json!({"tile":7}), json!({"count":2}));
    tiles["seq"] = json!(2);
    tiles["logical_time"] = json!(1201);
    link.send_ok("event", e.clone());
    link.send_ok("event", tiles.clone());
    link.barrier();
    // Exact admitted bytes are durable before disconnect purges private observations.
    let store = env.snapshot("event");
    let events = private_events(&store);
    assert_eq!(events.len(), 2);
    for event in [&e, &tiles] {
        assert!(
            events
                .iter()
                .any(|s| cbor(s).unwrap() == cbor(event).unwrap()),
            "exact admitted event bytes absent"
        );
    }
    assert_eq!(store.state().unwrap()["surface_sequences"][S][0], 2);
    drop(store);
    // Negative control with the same live runtime: sibling payload binding is enforced.
    link.close();
    let mut bad = Link::open();
    bad.ready_epoch(2);
    let mut sibling = observation("game.state", json!({"tile":7}), json!({"paused":false}));
    sibling["seq"] = json!(3); // fresh sequence: refusal can only be payload binding
    bad.assert_refused("event", sibling);
    bad.close();
    assert_eq!(coglet_runtime_stop(), 0);
    assert_reopens_with_original_pin(&env);
    // Reopen through the C ABI with the same embedded pin and negotiate again.
    env.start();
    let mut link = Link::open();
    link.hello();
    link.barrier();
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
}

fn pinned_action_outcome_wal() {
    let env = Env::new("action", setup(true));
    env.start();
    let mut link = Link::open();
    link.ready();
    let act = link.recv_until("act")["body"]["action"].clone();
    assert_eq!(act["capability"], "game.move");
    link.send_ok(
        "accept",
        json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
    );
    assert_eq!(link.barrier()["body"]["pending"], 1);
    let outcome = json!({"id":act["id"],"correlation_id":act["correlation_id"],"status":"succeeded","result":{}});
    link.send_ok(
        "event",
        json!({"source":S,"kind":"action.outcome","seq":1,"logical_time":1500,"payload":outcome,"correlation_id":act["correlation_id"]}),
    );
    assert_eq!(link.barrier()["body"]["pending"], 0);
    let store = env.snapshot("action");
    let d = &store.state().unwrap()["dispatches"]["4".repeat(32)];
    assert_eq!(d["outcome"], outcome);
    assert_eq!(d["intent"]["capability"], "game.move");
    drop(store);
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
    assert_reopens_with_original_pin(&env);
}

fn padded(mut raw: Vec<u8>, to: usize) -> Vec<u8> {
    assert!(raw.len() <= to);
    raw.resize(to, b' ');
    raw
}
fn size_boundaries() {
    let env = Env::new("sizes", setup(false));
    // Decoded catalog: exactly 1 MiB is accepted, one more byte is a loader error.
    let at_limit = padded(RAW.to_vec(), MAX_CATALOG);
    assert_eq!(at_limit.len(), MAX_CATALOG);
    assert_eq!(env.start_with(Some(Env::pinned(&at_limit))), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    let _ = std::fs::remove_file(env.wal());
    let over = padded(RAW.to_vec(), MAX_CATALOG + 1);
    assert_eq!(env.start_with(Some(Env::pinned(&over))), -2);
    assert_eq!(
        coglet_runtime_status(),
        1,
        "oversize catalog exposed a service"
    );
    let _ = std::fs::remove_file(env.wal());
    // Outer config: exactly 16 MiB of valid JSON (trailing whitespace) vs one byte more.
    let config = serde_json::to_vec(&env.config(Some(Env::pinned(RAW)))).unwrap();
    assert_eq!(start_bytes(&padded(config.clone(), MAX_CONFIG)), 0);
    assert_eq!(coglet_runtime_stop(), 0);
    let _ = std::fs::remove_file(env.wal());
    assert_eq!(start_bytes(&padded(config, MAX_CONFIG + 1)), -1);
    assert_eq!(coglet_runtime_status(), 1);
    assert!(
        !env.wal().exists(),
        "rejected oversize config touched the WAL"
    );
    // A decoded-byte pin: pinning the escaped JSON-string text instead is a mismatch.
    let escaped = serde_json::to_string(std::str::from_utf8(RAW).unwrap()).unwrap();
    let wrong =
        json!({"json":std::str::from_utf8(RAW).unwrap(),"sha256":digest(escaped.as_bytes())});
    assert_eq!(env.start_with(Some(wrong)), -2);
}

#[cfg(feature = "native-engine")]
fn native_event_guest() {
    fn escaped(raw: &[u8]) -> String {
        raw.iter().map(|b| format!("\\{b:02x}")).collect()
    }
    // Event-driven guest: no initial_actions, no initial timer. The only wake is the event.
    let action = json!({"coglet":C,"id":"4".repeat(32),"capability":"game.move","args":{"tile":7},"deadline":50_000_000,"correlation_id":"5".repeat(32)});
    let raw_action = cbor(&action).unwrap();
    let wasm = wat::parse_str(format!(
        "(module (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))
        (memory (export \"memory\") 1 2) (data (i32.const 0) \"{}\")
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32)
          i32.const 0 i32.const {} call $emit drop i32.const 0))",
        escaped(&raw_action),
        raw_action.len()
    ))
    .unwrap();
    let mut s = setup(false);
    s["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&wasm));
    let prov_raw = hex::decode(
        s["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut provenance: Value = coglet_types::decode("hub-build-record", &prov_raw)
        .unwrap()
        .deserialized()
        .unwrap();
    provenance["rewritten_sha256"] = json!(digest(&wasm));
    s["initial_files"]["/code/build/provenance.cbor"] =
        json!(hex::encode(cbor(&provenance).unwrap()));
    let budgets = &mut s["manifest"]["budgets"];
    budgets["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
    budgets["compute"] = json!(100_000);
    budgets["memory_pages"] = json!(2);
    budgets["wall_time_us"] = json!(500_000);
    budgets["host_bytes"] = json!(1_000_000);
    // Extension permissions replace the helper's expression permissions.
    s["manifest"]["subscriptions"] = json!(["game.observation"]);
    s["manifest"]["required_capabilities"] = json!(["game.move"]);
    s["manifest"]["exclusive_outputs"] = json!(["game.control"]);
    s["initial_actions"] = json!([]);
    s["initial_effect_ids"]["actions"] = json!([]);
    s["initial_timers"] = json!([]);
    s["initial_effect_ids"]["timers"] = json!([]);
    let mut env = Env::new("native", s);
    env.native = Some(
        json!({"trace_id":env.setup["inputs"]["trace_id"],"seed":7,"runtime_bytes":8_388_608,"stack_bytes":65_536}),
    );
    env.start();
    let mut link = Link::open();
    link.ready();
    // Grants alone must not run the guest: nothing is due and no initial action exists.
    link.barrier();
    assert!(
        env.snapshot("before").records().is_empty(),
        "guest ran before any event"
    );
    let e = observation("game.state", json!({}), json!({"paused":false}));
    link.send_ok("event", e.clone());
    // The real guest emits the action only because the committed event woke it.
    let act = link.recv_until("act")["body"]["action"].clone();
    assert_eq!(act["capability"], "game.move");
    assert_eq!(act["id"], "4".repeat(32));
    link.send_ok(
        "accept",
        json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
    );
    link.barrier();
    let live = env.snapshot("after");
    assert_eq!(live.records().len(), 1);
    drop(live);
    // Link live: C ownership keeps the runtime; close first, then stop.
    assert_eq!(coglet_runtime_stop(), 1);
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
    let recovered =
        RuntimeService::open_with_catalog(&env.wal(), &env.setup, 128, catalog(RAW)).unwrap();
    assert_eq!(recovered.records().len(), 1);
    let record = &recovered.records()[0];
    assert_eq!(record["status"], "committed");
    assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(record["actions"].as_array().unwrap().len(), 1);
    assert_eq!(record["actions"][0]["capability"], "game.move");
    // The committed journal input is the admitted extension event, not a timer.
    let events = record["events"].as_array().unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["kind"], "game.observation");
    assert_eq!(cbor(&events[0]).unwrap(), cbor(&e).unwrap());
    assert_eq!(recovered.queued(), 0);
}
#[cfg(not(feature = "native-engine"))]
fn native_event_guest() {
    panic!("native-engine feature required");
}

// ---------------------------------------------------------------- failure scenarios

fn assert_no_service_after_refusal() {
    assert_eq!(coglet_runtime_status(), 1, "failed start exposed a service");
    // Negotiation-only open stays allowed without a started runtime, but has no
    // extension authority and no dispatch.
    let mut link = Link::open();
    link.hello();
    link.assert_refused("capabilities", game_capabilities(&artifact()));
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
}

fn envelope_errors() {
    let env = Env::new("envelope", setup(false));
    let good = Env::pinned(RAW);
    let wal_free = |env: &Env| assert!(!env.wal().exists(), "-1 start touched the WAL");
    let mut cases: Vec<(&str, Value)> = vec![
        ("missing-json", json!({"sha256":digest(RAW)})),
        ("missing-sha", json!({"json":good["json"]})),
        ("unknown-field", {
            let mut v = good.clone();
            v["extra"] = json!(true);
            v
        }),
        ("empty-object", json!({})),
        (
            "non-string-json",
            json!({"json":{"version":1},"sha256":digest(RAW)}),
        ),
        ("non-string-sha", json!({"json":good["json"],"sha256":7})),
        ("string", json!("catalog")),
        ("array-not-record", json!([good["json"], good["sha256"]])),
    ];
    cases.push((
        "nested-unknown",
        json!({"json":good["json"],"sha256":digest(RAW),"path":"/x"}),
    ));
    for (label, catalog) in cases {
        assert_eq!(env.start_with(Some(catalog)), -1, "{label}");
        wal_free(&env);
        assert_no_service_after_refusal();
    }
    // Unknown top-level field remains refused even with a valid envelope.
    let mut config = env.config(Some(good.clone()));
    config["catalog"] = json!(1);
    assert_eq!(start_value(&config), -1);
    assert_no_service_after_refusal();
    // Positive control: the same envelope shape with only valid fields starts.
    assert_eq!(env.start_with(Some(good)), 0);
    assert_eq!(coglet_runtime_status(), 0);
    assert_eq!(coglet_runtime_stop(), 0);
}

fn loader_errors() {
    let env = Env::new("loader", setup(false));
    let sha = digest(RAW);
    let json_text = String::from_utf8(RAW.to_vec()).unwrap();
    let mut cases: Vec<(String, Value)> = vec![
        ("empty-pin".into(), json!({"json":json_text,"sha256":""})),
        ("short-pin".into(), json!({"json":json_text,"sha256":"bad"})),
        (
            "zero-pin".into(),
            json!({"json":json_text,"sha256":"0".repeat(64)}),
        ),
        (
            "uppercase-pin".into(),
            json!({"json":json_text,"sha256":sha.to_uppercase()}),
        ),
        (
            "long-pin".into(),
            json!({"json":json_text,"sha256":format!("{sha}0")}),
        ),
        (
            "changed-bytes-old-pin".into(),
            json!({"json":json_text.replace('1', "2"),"sha256":sha}),
        ),
    ];
    let mutate = |label: &str, f: &dyn Fn(&mut Value)| -> (String, Value) {
        let mut a = artifact();
        f(&mut a);
        let raw = serde_json::to_vec(&a).unwrap();
        (label.into(), Env::pinned(&raw))
    };
    cases.push(mutate("unknown-artifact-field", &|a| a["extra"] = json!(1)));
    cases.push(mutate("bad-version", &|a| a["version"] = json!(2)));
    cases.push(mutate("missing-schema-uri", &|a| {
        a["operations"]["game.state"]["output"] = json!("https://example.invalid/game/1#absent")
    }));
    cases.push(mutate("undeclared-resource", &|a| {
        a["operations"]["game.move"]["resources"] = json!(["game.undeclared"])
    }));
    cases.push(mutate("not-single-owner", &|a| {
        a["operations"]["game.move"]["single_owner"] = json!(false)
    }));
    cases.push(mutate("builtin-namespace", &|a| {
        a["namespace"] = json!("expression")
    }));
    cases.push(mutate("wrong-direction-selection", &|a| {
        a["operations"]["game.observation"]["selection"]["reads"] = json!(["game.move"])
    }));
    for (label, catalog) in &cases {
        assert_eq!(env.start_with(Some(catalog.clone())), -2, "{label}");
        assert_no_service_after_refusal();
    }
    // Positive control: unmutated bytes and pin start after all those failures.
    assert_eq!(env.start_with(Some(Env::pinned(RAW))), 0);
    assert_eq!(coglet_runtime_stop(), 0);
}

fn live_link_stop_fence() {
    let env = Env::new("live", setup(false));
    env.start();
    let mut link = Link::open();
    link.ready();
    link.barrier();
    // A live link owns the global runtime: stop must refuse, and not disturb it.
    assert_eq!(coglet_runtime_stop(), 1);
    assert_eq!(coglet_runtime_status(), 0);
    link.send_ok(
        "event",
        observation("game.state", json!({}), json!({"paused":false})),
    );
    link.barrier();
    // A second start cannot replace the live pinned runtime.
    assert_eq!(env.start_with(Some(Env::pinned(RAW))), -1);
    assert_eq!(coglet_runtime_stop(), 1);
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
    assert_eq!(coglet_runtime_status(), 1);
    assert_reopens_with_original_pin(&env);
}

fn file_digest(path: &Path) -> String {
    digest(&std::fs::read(path).unwrap())
}
fn reopen_pin_fence() {
    let env = Env::new("reopen", setup(true));
    env.start();
    let mut link = Link::open();
    link.ready();
    let act = link.recv_until("act")["body"]["action"].clone();
    link.send_ok(
        "accept",
        json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
    );
    link.barrier();
    // Terminal kill receipt retained; nothing is rerun or reset to hide uncertainty.
    link.send_ok(
        "kill",
        json!({"reason":"operator","correlation_id":"8".repeat(32)}),
    );
    loop {
        let m = link.recv_until("health");
        if m["body"]["state"] == "stopped" {
            break;
        }
    }
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
    let before = file_digest(&env.wal());
    let intent = {
        let store = DurableStore::open(&env.wal()).unwrap();
        store.state().unwrap()["dispatches"]["4".repeat(32)].clone()
    };
    assert_eq!(intent["intent"]["capability"], "game.move");

    let mut changed = RAW.to_vec();
    changed.push(b'\n'); // different bytes, valid digest/schema, different pin
    for (label, catalog) in [
        ("omitted", None),
        ("changed-valid-pin", Some(Env::pinned(&changed))),
        (
            "wrong-pin",
            Some(json!({"json":Env::pinned(RAW)["json"],"sha256":"0".repeat(64)})),
        ),
    ] {
        assert_eq!(env.start_with(catalog), -2, "{label}");
        assert_eq!(coglet_runtime_status(), 1, "{label} exposed a service");
        assert_eq!(file_digest(&env.wal()), before, "{label} changed receipts");
    }
    // The original pin still reopens, with the same dispatch receipt and no new act.
    env.start();
    let mut link = Link::open();
    link.hello();
    // Reconciliation retries the same already-committed action bytes, never a new effect.
    let retry = link.recv_until("act")["body"]["action"].clone();
    assert_eq!(retry, act, "reopen published a different/new action");
    link.barrier();
    link.close();
    assert_eq!(coglet_runtime_stop(), 0);
    let store = DurableStore::open(&env.wal()).unwrap();
    assert_eq!(
        store.state().unwrap()["dispatches"]["4".repeat(32)]["intent"],
        intent["intent"]
    );
    drop(store);
    assert_reopens_with_original_pin(&env);
}
