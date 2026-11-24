//! Real WAL commits with deterministic kernel-clock and commit-delay injection.
//! Neither seam exists in production builds; no sleeps or deadline relaxations.
use super::*;
use crate::{SurfaceBinding, SurfaceSession};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const ITEM: &str = "44444444444444444444444444444444";
const OFFSET: u64 = 900;

struct Fixture {
    session: SurfaceSession,
    service: Arc<Mutex<RuntimeService>>,
    clock: Arc<AtomicU64>,
    dir: std::path::PathBuf,
}
fn frame(kind: &str, body: Value) -> Vec<u8> {
    cbor(&json!({"type":kind,"body":body})).unwrap()
}
fn decode(raw: &[u8]) -> Value {
    coglet_types::decode("surface-message", raw)
        .unwrap()
        .deserialized()
        .unwrap()
}
fn position(seq: u64) -> Vec<u8> {
    frame(
        "event",
        json!({"source":S,"seq":seq,"logical_time":1200+seq,
        "kind":"speech.position","payload":{"stream":"a".repeat(32),"chunk":0,
        "item":ITEM,"played_ms":seq*10,"position_ms":seq*10,"state":"playing"},
        "correlation_id":"6".repeat(32)}),
    )
}
fn reply(read: &Value, seq: u64) -> Vec<u8> {
    frame(
        "read_reply",
        json!({"correlation_id":read["body"]["correlation_id"],
        "ok":true,"snapshot":seq,"value":{"item":ITEM,"played_ms":seq*10,"text":"hello"}}),
    )
}
impl Fixture {
    fn new(name: &str) -> Self {
        Self::with_expiry(name, 100_000_000)
    }
    fn with_expiry(name: &str, expires: u64) -> Self {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["name"] == "no-effects")
            .unwrap()["steps"][0]["send"]
            .clone();
        setup["manifest"]["id"] = json!(C);
        setup["manifest"]["subscriptions"] = json!(["speech.position"]);
        setup["initial_actions"] = json!([{"coglet":C,"id":ITEM,"capability":"speech.speak_chunk",
            "args":{"stream":"a".repeat(32),"chunk":0,"text":"hello","final":true},
            "deadline":50_000_000,"correlation_id":"5".repeat(32)}]);
        setup["initial_effect_ids"]["actions"] = json!([ITEM]);
        let dir =
            std::env::temp_dir().join(format!("heard-deadline-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut service = RuntimeService::open(&dir.join("runtime.wal"), &setup, 128).unwrap();
        let clock = Arc::new(AtomicU64::new(0));
        let sample = clock.clone();
        service.test_clock_sample = Some(Arc::new(move || {
            Ok(("test-boot".into(), sample.load(Ordering::SeqCst)))
        }));
        assert_eq!(service.supervised_now().unwrap(), 0);
        let service = Arc::new(Mutex::new(service));
        let mut session = SurfaceSession::with_service(
            SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
            service.clone(),
        )
        .unwrap();
        for (kind, body) in [
            (
                "hello",
                json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            ),
            (
                "capabilities",
                json!({"revision":1,"clock":1000,"entries":[
                {"name":"speech.control","events":["speech.position"],"reads":[],"actions":["speech.speak_chunk"],"exclusive":["audio.speaker"],"schemas":{"speech.position":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-position","speech.speak_chunk":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk"}},
                {"name":"speech.history","events":[],"reads":["speech.heard"],"actions":[],"exclusive":[],"schemas":{"speech.heard":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/heard-record"}}
            ],"correlation_id":"2".repeat(32)}),
            ),
            (
                "grants",
                json!({"epoch":1,"entries":[{"coglet":C,"operations":["speech.position","speech.heard","speech.speak_chunk"],"resources":["audio.speaker"],"expires":expires}],"correlation_id":"3".repeat(32)}),
            ),
        ] {
            assert_eq!(session.send(&frame(kind, body), 100), 0);
        }
        session.pop().unwrap();
        {
            let mut guard = service.lock().unwrap();
            guard.prepare_dispatch(&session, ITEM, 110).unwrap();
            guard
                .record_surface_accept(
                    &session,
                    &json!({"id":ITEM,"correlation_id":"5".repeat(32),"accepted":true}),
                    120,
                )
                .unwrap();
        }
        clock.store(130, Ordering::SeqCst);
        Self {
            session,
            service,
            clock,
            dir,
        }
    }
    fn slow_next_commit(&self, delay: u64) {
        let clock = self.clock.clone();
        let mut first = true;
        self.service.lock().unwrap().store.before_commit = Some(Box::new(move || {
            if first {
                first = false;
                clock.fetch_add(delay, Ordering::SeqCst);
            }
        }));
    }
    fn send(&mut self, raw: &[u8]) -> i32 {
        self.session.send(raw, self.clock.load(Ordering::SeqCst))
    }
    fn read(&mut self) -> Value {
        let read = decode(&self.session.pop().expect("published heard read"));
        assert_eq!(read["type"], "read");
        assert_eq!(read["body"]["capability"], "speech.heard");
        assert_eq!(read["body"]["args"], json!({"item":ITEM}));
        read
    }
    fn assert_purged(&self) {
        assert!(self.session.is_closed());
        let service = self.service.lock().unwrap();
        assert!(service.state.heard_pending.is_empty());
        assert!(service.state.private_events.is_empty());
        assert_eq!(service.queued(), 0);
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.service.lock().unwrap().store.before_commit = None;
        self.session.disconnect();
        // All handles are local to this fixture; unlink after final assertions.
        std::fs::remove_dir_all(&self.dir).unwrap();
    }
}

#[test]
fn slow_durable_store_gives_heard_read_one_second_from_publication() {
    let mut f = Fixture::new("slow");
    f.slow_next_commit(600_000);
    assert_eq!(f.send(&position(1)), 0);
    assert_eq!(f.clock.load(Ordering::SeqCst), 600_130);
    let read = f.read();
    assert_eq!(read["body"]["deadline"], 1_600_130 + OFFSET);
    // Beyond the OLD entry-based deadline, still before the issued deadline.
    f.clock.store(1_200_130, Ordering::SeqCst);
    assert_eq!(f.send(&reply(&read, 1)), 0);
    assert!(!f.session.is_closed());
    let service = f.service.lock().unwrap();
    assert!(service.state.heard_pending.is_empty());
    assert_eq!(
        service
            .state
            .staged_boundary
            .as_ref()
            .unwrap()
            .changes
            .len(),
        1
    );
    // Actual committed WAL state, not only session acceptance.
    assert_eq!(service.store.state().unwrap()["heard_pending"], json!({}));
}

#[test]
fn queued_replies_behind_slow_store_remain_fifo_and_do_not_extend_issued_deadlines() {
    let mut f = Fixture::new("fifo");
    assert_eq!(f.send(&position(1)), 0);
    let first = f.read();
    let first_deadline = first["body"]["deadline"].as_u64().unwrap();
    let (tx, rx) = mpsc::channel();
    let clock = f.clock.clone();
    let first_reply = reply(&first, 1);
    let mut once = true;
    // While the second event's durable commit owns the serial coordinator,
    // enqueue a reply and a later control. No priority lane may overtake them.
    f.service.lock().unwrap().store.before_commit = Some(Box::new(move || {
        if once {
            once = false;
            tx.send(first_reply.clone()).unwrap();
            tx.send(position(3)).unwrap();
            clock.fetch_add(500_000, Ordering::SeqCst);
        }
    }));
    assert_eq!(f.send(&position(2)), 0);
    let second = f.read();
    assert_eq!(second["body"]["deadline"], first_deadline + 500_000);
    let mut processed = Vec::new();
    for raw in rx.try_iter() {
        processed.push(decode(&raw)["type"].clone());
        assert_eq!(f.send(&raw), 0);
    }
    assert_eq!(
        processed,
        json!(["read_reply", "event"]).as_array().unwrap().clone()
    );
    let third = f.read();
    // Reply #1 must have been recorded before event #3; one archive mutation,
    // barriers #2 and #3 still live, and source sequence remains FIFO.
    {
        let service = f.service.lock().unwrap();
        assert_eq!(
            service
                .state
                .staged_boundary
                .as_ref()
                .unwrap()
                .changes
                .len(),
            1
        );
        assert_eq!(service.state.heard_pending.len(), 2);
        assert_eq!(service.state.surface_sequences[S].0, 3);
    }
    assert_eq!(f.send(&reply(&second, 2)), 0);
    assert_eq!(f.send(&reply(&third, 3)), 0);
    assert!(!f.session.is_closed());

    // An independent issued read survives a subsequent slow commit unchanged;
    // expiry still uses its original (earlier) deadline, not the newer read's.
    assert_eq!(f.send(&position(4)), 0);
    let old = f.read();
    let cutoff = old["body"]["deadline"].as_u64().unwrap() - OFFSET;
    f.slow_next_commit(500_000);
    assert_eq!(f.send(&position(5)), 0);
    let newer = f.read();
    assert_eq!(newer["body"]["deadline"], cutoff + OFFSET + 500_000);
    f.session.tick(cutoff);
    f.assert_purged();
}

#[test]
fn reply_at_or_after_published_deadline_disconnects_and_purges_private_state() {
    for late in [0, 1] {
        let mut f = Fixture::new(&format!("late-{late}"));
        f.slow_next_commit(600_000);
        assert_eq!(f.send(&position(1)), 0);
        let read = f.read();
        let deadline = read["body"]["deadline"].as_u64().unwrap() - OFFSET;
        f.clock.store(deadline + late, Ordering::SeqCst);
        f.send(&reply(&read, 1));
        f.assert_purged();
        assert!(f.session.front().is_none());
        assert!(f.service.lock().unwrap().state.staged_boundary.is_none());
    }
}

#[test]
fn negative_heard_reply_is_fail_closed_not_a_successful_empty_observation() {
    // Run307 only retained ok:false, not its error code. Exercise distinct
    // admissible negative replies without claiming which one occurred there.
    for code in [
        "deadline_exceeded",
        "not_found",
        "permission_denied",
        "unavailable",
    ] {
        let mut f = Fixture::new(code);
        assert_eq!(f.send(&position(1)), 0);
        let read = f.read();
        let raw = frame(
            "read_reply",
            json!({"correlation_id":read["body"]["correlation_id"],
            "ok":false,"error":{"code":code,"message":"unavailable observation","retryable":false}}),
        );
        assert_eq!(f.send(&raw), -2);
        f.assert_purged();
        assert!(f.service.lock().unwrap().state.staged_boundary.is_none());
    }
}

#[test]
fn slow_store_does_not_renew_expired_grants_or_inactivity() {
    for delay in [2_000_000, 3_000_000] {
        let expires = if delay == 2_000_000 {
            1_000_000
        } else {
            100_000_000
        };
        let mut f = Fixture::with_expiry(&format!("fences-{delay}"), expires);
        f.slow_next_commit(delay);
        assert_eq!(f.send(&position(1)), -2);
        assert_eq!(f.clock.load(Ordering::SeqCst), 130 + delay);
        f.assert_purged();
    }
}
