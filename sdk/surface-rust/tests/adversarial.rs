mod common;
use coglet_surface_rust::backend::{ActionRequest, Sink};
use coglet_surface_rust::error::ApiError;
use coglet_surface_rust::{Backend, ControlPeer, Native, Surface};
use common::*;
use serde_json::json;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

#[test]
fn failed_latch_write_is_observed_and_cannot_be_reset_blindly() {
    let mut r = Rig::new();
    r.connect();
    std::fs::create_dir(r.dir.join("kill.latch")).unwrap();
    r.s.kill_switch().trigger("disk failed");
    assert!(r.s.tick(0).is_err(), "persistence failure must be observed");
    r.s.disconnect(0).unwrap();
    assert!(r.s.reset_kill().is_err());
    assert!(r.s.start().is_err());
    std::fs::remove_dir(r.dir.join("kill.latch")).unwrap();
    let mut r = r.restart(Opts::default());
    assert!(
        r.s.kill_switch().is_latched(),
        "dirty marker survives missing latch"
    );
    r.s.reset_kill().unwrap();
    r.connect();
}
#[test]
fn rejected_stream_does_not_restore_as_admitted_tombstone() {
    let mut r = Rig::new();
    r.connect();
    let bad = r.act(
        1,
        "speech.speak_chunk",
        json!({"stream":hid(70),"chunk":1,"text":"bad","final":true}),
    );
    assert_eq!(accepted(&bad[0]), Some(false));
    r.s.disconnect(0).unwrap();
    let mut r = r.restart(Opts::default());
    r.connect();
    let good = r.act(
        2,
        "speech.speak_chunk",
        json!({"stream":hid(70),"chunk":0,"text":"good","final":true}),
    );
    assert_eq!(accepted(&good[0]), Some(true));
}
#[derive(Default)]
struct HoldingBackend {
    requests: Mutex<Vec<ActionRequest>>,
    sink: Mutex<Option<Sink>>,
}
impl Backend for HoldingBackend {
    fn operations(&self) -> BTreeSet<String> {
        [
            "speech.backchannel".into(),
            "expression.set_expression".into(),
        ]
        .into()
    }
    fn bind(&self, sink: Sink) {
        *self.sink.lock().unwrap() = Some(sink);
    }
    fn execute(&self, req: &ActionRequest, _: u64) -> Result<(), ApiError> {
        req.kill_fence.check()?;
        self.requests.lock().unwrap().push(req.clone());
        Ok(())
    }
}
#[test]
fn same_wire_id_for_two_owners_has_distinct_native_identity() {
    let mut r = Rig::new();
    let b = Arc::new(HoldingBackend::default());
    let mut cfg = config(&Opts {
        bound_c: true,
        ..Opts::default()
    });
    cfg.grants = vec![
        grant(B, &["speech.backchannel"], &["audio.speaker"], 100_000_000),
        grant(
            C,
            &["expression.set_expression"],
            &["display.face"],
            100_000_000,
        ),
    ];
    r.s = Surface::open(cfg, &tmpdir("collision"), b.clone()).unwrap();
    r.connect();
    for (owner, op, args) in [
        (
            B,
            "speech.backchannel",
            json!({"stream":hid(90),"cue":"ack"}),
        ),
        (
            C,
            "expression.set_expression",
            json!({"expression":"happy","intensity":1,"duration_ms":100}),
        ),
    ] {
        let msg = r.act_body(owner, 1, op, args, 1_000_000, r.epoch);
        let out = r.send(&msg);
        assert_eq!(accepted(&out[0]), Some(true), "{out:?}");
    }
    let executed = b.requests.lock().unwrap().clone();
    assert_eq!(executed.len(), 2);
    assert_ne!(executed[0].id, executed[1].id);
    let sink = b.sink.lock().unwrap().clone().unwrap();
    sink.complete(&executed[0].id, Ok(()));
    assert_eq!(outcomes(&r.tick()).len(), 1);
    assert_eq!(r.s.pending_count(), 1);
    sink.complete(&executed[0].id, Ok(()));
    assert!(outcomes(&r.tick()).is_empty());
    assert_eq!(r.s.pending_count(), 1);
    sink.complete(&executed[1].id, Ok(()));
    assert_eq!(outcomes(&r.tick()).len(), 1);
}

