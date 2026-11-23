use coglet_mock::surface::*;
use coglet_surface_rust::config::AdapterFixture;
use coglet_surface_rust::transport::{self, ControlPeer};
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

const FIX: &str = include_str!("../../../spec/fixtures/surface/adapter-config.json");
const ID: &str = "22222222222222222222222222222222";
const CORR: &str = "33333333333333333333333333333333";
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Dir(PathBuf);
impl Dir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "coglet-mock-tests-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Dir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn wire(j: Value) -> Vec<u8> {
    coglet_types::encode_typed("surface::surface-message", &j).unwrap()
}
fn decode(frames: Vec<Vec<u8>>) -> Vec<Value> {
    frames
        .into_iter()
        .map(|b| coglet_types::decode_typed("surface::surface-message", &b).unwrap())
        .collect()
}
fn hello_reply() -> Vec<u8> {
    wire(
        json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":"11111111111111111111111111111111"}}),
    )
}
fn action(epoch: u64) -> Vec<u8> {
    wire(
        json!({"type":"act","body":{"grant_epoch":epoch,"action":{"id":ID,"coglet":format!("coglet:{}", "b".repeat(64)),"capability":"expression.set_expression","args":{"expression":"happy","intensity":700,"duration_ms":1000},"deadline":1_000_000,"correlation_id":CORR}}}),
    )
}
fn setup(dir: &Dir, timeline: Timeline) -> (MockSurface, std::sync::Arc<ScriptedBackend>) {
    let f = AdapterFixture::parse(FIX, None).unwrap();
    let b = ScriptedBackend::new(f.speech, f.expression);
    let mut m = MockSurface::scripted(f.config, &dir.0, b.clone(), timeline).unwrap();
    let hello = decode(m.start().unwrap());
    assert_eq!(hello[0]["body"]["clock"], 0);
    m.receive(&hello_reply(), 0).unwrap();
    (m, b)
}
fn complete(at_us: u64) -> TimedCommand {
    TimedCommand {
        at_us,
        command: Command::Complete { id: ID.into() },
    }
}
#[test]
fn timeline_json_roundtrip_and_order_bounds() {
    let t = Timeline {
        events: vec![complete(20), complete(20)],
    };
    let parsed = Timeline::parse(&serde_json::to_string(&t).unwrap()).unwrap();
    assert_eq!(parsed.events.len(), 2);
    assert!(
        Timeline {
            events: vec![complete(21), complete(20)]
        }
        .validate()
        .is_err()
    );
    assert!(
        Timeline {
            events: vec![complete(MAX_TIMELINE_US + 1)]
        }
        .validate()
        .is_err()
    );
    assert!(
        Timeline {
            events: vec![complete(0); MAX_TIMELINE_EVENTS + 1]
        }
        .validate()
        .is_err()
    );
}
#[test]
fn invalid_script_data_rejected() {
    for s in [
        r#"{"events":[{"at_us":-1,"command":"kill","reason":"x"}]}"#,
        r#"{"events":[{"at_us":1,"command":"complete","id":"bad"}]}"#,
        r#"{"events":[{"at_us":1,"command":"invent_reply","reply":{}}]}"#,
        r#"{"events":[{"at_us":1,"command":"kill","reason":"x","extra":true}]}"#,
        r#"{"events":[],"extra":true}"#,
    ] {
        assert!(Timeline::parse(s).is_err(), "{s}");
    }
    assert!(Timeline::parse(&" ".repeat(MAX_CONFIG_BYTES + 1)).is_err());
}
#[test]
fn actual_admission_then_semantic_effect_and_completion() {
    let dir = Dir::new();
    let (mut m, b) = setup(
        &dir,
        Timeline {
            events: vec![complete(100)],
        },
    );
    let accepted = decode(m.receive(&action(1), 10).unwrap());
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0]["body"]["accepted"], true);
    assert_eq!(b.executed().len(), 1);
    assert_eq!(b.expression(10).0, "neutral");
    assert_eq!(m.core().pending_count(), 1);
    assert!(m.tick_at(99).unwrap().is_empty());
    let out = decode(m.tick_at(100).unwrap());
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["body"]["payload"]["status"], "succeeded");
    assert_eq!(b.expression(100).0, "happy");
    assert_eq!(m.core().pending_count(), 0);
    let retry = decode(m.receive(&action(1), 101).unwrap());
    assert_eq!(retry.len(), 2);
    assert_eq!(b.executed().len(), 1);
}
#[test]
fn revoke_cancels_effect_and_fences_captured_native_epoch() {
    let dir = Dir::new();
    let timeline = Timeline {
        events: vec![
            TimedCommand {
                at_us: 50,
                command: Command::Revoke {
                    coglet: format!("coglet:{}", "b".repeat(64)),
                },
            },
            complete(100),
            TimedCommand {
                at_us: 101,
                command: Command::Native {
                    captured_epoch: 1,
                    event: NativeEvent::SpeechStart {
                        utterance: ID.into(),
                        position_ms: 0,
                    },
                },
            },
        ],
    };
    let (mut m, b) = setup(&dir, timeline);
    m.receive(&action(1), 10).unwrap();
    let out = decode(m.tick_at(101).unwrap());
    assert!(
        out.iter()
            .any(|v| v["body"]["payload"]["status"] == "cancelled")
    );
    assert!(
        !out.iter()
            .any(|v| v["body"]["payload"]["status"] == "succeeded")
    );
    assert_eq!(b.cancelled(), [ID]);
    assert_eq!(m.discarded_native, 1);
    assert_eq!(b.expression(101).0, "neutral");
    assert_eq!(m.core().grant_epoch(), 2);
}
#[test]
fn kill_before_completion_and_deadline_at_completion_have_no_effect() {
    for kill in [true, false] {
        let dir = Dir::new();
        let mut events = Vec::new();
        if kill {
            events.push(TimedCommand {
                at_us: 50,
                command: Command::Kill {
                    reason: "operator".into(),
                },
            });
        }
        events.push(complete(1_000_000));
        let (mut m, b) = setup(&dir, Timeline { events });
        m.receive(&action(1), 10).unwrap();
        let out = decode(m.tick_at(1_000_000).unwrap());
        let status = if kill { "cancelled" } else { "expired" };
        assert!(out.iter().any(|v| v["body"]["payload"]["status"] == status));
        assert_eq!(b.expression(1_000_000).0, "neutral");
        assert!(!b.complete(ID, 1_000_001).unwrap());
    }
}
#[test]
fn install_is_atomic_and_case_only_selects_startup_epoch() {
    let mut j: Value = serde_json::from_str(FIX).unwrap();
    j["mock_timeline"] = serde_json::to_value(Timeline {
        events: vec![complete(100)],
    })
    .unwrap();
    let src = j.to_string();
    for (case, epoch) in [
        (None, 1),
        (Some("unrelated-case"), 1),
        (Some("grant-epoch-fence"), 2),
    ] {
        let dir = Dir::new();
        let mut m = MockSurface::from_fixture(&src, case, &dir.0).unwrap();
        m.start().unwrap();
        m.receive(&hello_reply(), 0).unwrap();
        assert_eq!(m.core().grant_epoch(), epoch);
        m.receive(&action(epoch), 10).unwrap();
        assert!(
            m.install_timeline(Timeline {
                events: vec![complete(9)]
            })
            .is_err()
        );
        let out = decode(m.tick_at(100).unwrap());
        assert_eq!(out[0]["body"]["payload"]["status"], "succeeded");
        assert!(m.tick_at(99).is_err());
    }
}
#[test]
fn in_process_pair_routes_real_core_and_disconnect_cancels() {
    let dir = Dir::new();
    let f = AdapterFixture::parse(FIX, None).unwrap();
    let b = ScriptedBackend::new(f.speech, f.expression);
    let m = MockSurface::scripted(f.config, &dir.0, b.clone(), Timeline::default()).unwrap();
    let (mut p, mut q) = transport::in_process_pair(m, 0).unwrap();
    assert!(q.recv().unwrap().is_some());
    q.send(&hello_reply()).unwrap();
    p.poll(0).unwrap();
    while q.recv().unwrap().is_some() {}
    q.send(&action(1)).unwrap();
    p.poll(10).unwrap();
    assert_eq!(b.executed().len(), 1);
    p.disconnect(20).unwrap();
    assert_eq!(b.cancelled(), [ID]);
    assert!(!b.complete(ID, 30).unwrap());
    assert!(q.send(&action(1)).is_err());
}
#[test]
fn captured_native_timeline_flows_through_core_normalizer() {
    let dir = Dir::new();
    let timeline = Timeline {
        events: vec![
            TimedCommand {
                at_us: 10,
                command: Command::Native {
                    captured_epoch: 1,
                    event: NativeEvent::SpeechStart {
                        utterance: ID.into(),
                        position_ms: 0,
                    },
                },
            },
            TimedCommand {
                at_us: 20,
                command: Command::Native {
                    captured_epoch: 1,
                    event: NativeEvent::SpeechEnd {
                        utterance: ID.into(),
                        revision: 1,
                        text: "hello".into(),
                        position_ms: 10,
                    },
                },
            },
        ],
    };
    let (mut m, _) = setup(&dir, timeline);
    let out = decode(m.tick_at(20).unwrap());
    assert_eq!(out.len(), 2);
    assert_eq!(out[0]["body"]["kind"], "speech.start");
    assert_eq!(out[1]["body"]["kind"], "speech.end");
    assert_eq!(out[1]["body"]["payload"]["text"], "hello");
}
#[test]
fn binary_both_transports_clock_zero_kill_and_cleanup() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    for mode in ["stdio", "in-process"] {
        let dir = Dir::new();
        let fixture = dir.0.join("config.json");
        std::fs::write(&fixture, FIX).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_coglet-mock-surface"))
            .args(["--stdio", "--transport", mode])
            .env("COGLET_FIXTURE_CONFIG", &fixture)
            .env("COGLET_FIXTURE_KILL_AT_MS", "100")
            .env("TMPDIR", &dir.0)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(Some(f)) = transport::read_frame(&mut output) {
                if tx.send(f).is_err() {
                    break;
                }
            }
        });
        let hello = decode(vec![rx.recv_timeout(Duration::from_secs(2)).unwrap()]);
        assert_eq!(hello[0]["body"]["clock"], 0);
        let start = Instant::now();
        transport::write_frame(&mut input, &hello_reply()).unwrap();
        input.flush().unwrap();
        let mut saw_kill = false;
        while !saw_kill {
            let v = decode(vec![rx.recv_timeout(Duration::from_secs(2)).unwrap()]);
            saw_kill = v[0]["type"] == "kill";
        }
        assert!(start.elapsed() < Duration::from_millis(500));
        transport::write_frame(&mut input, &action(1)).unwrap();
        input.flush().unwrap();
        loop {
            let v = decode(vec![rx.recv_timeout(Duration::from_secs(2)).unwrap()]);
            if v[0]["type"] == "accept" {
                assert_eq!(v[0]["body"]["accepted"], false);
                break;
            }
        }
        drop(input);
        let result = child.wait_with_output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let names: Vec<_> = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["config.json"]);
    }
}
