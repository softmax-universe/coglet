mod common;
use coglet_surface_rust::Native;
use common::*;
use serde_json::json;

fn observation(stream: u64, item: String) -> Native {
    Native::Position {
        stream: hid(stream),
        chunk: 0,
        item,
        played_ms: 1,
        position_ms: 1,
        state: "playing".into(),
    }
}
fn exercise(op: &str) {
    let mut r = Rig::new();
    r.connect();
    let args = if op == "speech.backchannel" {
        json!({"stream":hid(50),"cue":"ack"})
    } else {
        json!({"stream":hid(50),"chunk":0,"text":"hello","final":true})
    };
    assert_eq!(accepted(&r.act(7, op, args)[0]), Some(true));
    let request = r.backend.executed()[0].clone();
    assert_ne!(request.id, request.wire_id);
    r.backend.native(observation(50, request.id.clone()));
    assert!(
        events(&r.tick(), "speech.position").is_empty(),
        "native token must not acquire wire item authority"
    );
    r.backend.native(observation(50, request.wire_id.clone()));
    let out = r.tick();
    assert_eq!(
        events(&out, "speech.position")[0]["body"]["payload"]["item"],
        hid(7)
    );
    r.backend.native(Native::SpeechStart {
        utterance: hid(100),
        position_ms: 0,
    });
    r.tick();
    r.backend.native(Native::BargeIn {
        utterance: hid(100),
        stream: hid(50),
        item: request.wire_id,
        played_ms: 1,
        position_ms: 1,
        reflex: "stopped".into(),
    });
    let out = r.tick();
    assert_eq!(
        events(&out, "speech.barge_in")[0]["body"]["payload"]["item"],
        hid(7)
    );
    assert_eq!(outcomes(&out)[0]["body"]["payload"]["id"], hid(7));
    assert_eq!(r.backend.cancelled(), [hid(7)]);
    r.backend.complete(&request.id, Ok(()));
    assert!(
        outcomes(&r.tick()).is_empty(),
        "late native completion after reflex is fenced"
    );
}
#[test]
fn speak_item_is_original_action_id_not_native_token() {
    exercise("speech.speak_chunk");
}
#[test]
fn backchannel_item_is_original_action_id_not_native_token() {
    exercise("speech.backchannel");
}

#[test]
fn reused_owner_scoped_wire_id_remains_bound_to_its_stream_and_native_token() {
    let mut r = Rig::with(Opts {
        bound_c: true,
        ..Opts::default()
    });
    r.connect();
    r.act(
        7,
        "speech.speak_chunk",
        json!({"stream":hid(50),"chunk":0,"text":"old","final":true}),
    );
    let old = r.backend.executed()[0].clone();
    let old_sender = r.backend.sink();
    let grants =
        r.s.install_grants(
            1,
            vec![grant(
                C,
                ALL_SPEECH,
                &["audio.mic", "audio.speaker", "display.face"],
                100_000_000,
            )],
        )
        .unwrap();
    r.now = 1;
    r.epoch = decode(grants.last().unwrap())["body"]["epoch"]
        .as_u64()
        .unwrap();
    let act = r.act_body(
        C,
        7,
        "speech.backchannel",
        json!({"stream":hid(51),"cue":"ack"}),
        5_000_000,
        r.epoch,
    );
    assert_eq!(accepted(&r.send(&act)[0]), Some(true));
    let new = r.backend.executed()[1].clone();
    assert_ne!(old.id, new.id);
    assert_eq!(old.wire_id, new.wire_id);
    old_sender.native(observation(50, old.wire_id));
    r.backend.sink().native(observation(51, new.id.clone()));
    assert!(events(&r.tick(), "speech.position").is_empty());
    r.backend.sink().native(observation(51, new.wire_id));
    let out = r.tick();
    let position = events(&out, "speech.position")[0];
    assert_eq!(position["body"]["payload"]["stream"], hid(51));
    assert_eq!(position["body"]["payload"]["item"], hid(7));
    r.backend.complete(&old.id, Ok(()));
    assert!(outcomes(&r.tick()).is_empty());
    r.backend.complete(&new.id, Ok(()));
    let out = r.tick();
    assert_eq!(status(outcomes(&out)[0]), "succeeded");
}
