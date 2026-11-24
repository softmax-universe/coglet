//! ADR-0014 Linux integration. Real compiled guest, runtime native scheduler,
//! durable local Rust mock surface, while its harness device holds an encrypted
//! enrollment/health session to the real kernel. Surface frames stay local;
//! this is not a hub-relayed surface or deployment qualification.
//! The trusted constructors are intentionally NOT production cogletd options.
use coglet_hub::Kernel;
use coglet_mock::surface::MockSurface;
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
};
use coglet_runtime::{
    NativeEngine, NativeSchedule, Runtime, RuntimeService, SurfaceBinding, SurfaceSession, cbor,
    digest,
};
use coglet_surface_rust::{
    Config, GrantSpec, SpeechConfig, testing::ScriptedBackend, transport::ControlPeer,
};
use iroh::{Endpoint, endpoint::presets};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TRACE: &str = "11111111111111111111111111111111";
mod grants;
fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}
fn wasm() -> Vec<u8> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::read(root.join("target/m1-guests/wasm32-unknown-unknown/release/coglet_echo.wasm"))
        .expect("run bash tests/m1/run.sh before the M1 suite")
}
fn setup(code: &[u8]) -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../conformance/abi/cases.json")).unwrap();
    let mut s = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    // Keep the reference guest identity, subscriptions, and required operations.
    s["manifest"] = serde_json::from_str(include_str!("../../coglets/echo/manifest.json")).unwrap();
    for (key, value) in json!({"compute_mode":coglet_engine_wamr::COMPUTE_MODE,
        "compute":100_000_000,"memory_pages":256,"host_bytes":1_000_000,"wall_time_us":5_000_000})
    .as_object()
    .unwrap()
    {
        s["manifest"]["budgets"][key] = value.clone();
    }
    s["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(code));
    let raw = hex::decode(
        s["initial_files"]["/code/build/provenance.cbor"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let mut p: Value = coglet_types::decode("hub-build-record", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    p["rewritten_sha256"] = json!(digest(code));
    s["initial_files"]["/code/build/provenance.cbor"] = json!(hex::encode(cbor(&p).unwrap()));
    s
}
fn config(expires: u64) -> Config {
    let mut cfg = Config::new(S);
    cfg.bound_coglets = vec![C.into()];
    cfg.start_clock = 1000;
    cfg.grants = vec![GrantSpec {
        coglet: C.into(),
        operations: vec!["test.echo".into()],
        resources: vec![],
        expires,
    }];
    cfg
}
fn backend() -> Arc<ScriptedBackend> {
    let speech = SpeechConfig {
        stt_placement: "surface".into(),
        stt_route: "mock".into(),
        tts_placement: "surface".into(),
        tts_route: "mock".into(),
        barge: "stop".into(),
        session_mode: "text".into(),
    };
    let b = ScriptedBackend::new(speech, &["test.echo"]);
    b.auto_complete("test.echo");
    b
}
fn binding() -> SurfaceBinding {
    SurfaceBinding::new(S.into(), vec![C.into()]).unwrap()
}
struct Live {
    dir: tempfile::TempDir,
    setup: Value,
    service: Arc<Mutex<RuntimeService>>,
    session: SurfaceSession,
    surface: MockSurface,
    backend: Arc<ScriptedBackend>,
    clock: Instant,
}
impl Live {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let setup = setup(&wasm());
        let mut service =
            RuntimeService::open_echo_fixture(&dir.path().join("wal"), &setup, 128).unwrap();
        service.supervised_now().unwrap();
        service
            .enable_native(NativeSchedule {
                trace_id: TRACE.into(),
                seed: 7,
                runtime_bytes: 8_388_608,
                stack_bytes: 65_536,
            })
            .unwrap();
        let service = Arc::new(Mutex::new(service));
        let session = SurfaceSession::with_echo_fixture(binding(), service.clone()).unwrap();
        let backend = backend();
        let surface = MockSurface::open_echo_fixture(
            config(60_000_000),
            &dir.path().join("surface"),
            backend.clone(),
        )
        .unwrap();
        Self {
            dir,
            setup,
            service,
            session,
            surface,
            backend,
            clock: Instant::now(),
        }
    }
    fn now(&self) -> u64 {
        1000 + u64::try_from(self.clock.elapsed().as_micros()).unwrap()
    }
    fn exchange(&mut self, frames: Vec<Vec<u8>>) -> Vec<Value> {
        let mut incoming: VecDeque<_> = frames.into();
        let mut delivered = Vec::new();
        let mut count = 0;
        loop {
            while let Some(raw) = incoming.pop_front() {
                count += 1;
                assert!(count < 100, "unexpected protocol loop");
                let now = self.service.lock().unwrap().supervised_now().unwrap();
                assert_eq!(
                    self.session.send(&raw, now),
                    0,
                    "runtime refused {:?}",
                    decode(&raw)
                );
                self.session.pump_dispatch(now).unwrap();
            }
            let Some(raw) = self.session.pop() else { break };
            delivered.push(decode(&raw));
            let now = self.now();
            incoming.extend(self.surface.receive(&raw, now).unwrap());
        }
        delivered
    }
    fn start(&mut self) {
        let frames = self.surface.start().unwrap();
        self.exchange(frames);
    }
    fn echo(&mut self) -> Vec<Value> {
        let now = self.now();
        let frames = self
            .surface
            .emit_echo_fixture(now, json!({"message":"hello coglet","nested":{"n":42}}), C)
            .unwrap();
        assert!(
            frames
                .iter()
                .any(|f| decode(f)["body"]["kind"] == "test.echo")
        );
        self.exchange(frames)
    }
    fn export(&mut self) -> Value {
        let now = self.service.lock().unwrap().supervised_now().unwrap();
        let trace = self
            .service
            .lock()
            .unwrap()
            .export_trace(&json!(TRACE), now)
            .unwrap();
        let path = self.dir.path().join("journal.json");
        std::fs::write(&path, serde_json::to_vec(&trace).unwrap()).unwrap();
        serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
    }
}
async fn enroll_real_kernel() {
    let dir = tempfile::tempdir().unwrap();
    let hub = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let device = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let kernel = Kernel::open(dir.path(), hub.id(), Arc::new(|| Some(1))).unwrap();
    kernel.open_window(Duration::from_secs(30)).await.unwrap();
    let invite = kernel
        .invite(
            device.id(),
            Duration::from_secs(30),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: 1000,
                generation: 0,
            }],
        )
        .await
        .unwrap();
    let server = tokio::spawn({
        let h = hub.clone();
        let k = kernel.clone();
        async move {
            let conn = h.accept().await.unwrap().await.unwrap();
            Session::new(conn, h.id(), Role::Hub)
                .unwrap()
                .serve(Arc::new(k))
                .await
        }
    });
    let session = Session::new(
        device.connect(hub.addr(), ALPN).await.unwrap(),
        device.id(),
        Role::Device,
    )
    .unwrap();
    let enroll = |invite: Option<&str>| {
        json!({"method":"hub.enroll","target":null,"correlation_id":"22".repeat(16),
        "timeout_ms":1000,"params":{"offer":{"versions":["0.1"]},"invite":invite}})
    };
    assert_eq!(
        session.request(enroll(None)).await.unwrap()["type"],
        "error"
    );
    let response = session.request(enroll(Some(&invite))).await.unwrap();
    assert_eq!(response["type"], "reply", "{response}");
    assert_eq!(response["result"]["device"], device.id().to_string());
    assert_eq!(response["result"]["hub"], hub.id().to_string());
    // Retain the enrolled device session while executing its local runtime.
    // No fixture catalog crosses the hub boundary.
    let queue_depth = echo_roundtrip();
    let health = json!({"method":"hub.health","target":null,"correlation_id":"33".repeat(16),"timeout_ms":1000,
        "params":{"device":device.id().to_string(),"uptime_ms":0,"queue_depth":queue_depth,"available_bytes":0,"coglets":[]}});
    assert_eq!(
        session.request(health.clone()).await.unwrap()["type"],
        "reply"
    );
    kernel.revoke_peer(device.id()).await.unwrap();
    assert_eq!(
        session.request(health).await.unwrap()["error"]["code"],
        "permission_denied"
    );
    server.abort();
    let _ = server.await;
    device.close().await;
    hub.close().await;
}
#[tokio::test]
async fn hello_coglet() {
    tokio::time::timeout(Duration::from_secs(30), enroll_real_kernel())
        .await
        .unwrap();
}
fn echo_roundtrip() -> usize {
    let mut live = Live::new();
    live.start();
    let delivered = live.echo();
    let actions: Vec<_> = delivered.iter().filter(|m| m["type"] == "act").collect();
    assert_eq!(
        actions.len(),
        1,
        "delivered={delivered:?}; records={:?}",
        live.service
            .lock()
            .unwrap()
            .records()
            .iter()
            .map(|r| json!({"status":r["status"],"usage":r["usage"],"actions":r["actions"]}))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        actions[0]["body"]["action"]["args"],
        json!({"message":"hello coglet","nested":{"n":42}})
    );
    assert_eq!(live.backend.executed_ops(), ["test.echo"]);
    let trace = live.export();
    assert!(!trace["records"].as_array().unwrap().is_empty());
    let record = &trace["records"][0];
    assert_eq!(record["status"], "committed");
    assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(record["actions"].as_array().unwrap().len(), 1);
    drop(live.session);
    drop(live.service);
    let recovered =
        RuntimeService::open_echo_fixture(&live.dir.path().join("wal"), &live.setup, 128).unwrap();
    assert!(!recovered.records().is_empty());
    let queue_depth = recovered.queued();
    drop(recovered);
    let wal = coglet_runtime::DurableStore::open(&live.dir.path().join("wal")).unwrap();
    let dispatches = wal.state().unwrap()["dispatches"].as_object().unwrap();
    let dispatch = dispatches
        .get(actions[0]["body"]["action"]["id"].as_str().unwrap())
        .unwrap();
    assert_eq!(dispatch["outcome"]["status"], "succeeded");
    assert_eq!(dispatch["outcome"]["result"], json!({}));
    drop(wal);
    assert!(
        RuntimeService::open(&live.dir.path().join("wal"), &live.setup, 128).is_err(),
        "production recovery must reject fixture dispatches"
    );
    queue_depth
}
#[test]
fn replay_state_hash() {
    let mut live = Live::new();
    live.start();
    live.echo();
    let trace = live.export();
    let mut records = trace["records"].as_array().unwrap().clone();
    if !trace["trailing_boundary"].is_null() {
        records.push(trace["trailing_boundary"].clone());
    }
    // Tear down all live endpoints BEFORE invoking a fresh WAMR instance.
    let setup = live.setup.clone();
    drop(live);
    let engine =
        || NativeEngine::new(wasm(), &setup["manifest"]["budgets"], 8_388_608, 65_536).unwrap();
    let hashes = Runtime::from_fixture(&setup)
        .unwrap()
        .replay(&records, &mut engine())
        .unwrap();
    assert_eq!(
        hashes.last().unwrap(),
        trace["final_hash"].as_str().unwrap()
    );
    // Recompute the checksum after a mutation: checksum-only validation cannot
    // detect this lie, independent guest reexecution must reject its effects.
    let index = records
        .iter()
        .position(|r| {
            r["events"]
                .as_array()
                .is_some_and(|events| events.iter().any(|e| e["kind"] == "test.echo"))
        })
        .unwrap();
    for output in [false, true] {
        let mut mutated = records.clone();
        if output {
            mutated[index]["actions"][0]["args"]["message"] = json!("tampered");
        } else {
            mutated[index]["events"][0]["payload"]["message"] = json!("tampered");
        }
        mutated[index]
            .as_object_mut()
            .unwrap()
            .remove("record_hash");
        mutated[index]["record_hash"] = json!(digest(&cbor(&mutated[index]).unwrap()));
        let error = Runtime::from_fixture(&setup)
            .unwrap()
            .replay(&mutated, &mut engine())
            .unwrap_err();
        assert!(error.contains("replay mismatch at step"), "{error}");
    }
    eprintln!(
        "M1 independent compiled-echo replay final_hash={}",
        trace["final_hash"]
    );
}
#[test]
fn production_runtime_rejects_test_echo() {
    let dir = tempfile::tempdir().unwrap();
    let setup = setup(&wasm());
    let service = Arc::new(Mutex::new(
        RuntimeService::open(&dir.path().join("wal"), &setup, 128).unwrap(),
    ));
    assert!(SurfaceSession::with_echo_fixture(binding(), service.clone()).is_err());
    let mut session = SurfaceSession::with_service(binding(), service).unwrap();
    let b = backend();
    assert!(MockSurface::open(config(60_000_000), &dir.path().join("prod"), b.clone()).is_err());
    let mut surface =
        MockSurface::open_echo_fixture(config(60_000_000), &dir.path().join("fixture"), b).unwrap();
    let hello = surface.start().unwrap();
    assert_eq!(session.send(&hello[0], 0), 0);
    let reply = session.pop().unwrap();
    let messages = surface.receive(&reply, 1000).unwrap();
    let caps = messages
        .iter()
        .find(|m| decode(m)["type"] == "capabilities")
        .unwrap();
    assert_eq!(session.send(caps, 0), -2);
    assert!(session.is_closed());
    // Creating a fixture never activates the process-global production catalog.
    assert!(
        coglet_surface_rust::catalog::catalog()
            .unwrap()
            .op("test.echo")
            .is_none()
    );
}
#[test]
fn fixture_catalog_requires_explicit_grant() {
    grants::runtime_grant_negatives();
    for mode in ["missing", "wrong_actor", "expired", "revoked", "valid"] {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(2000);
        match mode {
            "missing" => cfg.grants.clear(),
            "wrong_actor" => cfg.grants[0].coglet = format!("coglet:{}", "c".repeat(64)),
            _ => (),
        }
        let result = MockSurface::open_echo_fixture(cfg, &dir.path().join("mock"), backend());
        if mode == "wrong_actor" {
            assert!(result.is_err());
            continue;
        }
        let mut surface = result.unwrap();
        let hello = surface.start().unwrap();
        let h = decode(&hello[0]);
        surface.receive(&cbor(&json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":h["body"]["correlation_id"]}})).unwrap(),1000).unwrap();
        if mode == "revoked" {
            surface
                .install_timeline(coglet_mock::surface::Timeline {
                    events: vec![coglet_mock::surface::TimedCommand {
                        at_us: 1001,
                        command: coglet_mock::surface::Command::Revoke { coglet: C.into() },
                    }],
                })
                .unwrap();
            surface.tick_at(1001).unwrap();
        }
        let now = if mode == "expired" { 2001 } else { 1002 };
        let frames = surface.emit_echo_fixture(now, json!({"x":1}), C).unwrap();
        assert_eq!(
            frames
                .iter()
                .any(|m| decode(m)["body"]["kind"] == "test.echo"),
            mode == "valid",
            "{mode}"
        );
    }
}
