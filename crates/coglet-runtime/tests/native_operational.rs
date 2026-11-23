//! Actual pointer boundary and actual cogletd socket: no SurfaceSession test shim.
#![cfg(target_os = "linux")]
use coglet_runtime::{cbor, ffi::*};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    process::{Child, Command},
    sync::Mutex,
    time::Duration,
};
const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
static FFI_LOCK: Mutex<()> = Mutex::new(());
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
    s["manifest"]["subscriptions"] = json!(["speech.start", "speech.partial", "speech.end"]);
    // Deliberately reverse lexical ID order: wire order must follow invocation.
    let actions: Vec<_> = ["f", "4"].iter().map(|id| json!({"coglet":C,"id":id.repeat(32),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":50_000_000,"correlation_id":id.repeat(32)})).collect();
    s["initial_effect_ids"]["actions"] = json!(["4".repeat(32), "f".repeat(32)]);
    s["initial_actions"] = json!(actions);
    s
}
struct Host {
    dir: PathBuf,
    setup: Value,
    child: Option<Child>,
    ffi: bool,
}
impl Host {
    fn new(ffi: bool, name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("native-operational-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let binding = json!({"surface":S,"permitted_coglets":[C]});
        std::fs::write(
            dir.join("binding.json"),
            serde_json::to_vec(&binding).unwrap(),
        )
        .unwrap();
        let setup = setup();
        std::fs::write(dir.join("setup.json"), serde_json::to_vec(&setup).unwrap()).unwrap();
        if ffi {
            let raw = serde_json::to_vec(&binding).unwrap();
            assert_eq!(
                unsafe { coglet_runtime_configure(raw.as_ptr(), raw.len()) },
                0
            );
        }
        let mut host = Self {
            dir,
            setup,
            child: None,
            ffi,
        };
        host.start();
        host
    }
    fn start(&mut self) {
        if self.ffi {
            let raw =
                serde_json::to_vec(&json!({"wal":self.dir.join("state.wal"),"setup":self.setup}))
                    .unwrap();
            assert_eq!(unsafe { coglet_runtime_start(raw.as_ptr(), raw.len()) }, 0);
        } else {
            self.child = Some(
                Command::new(env!("CARGO_BIN_EXE_cogletd"))
                    .arg("--surface-service-socket")
                    .arg(self.dir.join("sock/control"))
                    .arg(self.dir.join("binding.json"))
                    .arg(unsafe { libc::geteuid() }.to_string())
                    .arg(self.dir.join("state.wal"))
                    .arg(self.dir.join("setup.json"))
                    .spawn()
                    .unwrap(),
            );
        }
    }
    fn stop(&mut self) {
        if self.ffi {
            assert_eq!(coglet_runtime_stop(), 0);
        } else if let Some(mut child) = self.child.take() {
            child.kill().unwrap();
            child.wait().unwrap();
            std::fs::remove_file(self.dir.join("sock/control")).unwrap();
        }
    }
    fn connect(&mut self) -> Link {
        if self.ffi {
            let mut link = std::ptr::null_mut();
            assert_eq!(unsafe { coglet_surface_open(1, &mut link) }, 0);
            Link::Ffi(link)
        } else {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(s) = UnixStream::connect(self.dir.join("sock/control")) {
                    s.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
                    return Link::Socket(s);
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "daemon startup failed"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.stop();
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}
enum Link {
    Ffi(*mut CogletSurfaceLink),
    Socket(UnixStream),
}
impl Link {
    fn send(&mut self, kind: &str, body: Value) {
        let raw = cbor(&json!({"type":kind,"body":body})).unwrap();
        match self {
            Self::Ffi(l) => assert_eq!(
                unsafe { coglet_surface_send(*l, raw.as_ptr(), raw.len()) },
                0
            ),
            Self::Socket(s) => {
                s.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
                s.write_all(&raw).unwrap();
            }
        }
    }
    fn recv(&mut self) -> Vec<u8> {
        match self {
            Self::Ffi(l) => {
                let mut n = 0;
                assert_eq!(
                    unsafe { coglet_surface_recv(*l, std::ptr::null_mut(), 0, &mut n) },
                    2
                );
                let mut raw = vec![0; n];
                assert_eq!(
                    unsafe { coglet_surface_recv(*l, raw.as_mut_ptr(), raw.len(), &mut n) },
                    0
                );
                raw
            }
            Self::Socket(s) => {
                let mut h = [0; 4];
                s.read_exact(&mut h).unwrap();
                let mut raw = vec![0; u32::from_be_bytes(h) as usize];
                s.read_exact(&mut raw).unwrap();
                raw
            }
        }
    }
    fn hello(&mut self) {
        self.send("hello",json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}));
        assert_eq!(decode(&self.recv())["type"], "hello_reply");
    }
    fn grants(&mut self, epoch: u64) {
        self.send("capabilities",json!({"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}));
        self.send("grants",json!({"epoch":epoch,"entries":[{"coglet":C,"operations":["expression.set_expression"],"resources":["display.face"],"expires":100_000_000}],"correlation_id":"3".repeat(32)}));
    }
}
impl Drop for Link {
    fn drop(&mut self) {
        if let Self::Ffi(l) = self {
            unsafe { coglet_surface_close(*l) };
        }
    }
}
fn exercise(ffi: bool, name: &str) {
    let _guard = FFI_LOCK.lock().unwrap();
    let mut host = Host::new(ffi, name);
    let mut link = host.connect();
    link.hello();
    link.grants(1);
    let first = link.recv();
    let second = link.recv();
    assert_eq!(decode(&first)["body"]["action"]["id"], "f".repeat(32));
    assert_eq!(decode(&second)["body"]["action"]["id"], "4".repeat(32));
    if ffi {
        assert_eq!(coglet_runtime_stop(), 1, "stop orphaned a live link");
    }
    let mut effects = std::collections::BTreeMap::new();
    for raw in [&first, &second] {
        effects.insert(
            decode(raw)["body"]["action"]["id"]
                .as_str()
                .unwrap()
                .to_owned(),
            raw.clone(),
        );
    }
    // Clock advances across both new connection and WAL reopen. Retried bytes
    // must precede any fresh capability sample/grant; backend dedup admits no act.
    drop(link);
    host.stop();
    std::thread::sleep(Duration::from_millis(30));
    host.start();
    let mut link = host.connect();
    link.hello();
    for original in [&first, &second] {
        let retry = link.recv();
        assert_eq!(&retry, original, "deadline/epoch was retranslated");
        let id = decode(&retry)["body"]["action"]["id"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(
            effects.get(&id),
            Some(&retry),
            "backend would execute a second effect"
        );
    }
    assert_eq!(effects.len(), 2);
    link.grants(2);
    for raw in [&first, &second] {
        let action = decode(raw)["body"]["action"].clone();
        link.send(
            "accept",
            json!({"id":action["id"],"correlation_id":action["correlation_id"],"accepted":true}),
        );
    }
    link.send(
        "health",
        json!({"state":"ready","clock":1200,"pending":2,"correlation_id":"9".repeat(32)}),
    );
    let h = decode(&link.recv());
    assert_eq!(h["body"]["state"], "ready");
    assert_eq!(h["body"]["pending"], 2);
    for (i, raw) in [&first, &second, &first, &second].into_iter().enumerate() {
        let action = decode(raw)["body"]["action"].clone();
        link.send(
            "event",
            json!({"source":S,"kind":"action.outcome","seq":i+1,
            "logical_time":1300+i,"payload":{"id":action["id"],
            "correlation_id":action["correlation_id"],"status":"succeeded","result":{}},
            "correlation_id":action["correlation_id"]}),
        );
    }
    link.send(
        "health",
        json!({"state":"ready","clock":1400,"pending":0,"correlation_id":"9".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["body"]["pending"], 0);
    link.send(
        "kill",
        json!({"reason":"operator","correlation_id":"8".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["body"]["state"], "stopped");
    drop(link);
    host.stop();
    let mut recovered =
        coglet_runtime::RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128)
            .unwrap();
    assert_eq!(
        recovered.queued(),
        2,
        "terminal deliveries duplicated or lost an inbox wake"
    );
    let now = recovered.supervised_now().unwrap();
    let record = recovered
        .seal_boundary(&host.setup["inputs"]["trace_id"], now, &[])
        .unwrap();
    assert_eq!(
        record["boundary"].as_array().unwrap().len(),
        2,
        "fresh-sequence retries duplicated terminal bytes"
    );
    drop(recovered);
}
#[test]
fn ffi_byte_identical_retry_after_clock_advances_and_reconnect_no_duplicate_act() {
    exercise(true, "ffi");
}
#[test]
fn socket_byte_identical_retry_after_clock_advances_and_reconnect_no_duplicate_act() {
    exercise(false, "socket");
}

fn native_private_capture(ffi: bool, name: &str) {
    let _guard = FFI_LOCK.lock().unwrap();
    let mut host = Host::new(ffi, name);
    let mut link = host.connect();
    link.hello();
    let ops = ["speech.start", "speech.partial", "speech.end"];
    let schemas: serde_json::Map<String, Value> = ops
        .iter()
        .map(|name| {
            (
                name.to_string(),
                json!(format!(
                    "https://coglet.dev/spec/messages/speech.schema.json#/$defs/{}",
                    name.replace('.', "-")
                )),
            )
        })
        .collect();
    link.send("capabilities",json!({"revision":1,"clock":1100,"entries":[{"name":"speech.control",
        "events":ops,"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":schemas}],
        "correlation_id":"2".repeat(32)}));
    link.send("grants",json!({"epoch":1,"entries":[{"coglet":C,"operations":ops,
        "resources":["audio.mic","audio.speaker"],"expires":100_000_000}],"correlation_id":"3".repeat(32)}));
    for (i, (kind, revision)) in [
        ("speech.start", 0),
        ("speech.partial", 1),
        ("speech.partial", 1),
        ("speech.end", 2),
        ("speech.partial", 3),
        ("speech.end", 4),
    ]
    .iter()
    .enumerate()
    {
        let mut payload = json!({"utterance":"d".repeat(32)});
        if *kind != "speech.partial" {
            payload["position_ms"] = json!(10);
        }
        if *kind != "speech.start" {
            payload["text"] = json!("private text");
            payload["revision"] = json!(revision);
        }
        link.send(
            "event",
            json!({"source":S,"kind":kind,"seq":i+1,"logical_time":1200+i,
            "payload":payload,"correlation_id":"e".repeat(32)}),
        );
    }
    // A health roundtrip is a transport barrier after all six event commits.
    link.send(
        "health",
        json!({"state":"ready","clock":1400,"pending":0,"correlation_id":"9".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["type"], "health");
    if !ffi {
        host.stop();
    }
    drop(link);
    if ffi {
        host.stop();
    }
    let store = coglet_runtime::DurableStore::open(&host.dir.join("state.wal")).unwrap();
    let state = store.state().unwrap();
    let key = format!("{S}/{}", "d".repeat(32));
    assert_eq!(state["surface_sequences"][S][0], 6);
    assert_eq!(state["speech_temporal"]["discarded_revisions"], 3);
    assert_eq!(state["speech_temporal"]["utterances"][&key]["revision"], 2);
    assert_eq!(state["speech_temporal"]["utterances"][&key]["ended"], true);
    if !ffi {
        assert_eq!(
            state["private_events"].as_object().unwrap().len(),
            3,
            "native socket queued stale private revisions"
        );
    }
    drop(store);
    let service =
        coglet_runtime::RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128)
            .unwrap();
    assert_eq!(
        service.queued(),
        0,
        "recovery revived disconnected private input"
    );
    drop(service);
}
#[test]
fn ffi_private_capture_filters_late_text_and_persists_end_fence() {
    native_private_capture(true, "ffi-capture");
}
#[test]
fn socket_private_capture_filters_late_text_and_persists_end_fence() {
    native_private_capture(false, "socket-capture");
}

fn library_session(
    service: std::sync::Arc<Mutex<coglet_runtime::RuntimeService>>,
) -> coglet_runtime::SurfaceSession {
    let mut session = coglet_runtime::SurfaceSession::with_service(
        coglet_runtime::SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service,
    )
    .unwrap();
    let messages = [
        json!({"type":"hello","body":{"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}}),
        json!({"type":"capabilities","body":{"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":[],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}}),
        json!({"type":"grants","body":{"epoch":1,"entries":[{"coglet":C,"operations":["expression.set_expression"],"resources":["display.face"],"expires":100_000_000}],"correlation_id":"3".repeat(32)}}),
    ];
    for (i, m) in messages.iter().enumerate() {
        assert_eq!(session.send(&cbor(m).unwrap(), 100 + i as u64), 0);
    }
    session.pop();
    session
}
#[test]
fn expired_undispatched_head_is_durably_rejected_without_starving_later_actions() {
    let dir = std::env::temp_dir().join(format!("native-expired-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut setup = setup();
    setup["initial_actions"][0]["deadline"] = json!(110);
    let service = std::sync::Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&dir.join("state.wal"), &setup, 128).unwrap(),
    ));
    let mut session = library_session(service.clone());
    session.pump_dispatch(120).unwrap();
    assert_eq!(
        decode(&session.pop().unwrap())["body"]["action"]["id"],
        "4".repeat(32)
    );
    assert!(session.pop().is_none());
    assert_eq!(service.lock().unwrap().queued(), 1);
    drop(session);
    drop(service);
    let mut service =
        coglet_runtime::RuntimeService::open(&dir.join("state.wal"), &setup, 128).unwrap();
    let record = service
        .seal_boundary(&setup["inputs"]["trace_id"], 130, &[])
        .unwrap();
    assert_eq!(record["boundary"].as_array().unwrap().len(), 1);
    let bytes = hex::decode(record["boundary"][0]["after"].as_str().unwrap()).unwrap();
    let inbox: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        inbox["event"]["payload"]["error"]["code"],
        "deadline_exceeded"
    );
    drop(service);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn ambiguous_wal_failure_fences_native_clock_and_retained_dispatch() {
    const CHILD: &str = "COGLET_NATIVE_POISON_CHILD";
    if std::env::var_os(CHILD).is_none() {
        assert!(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "ambiguous_wal_failure_fences_native_clock_and_retained_dispatch",
                    "--nocapture"
                ])
                .env(CHILD, "1")
                .status()
                .unwrap()
                .success()
        );
        return;
    }
    let dir = std::env::temp_dir().join(format!("native-poison-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let service = std::sync::Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&dir.join("state.wal"), &setup(), 128).unwrap(),
    ));
    service.lock().unwrap().supervised_now().unwrap();
    let mut session = library_session(service.clone());
    session.pump_dispatch(120).unwrap();
    assert!(session.pop().is_some());
    // Process-isolated real OS write failure, not a fabricated WAL error.
    unsafe {
        libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        let mut old = std::mem::zeroed();
        assert_eq!(libc::getrlimit(libc::RLIMIT_FSIZE, &mut old), 0);
        let limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: old.rlim_max,
        };
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &limit), 0);
        let failed=service.lock().unwrap().enqueue(json!({"source":S,"kind":"expression.state","seq":1,"logical_time":1200,"payload":{},"correlation_id":"7".repeat(32)}));
        assert_eq!(libc::setrlimit(libc::RLIMIT_FSIZE, &old), 0);
        assert!(failed.is_err());
    }
    assert!(service.lock().unwrap().supervised_now().is_err());
    // A new link cannot expose retained envelopes even though no further WAL
    // write would have been needed to return their bytes.
    let mut fresh = coglet_runtime::SurfaceSession::with_service(
        coglet_runtime::SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    assert_eq!(fresh.send(&cbor(&json!({"type":"hello","body":{"surface":S,"offer":{"versions":["0.1"]},"clock":1200,"correlation_id":"1".repeat(32)}})).unwrap(),130),0);
    fresh.pop();
    assert!(fresh.pump_dispatch(130).is_err());
    assert!(fresh.pop().is_none());
    drop(fresh);
    drop(session);
    drop(service);
    std::fs::remove_dir_all(dir).unwrap();
}
