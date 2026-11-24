//! Simulator-to-runtime heard history: the Python duplex-audio simulator
//! (sim/coglet_sim/host.py, real surface wire over pipes) drives a durable
//! RuntimeService through SurfaceSession. The runtime builds `runtime.heard`
//! from its own owner reads, seals the journal, and offline replay of that
//! archive reproduces every hash without the surface. Two independent runs
//! produce byte-identical heard state.
//!
//! Needs `uv` and the sim project; CI runs it explicitly from rust.yml:
//! `cargo test -p coglet-runtime --test sim_duplex_heard -- --ignored`.
#![cfg(unix)]
use coglet_runtime::{
    Engine, EngineReport, Runtime, RuntimeService, StepHost, SurfaceBinding, SurfaceSession, cbor,
};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

// Identities from spec/fixtures/surface/adapter-config.json used by the simulator.
const S: &str = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const C: &str = "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const STREAM: &str = "2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e2e";
// (action id, correlation id, text, final). Voice script "heard-history":
// "Good morning." aligned and finished, unaligned text finished, "Hello world"
// interrupted by user speech at 1200 ms of 3000, then a queued chunk never played.
const SPEAK: [(&str, &str, &str, bool); 4] = [
    (
        "a0000000000000000000000000000001",
        "c0000000000000000000000000000001",
        "Good morning.",
        false,
    ),
    (
        "a0000000000000000000000000000002",
        "c0000000000000000000000000000002",
        "Unaligned words",
        false,
    ),
    (
        "a0000000000000000000000000000003",
        "c0000000000000000000000000000003",
        "Hello world",
        false,
    ),
    (
        "a0000000000000000000000000000004",
        "c0000000000000000000000000000004",
        "Never heard",
        true,
    ),
];

struct MustNotRun;
impl Engine for MustNotRun {
    fn execute(&mut self, _: &Value, _: &mut StepHost) -> coglet_runtime::Result<EngineReport> {
        panic!("offline heard replay invoked the guest engine");
    }
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}

fn setup() -> Value {
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
    s["manifest"]["subscriptions"] = json!(["speech.position", "speech.barge_in"]);
    s["initial_actions"] = json!(
        SPEAK
            .iter()
            .enumerate()
            .map(|(chunk, (id, correlation, text, last))| json!({
                "coglet": C, "id": id, "capability": "speech.speak_chunk",
                "args": {"stream": STREAM, "chunk": chunk, "text": text, "final": last},
                "deadline": 30_000_000, "correlation_id": correlation}))
            .collect::<Vec<_>>()
    );
    s["initial_effect_ids"]["actions"] = json!(SPEAK.iter().map(|a| a.0).collect::<Vec<_>>());
    s
}