#[test]
fn old_dispatch_fence_remains_stopped_after_operator_reset() {
    let mut r = Rig::new();
    r.connect();
    r.act(
        1,
        "speech.backchannel",
        json!({"stream":hid(60),"cue":"ack"}),
    );
    let req = r.backend.executed()[0].clone();
    let bound = r.backend.sink();
    r.s.kill_switch().trigger("stop");
    r.s.disconnect(0).unwrap();
    r.s.reset_kill().unwrap();
    assert!(
        bound.kill_fence().is_stopped(),
        "native bind captures the kill generation too"
    );
    assert!(
        req.kill_fence.check().is_err(),
        "deferred old job cannot restart after reset"
    );
}
#[test]
fn provider_callback_must_match_captured_session_not_current_session() {
    let mut r = Rig::with(Opts {
        mode: "speech_to_speech",
        routes: &["provider"],
        tools: &["lookup"],
        ..Opts::default()
    });
    r.backend.auto_complete("speech.session_open");
    r.backend.auto_complete("speech.session_close");
    r.connect();
    r.act(
        1,
        "speech.session_open",
        json!({"session":hid(80),"route":"provider"}),
    );
    let old = r.backend.sink().capture(B, Some(&hid(80)));
    r.act(2, "speech.session_close", json!({"session":hid(80)}));
    r.act(
        3,
        "speech.session_open",
        json!({"session":hid(81),"route":"provider"}),
    );
    // A stale provider must not invent a stream belonging to the newer session.
    old.native(Native::Position {
        stream: hid(82),
        chunk: 0,
        item: hid(83),
        played_ms: 1,
        position_ms: 1,
        state: "playing".into(),
    });
    assert!(events(&r.tick(), "speech.position").is_empty());
    // Nor may it name the new session in a tool callback.
    old.native(Native::ToolCall {
        session: hid(81),
        call: hid(84),
        name: "lookup".into(),
        arguments: json!({}),
    });
    assert!(events(&r.tick(), "speech.tool_call").is_empty());
    r.backend
        .sink()
        .capture(B, Some(&hid(81)))
        .native(Native::Position {
            stream: hid(85),
            chunk: 0,
            item: hid(86),
            played_ms: 1,
            position_ms: 1,
            state: "playing".into(),
        });
    assert_eq!(events(&r.tick(), "speech.position").len(), 1);
}

#[test]
fn live_receipt_pruning_releases_ledger_capacity_without_restart() {
    use std::sync::atomic::{AtomicU64, Ordering};
    let mut r = Rig::new();
    let mut cfg = config(&Opts::default());
    cfg.limits.max_ledger = 1;
    let wall = Arc::new(AtomicU64::new(1_000));
    let c = wall.clone();
    r.s = Surface::open_with_clock(
        cfg,
        &tmpdir("live-capacity"),
        r.backend.clone(),
        Arc::new(move || c.load(Ordering::SeqCst)),
    )
    .unwrap();
    r.connect();
    // A rejection consumes one durable receipt, not pending capacity.
    let bad = r.act_body(
        B,
        1,
        "speech.speak_chunk",
        json!({"stream":hid(70),"chunk":1,"text":"bad","final":true}),
        1_000,
        r.epoch,
    );
    assert_eq!(accepted(&r.send(&bad)[0]), Some(false));
    assert_eq!(
        code(
            &r.act(
                2,
                "speech.backchannel",
                json!({"stream":hid(71),"cue":"ack"})
            )[0]
        ),
        "resource_exhausted"
    );
    wall.store(
        1_000 + coglet_surface_rust::store::RETENTION_US + 60_000_001,
        Ordering::SeqCst,
    );
    r.tick();
    assert_eq!(
        accepted(
            &r.act(
                3,
                "speech.backchannel",
                json!({"stream":hid(72),"cue":"ack"})
            )[0]
        ),
        Some(true)
    );
}
#[test]
fn cannot_connect_if_running_marker_cannot_be_persisted() {
    let mut r = Rig::new();
    std::fs::remove_file(r.dir.join("clean.shutdown")).unwrap();
    std::fs::create_dir(r.dir.join("clean.shutdown")).unwrap();
    assert!(r.s.start().is_err());
    assert!(r.s.kill_switch().is_latched());
    assert!(r.backend.executed().is_empty());
}
#[test]
fn unclean_restart_without_pending_work_still_stays_stopped() {
    let mut r = Rig::new();
    r.connect();
    let r = r.restart(Opts::default());
    assert!(r.s.kill_switch().is_latched());
    assert!(r.backend.stop_all_calls() > 0);
}

