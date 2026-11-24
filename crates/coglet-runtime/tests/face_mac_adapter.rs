//! Compiled Swift Face adapter -> production runtime session -> durable WAL ->
//! offline replay. Media is injected by the simulator, never claimed acoustic.
#![cfg(unix)]
use coglet_runtime::{
    Engine, EngineReport, Runtime, RuntimeService, StepHost, SurfaceBinding, SurfaceSession,
};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex};

const S: &str = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ITEM: &str = "11111111111111111111111111111111";
struct NoGuest;
impl Engine for NoGuest {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
        panic!("offline replay must not execute the guest")
    }
}
struct Face {
    child: Child,
    input: Option<ChildStdin>,
    output: std::sync::mpsc::Receiver<String>,
}
impl Face {
    fn spawn() -> Self {
        let mut child =
            Command::new(std::env::var("FACE_MAC_EXECUTABLE").expect("compiled face-mac path"))
                .arg("--simulator")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .unwrap();
        let input = child.stdin.take();
        let reader = BufReader::new(child.stdout.take().unwrap());
        let (sender, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in reader.lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            input,
            output,
        }
    }
    fn command(&mut self, command: Value) -> Vec<Vec<u8>> {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{command}").unwrap();
        input.flush().unwrap();
        let line = self
            .output
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("Face executable exited or exceeded command watchdog");
        let response: Value = serde_json::from_str(&line).unwrap();
        serde_json::from_value(response["frames"].clone()).unwrap()
    }
}
impl Drop for Face {
    fn drop(&mut self) {
        drop(self.input.take());
        // Test runner/CI watchdog bounds abnormal child shutdown.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}
fn exchange(
    face: &mut Face,
    session: &mut SurfaceSession,
    command: Value,
    now: u64,
    seen: &mut Vec<Value>,
) {
    for raw in face.command(command) {
        let message = decode(&raw);
        assert_eq!(session.send(&raw, now), 0, "runtime rejected {message}");
        seen.push(message);
    }
    for _ in 0..16 {
        let Some(raw) = session.pop() else { return };
        for reply in face.command(json!({"wire": raw})) {
            let message = decode(&reply);
            assert_eq!(session.send(&reply, now), 0, "runtime rejected {message}");
            seen.push(message);
        }
    }
    panic!("control exchange did not quiesce");
}
#[test]
#[ignore = "requires compiled Swift face-mac; run with FACE_MAC_EXECUTABLE and a case watchdog"]
fn compiled_face_barge_in_journals_cancelled_action_and_replays_offline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let cases: Value =
        serde_json::from_slice(&std::fs::read(root.join("conformance/abi/cases.json")).unwrap())
            .unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["manifest"]["id"] = json!(C);
    setup["manifest"]["subscriptions"] = json!([
        "speech.start",
        "speech.end",
        "speech.position",
        "speech.barge_in"
    ]);
    setup["initial_actions"] = json!([{"coglet": C, "id": ITEM, "capability":"speech.speak_chunk",
        "args":{"stream":"22222222222222222222222222222222", "chunk":0, "text":"Hello world", "final":true},
        "deadline":30_000_000, "correlation_id":ITEM}]);
    setup["initial_effect_ids"]["actions"] = json!([ITEM]);
    let directory = std::env::temp_dir().join(format!("face-mac-wal-{}", std::process::id()));
    std::fs::create_dir_all(&directory).unwrap();
    let wal = directory.join("state.wal");
    let service = Arc::new(Mutex::new(RuntimeService::open(&wal, &setup, 128).unwrap()));
    let mut session = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    let mut face = Face::spawn();
    let mut seen = Vec::new();
    exchange(&mut face, &mut session, json!({}), 0, &mut seen);
    exchange(
        &mut face,
        &mut session,
        json!({"activate":true, "synthesis":{"duration_ms":3000,
        "spans":[{"end_ms":1200,"text":"Hello"},{"end_ms":3000,"text":" world"}]}}),
        0,
        &mut seen,
    );
    session.pump_dispatch(0).unwrap();
    exchange(&mut face, &mut session, json!({}), 0, &mut seen);
    exchange(
        &mut face,
        &mut session,
        json!({"now":100_000,"render":28_800,"start":true}),
        100_000,
        &mut seen,
    );
    let events: Vec<&Value> = seen
        .iter()
        .filter(|m| m["type"] == "event")
        .map(|m| &m["body"])
        .collect();
    let barge = events
        .iter()
        .find(|e| e["kind"] == "speech.barge_in")
        .unwrap();
    assert_eq!(barge["payload"]["played_ms"], 1200);
    assert_eq!(barge["payload"]["item"], ITEM);
    let outcome = events
        .iter()
        .find(|e| e["kind"] == "action.outcome")
        .unwrap();
    assert_eq!(outcome["payload"]["id"], ITEM);
    assert_eq!(outcome["payload"]["status"], "cancelled");
    assert!(!session.is_closed());
    drop(face);
    drop(session);
    let mut service = Arc::try_unwrap(service).ok().unwrap().into_inner().unwrap();
    service
        .seal_boundary(&setup["inputs"]["trace_id"], 100_001, &[])
        .unwrap();
    let records = service.records().to_vec();
    assert!(!records.is_empty());
    // Prove replay contains the received cancellation, not only a vacuous seal.
    // Native barge observations above are received by the live runtime; this
    // no-guest boundary test does not claim they were consumed by a guest step.
    let inbox = records
        .iter()
        .flat_map(|r| r["boundary"].as_array().into_iter().flatten())
        .filter(|change| change["method"] == "runtime.receipt")
        .filter_map(|change| change["after"].as_str())
        .next_back()
        .expect("journaled terminal inbox");
    let inbox = String::from_utf8(hex::decode(inbox).unwrap()).unwrap();
    let messages: Vec<Value> = inbox
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(
        messages
            .iter()
            .any(|m| m["event"]["kind"] == "action.outcome"
                && m["event"]["payload"]["id"] == ITEM
                && m["event"]["payload"]["status"] == "cancelled"),
        "cancelled action absent from archived inbox: {messages:#?}"
    );
    drop(service);
    let reopened = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(reopened.records(), records.as_slice());
    drop(reopened);
    let mut replay = Runtime::from_fixture(&setup).unwrap();
    let hashes = replay.replay(&records, &mut NoGuest).unwrap();
    let expected: Vec<String> = records
        .iter()
        .map(|r| {
            r.get("output_hash")
                .or_else(|| r.get("state_hash"))
                .unwrap()
                .as_str()
                .unwrap()
                .into()
        })
        .collect();
    assert_eq!(hashes, expected);
    std::fs::remove_dir_all(directory).unwrap();
}
