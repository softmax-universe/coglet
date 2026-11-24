//! Recovery through real runtime, adapter and router persistence seams.
#[path = "support/faults.rs"]
mod faults;
use coglet_runtime::{
    DurableStore, Engine, EngineReport, MockEngine, Runtime, RuntimeService, StepHost, cbor,
};
use faults::{Fault, Link, MAX_ATTEMPTS};
use serde_json::{Value, json};
use std::{path::Path, process::Command, sync::Arc, time::Duration};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "action")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["guest_script"]["writes"] = json!({"/data/count":"3939"});
    setup
}
fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}

#[test]
fn fixed_seed_schedule_reorders_delays_and_bounds_retries() {
    fn run() -> Vec<u32> {
        let mut link = Link::new();
        link.send(0, 1, Fault::Delay(20));
        link.send(0, 2, Fault::Delay(10));
        link.send(0, 3, Fault::Pass);
        assert_eq!(link.attempts, MAX_ATTEMPTS);
        let mut out = link.drain(0);
        assert_eq!(out, [3]);
        assert!(link.drain(9).is_empty());
        out.extend(link.drain(10));
        out.extend(link.drain(20));
        out
    }
    assert_eq!(run(), [3, 2, 1]);
    assert_eq!(run(), run());
    let mut link = Link::new();
    for now in [0, 10, 30] {
        assert!(!link.send(now, 1, Fault::Drop));
    }
    assert_eq!(link.attempts, MAX_ATTEMPTS);
    assert!(link.drain(100).is_empty());
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| link.send(
            100,
            1,
            Fault::Pass
        )))
        .is_err()
    );
}

fn backend() -> Arc<coglet_surface_rust::testing::ScriptedBackend> {
    use coglet_surface_rust::{SpeechConfig, testing::ScriptedBackend};
    ScriptedBackend::new(
        SpeechConfig {
            stt_placement: "surface".into(),
            stt_route: "mock".into(),
            tts_placement: "surface".into(),
            tts_route: "mock".into(),
            barge: "stop".into(),
            session_mode: "text".into(),
        },
        &["test.echo"],
    )
}
fn adapter(
    path: &Path,
    backend: Arc<coglet_surface_rust::testing::ScriptedBackend>,
) -> coglet_surface_rust::Surface {
    use coglet_surface_rust::{Config, GrantSpec};
    let mut cfg = Config::new(S);
    cfg.bound_coglets = vec![C.into()];
    cfg.grants = vec![GrantSpec {
        coglet: C.into(),
        operations: vec!["test.echo".into()],
        resources: vec![],
        expires: 60_000_000,
    }];
    coglet_surface_rust::Surface::open_echo_fixture(cfg, path, backend).unwrap()
}
fn connect(s: &mut coglet_surface_rust::Surface) {
    use coglet_surface_rust::ControlPeer;
    let hello = decode(&s.start().unwrap()[0]);
    s.receive(&cbor(&json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hello["body"]["correlation_id"]}})).unwrap(), 0).unwrap();
}
fn act(epoch: u64) -> Vec<u8> {
    cbor(&json!({"type":"act","body":{"action":{"coglet":C,"id":"4".repeat(32),"capability":"test.echo","args":{"message":"fault"},"deadline":50_000_000,"correlation_id":"5".repeat(32)},"grant_epoch":epoch}})).unwrap()
}
#[test]
fn adapter_lost_terminal_reply_restart_replays_receipt_not_effect() {
    use coglet_surface_rust::ControlPeer;
    let dir = tempfile::tempdir().unwrap();
    let b = backend();
    b.auto_complete("test.echo");
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    let request = act(s.grant_epoch());
    let replies = s.receive(&request, 10).unwrap();
    assert_eq!(b.executed().len(), 1);
    let terminal = replies
        .iter()
        .map(|r| decode(r))
        .find(|r| r["body"]["kind"] == "action.outcome")
        .unwrap();
    let mut link = Link::new();
    assert!(!link.send(10, terminal.clone(), Fault::Drop));
    assert!(link.drain(100).is_empty());
    s.disconnect(20).unwrap();
    drop(s);
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    // Old grant epoch is deliberately unchanged: receipt lookup precedes new
    // authority checks for identical already-admitted action identities.
    let replay = s.receive(&request, 30).unwrap();
    // Receipt payload is immutable; reconnect allocates a fresh event envelope
    // (source seq/clock are not part of the physical-effect receipt identity).
    let replay = replay
        .iter()
        .map(|r| decode(r))
        .find(|r| r["body"]["kind"] == "action.outcome")
        .unwrap();
    assert_eq!(replay["body"]["payload"], terminal["body"]["payload"]);
    assert_eq!(replay["body"]["source"], terminal["body"]["source"]);
    assert_eq!(b.executed().len(), 1);
}
#[test]
fn adapter_delayed_action_after_disconnect_cannot_execute() {
    use coglet_surface_rust::ControlPeer;
    let dir = tempfile::tempdir().unwrap();
    let b = backend();
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    let mut link = Link::new();
    link.send(0, act(s.grant_epoch()), Fault::Delay(20));
    link.send(1, vec![], Fault::Disconnect);
    s.disconnect(1).unwrap();
    link.reconnect();
    assert!(link.drain(20).is_empty());
    assert!(b.executed().is_empty());
    drop(s);
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    let replies = s.receive(&act(s.grant_epoch()), 30).unwrap();
    assert!(
        replies
            .iter()
            .any(|r| decode(r)["body"]["accepted"] == true)
    );
    assert_eq!(b.executed().len(), 1);
}