struct KillBeforeNativeStart {
    kill: Mutex<Option<coglet_surface_rust::KillSwitch>>,
    sem: Arc<coglet_surface_rust::SemanticBackend>,
}
impl Backend for KillBeforeNativeStart {
    fn operations(&self) -> BTreeSet<String> {
        self.sem.operations()
    }
    fn bind(&self, sink: Sink) {
        self.sem.bind(sink);
    }
    fn execute(&self, req: &ActionRequest, now: u64) -> Result<(), ApiError> {
        self.kill
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .trigger("after durable admission");
        self.sem.execute(req, now)
    }
    fn stop_all(&self) {
        self.sem.stop_all();
    }
}
#[test]
fn admitted_action_killed_at_native_boundary_is_cancelled_without_effect() {
    let mut r = Rig::new();
    let sem = coglet_surface_rust::SemanticBackend::new(speech_config("text"), Default::default());
    let b = Arc::new(KillBeforeNativeStart {
        kill: Mutex::new(None),
        sem: sem.clone(),
    });
    r.s = Surface::open(
        config(&Opts::default()),
        &tmpdir("native-boundary"),
        b.clone(),
    )
    .unwrap();
    *b.kill.lock().unwrap() = Some(r.s.kill_switch());
    r.connect();
    let out = r.act(
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":100}),
    );
    assert_eq!(accepted(&out[0]), Some(true));
    assert_eq!(status(outcomes(&out)[0]), "cancelled");
    assert_eq!(sem.state(0).0, "neutral");
    assert_eq!(r.s.pending_count(), 0);
}

struct KillInPreflight {
    kill: Mutex<Option<coglet_surface_rust::KillSwitch>>,
    executed: Mutex<usize>,
}
impl Backend for KillInPreflight {
    fn operations(&self) -> BTreeSet<String> {
        ["expression.set_expression".into()].into()
    }
    fn preflight(&self, _: &ActionRequest, _: u64) -> Result<(), ApiError> {
        self.kill
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .trigger("inside preflight");
        Ok(())
    }
    fn execute(&self, _: &ActionRequest, _: u64) -> Result<(), ApiError> {
        *self.executed.lock().unwrap() += 1;
        Ok(())
    }
}
#[test]
fn kill_inside_preflight_never_dispatches() {
    let b = Arc::new(KillInPreflight {
        kill: Mutex::new(None),
        executed: Mutex::new(0),
    });
    let mut s = Surface::open(config(&Opts::default()), &tmpdir("race"), b.clone()).unwrap();
    *b.kill.lock().unwrap() = Some(s.kill_switch());
    let h = decode(&s.start().unwrap()[0]);
    s.receive(&encode(&json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":h["body"]["correlation_id"]}})), 0).unwrap();
    let msg = json!({"type":"act","body":{"grant_epoch":1,"action":{"id":hid(1),"coglet":B,"capability":"expression.set_expression","args":{"expression":"happy","intensity":1,"duration_ms":100},"deadline":1000,"correlation_id":hid(1)}}});
    s.receive(&encode(&msg), 0).unwrap();
    assert_eq!(*b.executed.lock().unwrap(), 0);
}
#[test]
fn old_native_sender_and_queued_start_are_fenced_across_epochs() {
    let mut r = Rig::new();
    r.connect();
    let old: Sink = r.backend.sink();
    old.native(Native::SpeechStart {
        utterance: hid(50),
        position_ms: 0,
    });
    r.s.revoke(0, B).unwrap();
    r.s.install_grants(
        0,
        vec![grant(
            B,
            ALL_SPEECH,
            &["audio.mic", "audio.speaker", "display.face"],
            100_000_000,
        )],
    )
    .unwrap();
    old.native(Native::SpeechStart {
        utterance: hid(51),
        position_ms: 0,
    });
    assert!(events(&r.tick(), "speech.start").is_empty());
    r.backend.native(Native::SpeechStart {
        utterance: hid(52),
        position_ms: 0,
    });
    assert_eq!(events(&r.tick(), "speech.start").len(), 1);
}
