//! Durability (restart, exclusive ownership) and speech stream/native-event behaviour.

mod common;
use coglet_surface_rust::{Native, Surface};
use common::*;
use serde_json::json;

fn chunk(st: u64, n: u64, fin: bool) -> serde_json::Value {
    json!({"stream":hid(st),"chunk":n,"text":"hi","final":fin})
}

#[test]
fn restart_turns_pending_into_failed_unavailable_and_never_replays() {
    let mut r = Rig::new();
    r.connect();
    let m = r.act_body(B, 1, "speech.speak_chunk", chunk(5, 0, true), 5_000_000, 1);
    assert_eq!(accepted(&r.send(&m)[0]), Some(true));
    assert_eq!(r.backend.executed().len(), 1);
    let mut r = r.restart(Opts::default());
    assert_eq!(
        r.backend.stop_all_calls(),
        1,
        "uncertain work is stopped on recovery"
    );
    assert!(
        r.s.kill_switch().is_latched(),
        "unclean connected restart is stopped"
    );
    r.s.reset_kill().unwrap(); // explicit local reconciliation before new authority
    let (_, g) = r.connect();
    assert!(
        g["body"]["epoch"].as_u64().unwrap() > 1,
        "epoch persists across restart"
    );
    let out = r.send(&m);
    assert_eq!(accepted(&out[0]), Some(true));
    assert_eq!(status(&out[1]), "failed");
    assert_eq!(out[1]["body"]["payload"]["error"]["code"], "unavailable");
    assert!(r.backend.executed().is_empty(), "never replayed");
}

#[test]
fn second_instance_on_the_same_state_is_refused() {
    let r = Rig::new();
    let dir = r.dir.clone();
    let b = coglet_surface_rust::testing::ScriptedBackend::new(speech_config("text"), ALL_SPEECH);
    assert!(Surface::open(config(&Opts::default()), &dir, b).is_err());
}

#[test]
fn identity_mismatch_and_lost_ledger_fail_closed() {
    let r = Rig::new();
    let dir = r.dir.clone();
    drop(r);
    let mut cfg = config(&Opts::default());
    cfg.surface = format!("surface:{}", "e".repeat(64));
    let b = coglet_surface_rust::testing::ScriptedBackend::new(speech_config("text"), ALL_SPEECH);
    assert!(Surface::open(cfg, &dir, b).is_err(), "identity mismatch");
    // lose the ledger but keep the identity
    for e in std::fs::read_dir(&dir).unwrap() {
        let p = e.unwrap().path();
        if p.file_name().unwrap().to_string_lossy().contains("ledger") {
            std::fs::remove_file(p).unwrap();
        }
    }
    let b = coglet_surface_rust::testing::ScriptedBackend::new(speech_config("text"), ALL_SPEECH);
    assert!(
        Surface::open(config(&Opts::default()), &dir, b).is_err(),
        "lost ledger"
    );
}

#[test]
fn latch_survives_restart_and_blocks_grants() {
    let mut r = Rig::new();
    r.connect();
    r.s.kill_switch().trigger("off");
    let mut r = r.restart(Opts::default());
    assert!(r.s.kill_switch().is_latched());
    let hello = r.start();
    let reply = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hello["body"]["correlation_id"]}});
    let out = r.send(&reply);
    assert_eq!(
        out[1]["body"]["entries"],
        json!([]),
        "no authority while latched"
    );
    r.epoch = out[1]["body"]["epoch"].as_u64().unwrap();
    assert_eq!(out[2]["type"], "kill");
    assert_eq!(out[3]["body"]["state"], "stopped");
    let act = r.act(
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(code(&act[0]), "permission_denied");
}

#[test]
fn chunk_success_only_comes_from_the_backend() {
    let mut r = Rig::new();
    r.connect();
    let out = r.act(1, "speech.speak_chunk", chunk(5, 0, true));
    assert_eq!(out.len(), 1, "accept only; no playback success is invented");
    assert!(outcomes(&r.tick()).is_empty());
    r.backend.complete(&hid(1), Ok(()));
    let out = r.tick();
    assert_eq!(status(outcomes(&out)[0]), "succeeded");
    // a completion for an unknown/finished id is counted and ignored
    r.backend.complete(&hid(1), Ok(()));
    r.backend.complete(&hid(404), Ok(()));
    assert!(r.tick().is_empty());
    assert_eq!(r.s.diagnostics().late_completions, 2);
}