#[test]
fn adapter_crash_with_pending_effect_recovers_unknown_without_reexecution() {
    use coglet_surface_rust::ControlPeer;
    let dir = tempfile::tempdir().unwrap();
    let b = backend();
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    let request = act(s.grant_epoch());
    let accepted = s.receive(&request, 10).unwrap();
    assert!(
        accepted
            .iter()
            .any(|r| decode(r)["body"]["accepted"] == true)
    );
    assert_eq!(b.executed().len(), 1);
    // No disconnect callback: model process loss at the durable SDK ledger seam.
    // Runtime process death is separately exercised by an actual child exit.
    drop(s);
    let mut s = adapter(dir.path(), b.clone());
    connect(&mut s);
    let replies = s.receive(&request, 30).unwrap();
    let terminal = replies
        .iter()
        .map(|r| decode(r))
        .find(|r| r["body"]["kind"] == "action.outcome")
        .unwrap();
    assert_eq!(terminal["body"]["payload"]["status"], "failed");
    assert_eq!(terminal["body"]["payload"]["error"]["code"], "unavailable");
    assert_eq!(b.executed().len(), 1);
}

struct Crash;
impl Engine for Crash {
    fn execute(&mut self, _: &Value, host: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
        host.guest_write("/data/crash", b"not committed")?;
        std::process::exit(73)
    }
}
struct MustNotExecute;
impl Engine for MustNotExecute {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
        panic!("uncertain attempt reran")
    }
}
#[test]
fn crash_mid_step_child() {
    let Some(path) = std::env::var_os("COGLET_FAULT_CRASH_WAL") else {
        return;
    };
    let send = setup();
    let mut service = RuntimeService::open(Path::new(&path), &send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    service.run_next(&send["inputs"], &mut Crash).unwrap();
    panic!("crash hook not reached");
}
#[test]
fn runtime_process_crash_fences_attempt_and_reconciles_original_observation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    let status = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "crash_mid_step_child", "--nocapture"])
        .env("COGLET_FAULT_CRASH_WAL", &path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(73));
    let send = setup();
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    for _ in 0..MAX_ATTEMPTS {
        assert!(
            service
                .run_next(&send["inputs"], &mut MustNotExecute)
                .is_err()
        );
    }
    assert_eq!(service.queued(), 1);
    assert!(service.records().is_empty());
    drop(service);
    let store = DurableStore::open(&path).unwrap();
    assert!(
        store.state().unwrap()["runtime"]["files"]
            .get("/data/crash")
            .is_none()
    );
    drop(store);
    // Fixture oracle supplies the original observation for reconciliation. This
    // does NOT claim a crashed guest finished, or silently authorize reexecution.
    let observation = Runtime::from_fixture(&send)
        .unwrap()
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_ne!(
        observation["journal"]["input_hash"],
        observation["journal"]["output_hash"]
    );
    assert_eq!(
        observation["journal"]["actions"].as_array().unwrap().len(),
        1
    );
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    service.recover_attempt(&observation["journal"]).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    assert_eq!(service.records(), &[observation["journal"].clone()]);
    assert!(
        service
            .run_next(&send["inputs"], &mut MustNotExecute)
            .unwrap()
            .is_none()
    );
    let hashes = Runtime::from_fixture(&send)
        .unwrap()
        .replay(
            service.records(),
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(
        hashes.last().unwrap(),
        observation["journal"]["output_hash"].as_str().unwrap()
    );
}

#[test]
fn runtime_torn_commit_recovers_last_complete_state_hash_at_header_payload_and_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("original.wal");
    let send = setup();
    let mut service = RuntimeService::open(&path, &send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    // This step appends attempt + result frames. Find the final result frame,
    // then tear only that frame: every partial result must retain the fence.
    let result = service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    drop(service);
    let raw = std::fs::read(&path).unwrap();
    let mut pos = 0;
    let mut last = 0;
    while pos < raw.len() {
        last = pos;
        pos += 4 + u32::from_be_bytes(raw[pos..pos + 4].try_into().unwrap()) as usize;
    }
    let copy = dir.path().join("torn.wal");
    for cut in [
        last,
        last + 1,
        last + 2,
        last + 3,
        last + 4,
        last + 5,
        last + (raw.len() - last) / 2,
        raw.len() - 1,
    ] {
        std::fs::write(&copy, &raw[..cut]).unwrap();
        let mut recovered = RuntimeService::open(&copy, &send, 4).unwrap();
        assert!(recovered.records().is_empty(), "partial commit at {cut}");
        assert!(
            recovered
                .run_next(&send["inputs"], &mut MustNotExecute)
                .is_err()
        );
        drop(recovered);
        let store = DurableStore::open(&copy).unwrap();
        assert_eq!(
            store.state().unwrap()["runtime"]["files"],
            send["initial_files"]
        );
        assert_eq!(
            store.state().unwrap()["runtime"]["pending_actions"],
            json!({})
        );
        assert_eq!(store.state().unwrap()["dispatches"], json!({}));
        drop(store);
        assert_eq!(std::fs::metadata(&copy).unwrap().len(), last as u64);
    }
    std::fs::write(&copy, &raw).unwrap();
    let recovered = RuntimeService::open(&copy, &send, 4).unwrap();
    assert_eq!(recovered.records(), &[result["journal"].clone()]);
    let hashes = Runtime::from_fixture(&send)
        .unwrap()
        .replay(
            recovered.records(),
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(
        hashes.last().unwrap(),
        result["journal"]["output_hash"].as_str().unwrap()
    );
}

#[cfg(target_os = "linux")]
#[path = "support/disk_full_fault.rs"]
mod disk_full_fault;
#[path = "support/router_faults.rs"]
mod router_faults;