struct Sim {
    child: Child,
    stdin: Option<ChildStdin>,
    frames: mpsc::Receiver<Vec<u8>>,
}
impl Sim {
    fn spawn() -> Self {
        let mut child = Command::new(std::env::var("COGLET_UV").unwrap_or_else(|_| "uv".into()))
            .args([
                "run",
                "--locked",
                "--quiet",
                "python",
                "-m",
                "coglet_sim.host",
            ])
            .current_dir(root().join("sim"))
            .env("COGLET_DUPLEX_CASE", "heard-history")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn duplex-audio simulator (needs uv)");
        let mut stdout = child.stdout.take().unwrap();
        let (tx, frames) = mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let mut header = [0; 4];
                if stdout.read_exact(&mut header).is_err() {
                    return;
                }
                let mut raw = vec![0; u32::from_be_bytes(header) as usize];
                if stdout.read_exact(&mut raw).is_err() || tx.send(raw).is_err() {
                    return;
                }
            }
        });
        let stdin = child.stdin.take();
        Self {
            child,
            stdin,
            frames,
        }
    }
    fn send(&mut self, raw: &[u8]) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
        stdin.write_all(raw).unwrap();
        stdin.flush().unwrap();
    }
}
impl Drop for Sim {
    fn drop(&mut self) {
        drop(self.stdin.take()); // EOF ends the simulator's serve loop
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if self.child.try_wait().unwrap().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Run {
    heard: Value,
    heard_bytes: String,
    outcomes: Vec<Value>,
    surface_items: Vec<String>,
}

fn run_once(name: &str) -> Run {
    let dir = std::env::temp_dir().join(format!("sim-heard-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let wal = dir.join("state.wal");
    let setup = setup();
    let service = Arc::new(Mutex::new(RuntimeService::open(&wal, &setup, 128).unwrap()));
    let mut session = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    let started = Instant::now();
    let clock = || 1_000 + started.elapsed().as_micros() as u64;
    let mut sim = Sim::spawn();
    let mut seen = Vec::new();
    let (mut reads, mut replies) = (0, 0);
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        assert!(
            Instant::now() < deadline,
            "simulator session timed out: {seen:#?}"
        );
        let now = clock();
        session.tick(now);
        session.pump_dispatch(now).unwrap();
        while let Some(raw) = session.pop() {
            if decode(&raw)["type"] == "read" {
                reads += 1;
            }
            sim.send(&raw);
        }
        assert!(
            !session.is_closed(),
            "runtime closed the session: {seen:#?}"
        );
        while let Ok(raw) = sim.frames.recv_timeout(Duration::from_millis(2)) {
            let message = decode(&raw);
            if message["type"] == "read_reply" {
                replies += 1;
            }
            assert_eq!(session.send(&raw, clock()), 0, "runtime refused {message}");
            seen.push(message);
        }
        let outcomes = seen
            .iter()
            .filter(|m| m["body"]["kind"] == "action.outcome")
            .count();
        if outcomes == SPEAK.len() && reads == replies && reads > 0 {
            // Allow any trailing frames, then require the exchange quiescent.
            std::thread::sleep(Duration::from_millis(50));
            if sim.frames.try_recv().is_err() && session.front().is_none() {
                break;
            }
        }
    }
    drop(sim);
    drop(session);
    let mut service = Arc::try_unwrap(service).ok().unwrap().into_inner().unwrap();
    let sealed = service
        .seal_boundary(&setup["inputs"]["trace_id"], clock() + 1, &[])
        .unwrap();
    let records = service.records().to_vec();
    assert_eq!(records.last(), Some(&sealed));
    drop(service);

    // Durable: the WAL reopens to the same journal.
    let reopened = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(reopened.records(), records.as_slice());
    drop(reopened);

    // Offline replay reads only the archive: no surface, no simulator, no guest.
    let mut replay = Runtime::from_fixture(&setup).unwrap();
    let hashes = replay.replay(&records, &mut MustNotRun).unwrap();
    let expected: Vec<String> = records
        .iter()
        .map(|r| {
            r.get("output_hash")
                .or_else(|| r.get("state_hash"))
                .and_then(Value::as_str)
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(hashes, expected);

    let changes: Vec<&Value> = records
        .iter()
        .flat_map(|r| r["boundary"].as_array().into_iter().flatten())
        .filter(|c| c["method"] == "runtime.heard")
        .collect();
    assert!(
        !changes.is_empty(),
        "no runtime.heard boundary was journaled"
    );
    assert!(
        changes
            .iter()
            .all(|c| c["path"] == "/data/runtime/heard.cbor" && c["actor"] == C)
    );
    let heard_bytes = changes.last().unwrap()["after"]
        .as_str()
        .unwrap()
        .to_owned();
    let heard: Value = coglet_types::decode("heard-state", &hex::decode(&heard_bytes).unwrap())
        .unwrap()
        .deserialized()
        .unwrap();
    let outcomes = seen
        .iter()
        .filter(|m| m["body"]["kind"] == "action.outcome")
        .map(|m| m["body"]["payload"].clone())
        .collect();
    let surface_items = seen
        .iter()
        .filter(|m| {
            matches!(
                m["body"]["kind"].as_str(),
                Some("speech.position" | "speech.barge_in")
            )
        })
        .map(|m| m["body"]["payload"]["item"].as_str().unwrap().to_owned())
        .collect();
    std::fs::remove_dir_all(&dir).unwrap();
    Run {
        heard,
        heard_bytes,
        outcomes,
        surface_items,
    }
}

#[test]
#[ignore = "spawns the Python duplex-audio simulator via uv; run by rust.yml"]
fn sim_heard_history_journals_heard_prefix_and_replays_offline() {
    let first = run_once("first");
    let items = first.heard["items"].as_array().unwrap();
    let by_item = |id: &str| {
        items
            .iter()
            .find(|v| v["item"] == id)
            .unwrap_or_else(|| panic!("no heard item {id}: {items:#?}"))
    };
    // Finished aligned item: preserved in full, independent of later items.
    let good = by_item(SPEAK[0].0);
    assert_eq!(good["surface"], S);
    assert_eq!(good["played_ms"], 1400);
    assert_eq!(good["heard"], "Good morning.");
    assert_eq!(good["stopped"], true);
    // Unknown alignment: actual playback advances but heard text stays empty.
    let unaligned = by_item(SPEAK[1].0);
    assert_eq!(unaligned["played_ms"], 750);
    assert_eq!(unaligned["heard"], "");
    assert_eq!(unaligned["spans"], json!([]));
    // Interrupted item: only the aligned prefix ending at or before 1200 ms.
    let interrupted = by_item(SPEAK[2].0);
    assert_eq!(interrupted["played_ms"], 1200);
    assert_eq!(interrupted["heard"], "Hello");
    assert_eq!(
        interrupted["spans"],
        json!([{"end_ms":1200,"text":"Hello"}])
    );
    assert_eq!(interrupted["stopped"], true);
    // Queued item after the interruption: never rendered, never heard.
    assert!(items.iter().all(|v| v["item"] != SPEAK[3].0));
    assert_eq!(items.len(), 3);
    // Wire identity: every playback item is an admitted action ID, and every
    // outcome carries that action's own correlation.
    let ids: Vec<&str> = SPEAK.iter().map(|a| a.0).collect();
    assert!(
        first
            .surface_items
            .iter()
            .all(|i| ids.contains(&i.as_str()))
    );
    let mut statuses: Vec<(String, String, String)> = first
        .outcomes
        .iter()
        .map(|o| {
            (
                o["id"].as_str().unwrap().into(),
                o["correlation_id"].as_str().unwrap().into(),
                o["status"].as_str().unwrap().into(),
            )
        })
        .collect();
    statuses.sort();
    assert_eq!(
        statuses,
        SPEAK
            .iter()
            .zip(["succeeded", "succeeded", "cancelled", "cancelled"])
            .map(|((id, correlation, _, _), status)| (
                id.to_string(),
                correlation.to_string(),
                status.to_string()
            ))
            .collect::<Vec<_>>()
    );
    // Replay equality across independent simulator + runtime executions.
    let second = run_once("second");
    assert_eq!(second.heard_bytes, first.heard_bytes);
    assert_eq!(cbor(&second.heard).unwrap(), cbor(&first.heard).unwrap());
}