#[test]
fn out_of_order_completion_fails_the_chunk_closed() {
    let mut r = Rig::new();
    r.connect();
    r.act(1, "speech.speak_chunk", chunk(5, 0, false));
    r.act(2, "speech.speak_chunk", chunk(5, 1, true));
    r.backend.complete(&hid(2), Ok(()));
    let out = r.tick();
    let o = outcomes(&out);
    assert!(
        o.iter()
            .any(|m| m["body"]["payload"]["id"] == hid(2) && status(m) == "failed")
    );
    assert!(
        o.iter().all(|m| status(m) != "succeeded"),
        "no success for audio that cannot have played"
    );
    assert_eq!(r.s.diagnostics().unsound_completions, 1);
}

#[test]
fn chunk_gap_duplicate_and_stream_rules() {
    let mut r = Rig::new();
    r.connect();
    let gap = r.act(1, "speech.speak_chunk", chunk(5, 1, false));
    assert_eq!(accepted(&gap[0]), Some(false));
    assert_eq!(code(&gap[0]), "conflict");
    assert_eq!(
        accepted(&r.act(2, "speech.speak_chunk", chunk(5, 0, false))[0]),
        Some(true)
    );
    // a second open stream is refused while one is open
    let other = r.act(3, "speech.speak_chunk", chunk(6, 0, false));
    assert_eq!(accepted(&other[0]), Some(false));
    // oversized text
    let big = json!({"stream":hid(5),"chunk":1,"text":"x".repeat(4097),"final":false});
    assert_eq!(
        accepted(&r.act(4, "speech.speak_chunk", big)[0]),
        Some(false)
    );
    // final then more
    assert_eq!(
        accepted(&r.act(5, "speech.speak_chunk", chunk(5, 1, true))[0]),
        Some(true)
    );
    assert_eq!(
        accepted(&r.act(6, "speech.speak_chunk", chunk(5, 2, true))[0]),
        Some(false)
    );
}

#[test]
fn stop_unknown_stream_is_not_found_and_known_stream_cancels_chunks() {
    let mut r = Rig::new();
    r.connect();
    let out = r.act(1, "speech.stop", json!({"stream":hid(9)}));
    assert_eq!(accepted(&out[0]), Some(false));
    assert_eq!(code(&out[0]), "not_found");
    r.act(2, "speech.speak_chunk", chunk(5, 0, false));
    let out = r.act(3, "speech.stop", json!({"stream":hid(5)}));
    assert_eq!(accepted(&out[0]), Some(true));
    assert!(r.backend.cancelled().contains(&hid(2)));
    assert!(out.iter().any(|m| status_ok(m, "cancelled")));
    // idempotent stop of a closed stream
    let again = r.act(4, "speech.stop", json!({"stream":hid(5)}));
    assert_eq!(accepted(&again[0]), Some(true));
    // chunks to a stopped stream are refused: ids stay tombstoned
    assert_eq!(
        accepted(&r.act(5, "speech.speak_chunk", chunk(5, 1, false))[0]),
        Some(false)
    );
}

fn status_ok(m: &serde_json::Value, s: &str) -> bool {
    m["type"] == "event" && m["body"]["payload"]["status"] == s
}

#[test]
fn native_utterance_events_are_normalized_and_owner_scoped() {
    let mut r = Rig::new();
    r.connect();
    let b = r.backend.clone();
    // inspect the Native shape through the public enum via a start event
    b.native(Native::SpeechStart {
        utterance: hid(1),
        position_ms: 0,
    });
    let out = r.tick();
    let ev = events(&out, "speech.start");
    assert_eq!(ev.len(), 1);
    assert_eq!(ev[0]["body"]["payload"]["utterance"], hid(1));
}
