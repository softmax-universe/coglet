use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
use serde_json::{Value, json};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn send(session: &mut SurfaceSession, kind: &str, body: Value, now: u64) -> i32 {
    session.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now)
}
fn session() -> SurfaceSession {
    let mut s = SurfaceSession::new(SurfaceBinding::new(S.into(), vec![C.into()]).unwrap());
    assert_eq!(
        send(
            &mut s,
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            10
        ),
        0
    );
    s.pop().unwrap();
    s
}
fn caps(s: &mut SurfaceSession) {
    assert_eq!(
        send(
            s,
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":["expression.state"],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.state":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/expression-state","expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}),
            110
        ),
        0
    );
}
fn grants(s: &mut SurfaceSession, epoch: u64) {
    assert_eq!(
        send(
            s,
            "grants",
            json!({"epoch":epoch,"entries":[{"coglet":C,"operations":["expression.state","expression.set_expression"],"resources":["display.face"],"expires":1_000_000}],"correlation_id":"3".repeat(32)}),
            90 + 30 * epoch
        ),
        0
    );
}
fn action() -> Value {
    json!({"coglet":C,"id":"4".repeat(32),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":20_000,"correlation_id":"5".repeat(32)})
}
fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["manifest"]["id"] = json!(C);
    setup["initial_effect_ids"]["actions"] = json!([action()["id"]]);
    setup["initial_actions"] = json!([action()]);
    setup
}
fn wal(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("coglet-dispatch-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("state.wal")
}
#[test]
fn native_read_is_caller_scoped_and_timeout_discards_late_reply() {
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let id = "a".repeat(32);
    s.request_read(C, "expression.state", json!({}), &id, 500, 130)
        .unwrap();
    let raw = s.pop().unwrap();
    let request: Value = coglet_types::decode("surface-message", &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(request["body"]["deadline"], 1490);
    assert_eq!(request["body"]["grant_epoch"], 1);
    assert!(
        s.request_read(C, "expression.state", json!({}), &id, 500, 140)
            .is_err()
    );
    let reply = json!({"correlation_id":id,"ok":true,"snapshot":1,"value":{"expression":"happy","intensity":500,"remaining_ms":10}});
    assert_eq!(send(&mut s, "read_reply", reply.clone(), 150), 0);
    assert!(
        s.take_read_result(
            "coglet:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
            160
        )
        .is_none()
    );
    assert_eq!(s.take_read_result(C, 160).unwrap(), reply);
    let late = "b".repeat(32);
    s.request_read(C, "expression.state", json!({}), &late, 200, 170)
        .unwrap();
    s.tick(200);
    assert!(s.pop().is_none(), "timed-out read remained queued");
    assert_eq!(
        s.take_read_result(C, 200).unwrap()["error"]["code"],
        "deadline_exceeded"
    );
    let mut reply = reply;
    reply["correlation_id"] = json!(late);
    assert_eq!(send(&mut s, "read_reply", reply, 210), 0);
    assert!(s.take_read_result(C, 220).is_none());
}
#[test]
fn native_read_revocation_purges_pending_and_completed_private_values() {
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let id = "a".repeat(32);
    s.request_read(C, "expression.state", json!({}), &id, 500, 130)
        .unwrap();
    s.pop();
    assert_eq!(
        send(
            &mut s,
            "read_reply",
            json!({"correlation_id":id,"ok":true,"snapshot":1,"value":{"expression":"happy","intensity":500,"remaining_ms":10}}),
            140
        ),
        0
    );
    s.request_read(C, "expression.state", json!({}), &"b".repeat(32), 500, 150)
        .unwrap();
    assert_eq!(
        send(
            &mut s,
            "grants",
            json!({"epoch":2,"entries":[],"correlation_id":"c".repeat(32)}),
            160
        ),
        0
    );
    assert!(s.pop().is_none());
    let result = s.take_read_result(C, 170).unwrap();
    assert_eq!(result["error"]["code"], "permission_denied");
    assert!(s.take_read_result(C, 170).is_none());
}
#[test]
fn native_read_invalid_selected_schema_and_unknown_reply_fail_closed() {
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    assert!(
        s.request_read(
            C,
            "expression.state",
            json!({"extra":true}),
            &"a".repeat(32),
            500,
            130
        )
        .is_err()
    );
    assert!(s.pop().is_none());
    s.request_read(C, "expression.state", json!({}), &"a".repeat(32), 500, 130)
        .unwrap();
    assert_eq!(
        send(
            &mut s,
            "read_reply",
            json!({"correlation_id":"a".repeat(32),"ok":true,"snapshot":1,"value":{}}),
            140
        ),
        -2
    );
    assert!(s.take_read_result(C, 150).is_none());
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    assert_eq!(
        send(
            &mut s,
            "read_reply",
            json!({"correlation_id":"a".repeat(32),"ok":false,"error":{"code":"unavailable","message":"no","retryable":false}}),
            130
        ),
        -2
    );
}
fn live(
    service: std::sync::Arc<std::sync::Mutex<coglet_runtime::RuntimeService>>,
) -> SurfaceSession {
    let mut s = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service,
    )
    .unwrap();
    assert_eq!(
        send(
            &mut s,
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            10
        ),
        0
    );
    s.pop();
    s
}
fn speech(s: &mut SurfaceSession, epoch: u64) {
    if epoch == 1 {
        assert_eq!(
            send(
                s,
                "capabilities",
                json!({"revision":1,"clock":1100,"entries":[{"name":"speech.control","events":["speech.start"],"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":{"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start"}}],"correlation_id":"2".repeat(32)}),
                110
            ),
            0
        );
    }
    assert_eq!(
        send(
            s,
            "grants",
            json!({"epoch":epoch,"entries":[{"coglet":C,"operations":["speech.start"],"resources":["audio.mic","audio.speaker"],"expires":500_000}],"correlation_id":"3".repeat(32)}),
            if epoch == 2 { 135 } else { 120 + epoch }
        ),
        0
    );
}
fn speech_event(seq: u64) -> Value {
    json!({"source":S,"kind":"speech.start","seq":seq,"logical_time":1200,"payload":{"utterance":"d".repeat(32),"position_ms":0},"correlation_id":"e".repeat(32)})
}
#[test]
fn private_event_revoke_disconnect_restart_and_source_highwater() {
    use std::sync::{Arc, Mutex};
    let path = wal("private");
    let mut setup = setup();
    setup["manifest"]["subscriptions"] = json!(["speech.start"]);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut s = live(service.clone());
    speech(&mut s, 1);
    assert_eq!(send(&mut s, "event", speech_event(1), 130), 0);
    assert_eq!(service.lock().unwrap().queued(), 1);
    speech(&mut s, 2);
    assert_eq!(service.lock().unwrap().queued(), 0);
    assert_eq!(send(&mut s, "event", speech_event(2), 140), 0);
    s.disconnect();
    assert_eq!(service.lock().unwrap().queued(), 0);
    drop(s);
    drop(service);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut s = live(service.clone());
    assert_eq!(
        send(
            &mut s,
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[{"name":"speech.control","events":["speech.start"],"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":{"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start"}}],"correlation_id":"2".repeat(32)}),
            110
        ),
        0
    );
    speech(&mut s, 3);
    assert_eq!(
        send(&mut s, "event", speech_event(2), 140),
        -2,
        "source seq rollback survived restart"
    );
    assert_eq!(service.lock().unwrap().queued(), 0);
    drop(s);
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn private_event_expiry_purges_before_any_guest_step() {
    use std::sync::{Arc, Mutex};
    let path = wal("expiry");
    let mut setup = setup();
    setup["manifest"]["subscriptions"] = json!(["speech.start"]);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut s = live(service.clone());
    speech(&mut s, 1);
    assert_eq!(send(&mut s, "event", speech_event(1), 130), 0);
    let mut inputs = setup["inputs"].clone();
    inputs["logical_time"] = json!(500_000);
    assert!(
        service
            .lock()
            .unwrap()
            .run_next(
                &inputs,
                &mut coglet_runtime::MockEngine::new(setup["guest_script"].clone())
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(service.lock().unwrap().queued(), 0);
    drop(s);
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn concurrent_session_epoch_purges_old_read_reply_and_completed_result() {
    use std::sync::{Arc, Mutex};
    let path = wal("read-epoch");
    let setup = setup();
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut old = live(service.clone());
    caps(&mut old);
    grants(&mut old, 1);
    let id = "a".repeat(32);
    old.request_read(C, "expression.state", json!({}), &id, 500, 130)
        .unwrap();
    old.pop();
    let mut fresh = live(service.clone());
    caps(&mut fresh);
    grants(&mut fresh, 2);
    assert_eq!(
        send(
            &mut old,
            "read_reply",
            json!({"correlation_id":id,"ok":true,"snapshot":1,"value":{"expression":"happy","intensity":500,"remaining_ms":10}}),
            140
        ),
        0
    );
    assert_eq!(
        old.take_read_result(C, 150).unwrap()["error"]["code"],
        "permission_denied"
    );
    assert!(old.take_read_result(C, 150).is_none());
    drop(old);
    drop(fresh);
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn heard_reads_require_speaker_owned_retained_item_and_enforce_utf8_limit() {
    use std::sync::{Arc, Mutex};
    let path = wal("heard-read");
    let mut setup = setup();
    let mut speak = action();
    speak["capability"] = json!("speech.speak_chunk");
    speak["args"] = json!({"stream":"6".repeat(32),"chunk":0,"text":"hello","final":true});
    setup["initial_actions"] = json!([speak.clone()]);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut s = live(service.clone());
    assert_eq!(
        send(
            &mut s,
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[
        {"name":"speech.control","events":[],"reads":[],"actions":["speech.speak_chunk"],"exclusive":["audio.speaker"],"schemas":{"speech.speak_chunk":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk"}},
        {"name":"speech.history","events":[],"reads":["speech.heard"],"actions":[],"exclusive":[],"schemas":{"speech.heard":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/heard-record"}}
    ],"correlation_id":"2".repeat(32)}),
            110
        ),
        0
    );
    let grant = |epoch, resources| json!({"epoch":epoch,"entries":[{"coglet":C,"operations":["speech.heard","speech.speak_chunk"],"resources":resources,"expires":1_000_000}],"correlation_id":"3".repeat(32)});
    assert_eq!(send(&mut s, "grants", grant(1, json!([])), 120), 0);
    assert!(
        s.request_read(
            C,
            "speech.heard",
            json!({"item":speak["id"]}),
            &"a".repeat(32),
            1000,
            130
        )
        .is_err()
    );
    assert_eq!(
        send(&mut s, "grants", grant(2, json!(["audio.speaker"])), 140),
        0
    );
    assert!(
        s.request_read(
            C,
            "speech.heard",
            json!({"item":speak["id"]}),
            &"a".repeat(32),
            1000,
            150
        )
        .is_err()
    );
    service
        .lock()
        .unwrap()
        .prepare_dispatch(&s, speak["id"].as_str().unwrap(), 150)
        .unwrap();
    service
        .lock()
        .unwrap()
        .record_surface_accept(
            &s,
            &json!({"id":speak["id"],"correlation_id":speak["correlation_id"],"accepted":true}),
            160,
        )
        .unwrap();
    s.request_read(
        C,
        "speech.heard",
        json!({"item":speak["id"]}),
        &"a".repeat(32),
        1000,
        170,
    )
    .unwrap();
    s.pop();
    let mut reply = json!({"correlation_id":"a".repeat(32),"ok":true,"snapshot":1,"value":{"item":speak["id"],"played_ms":1,"text":"hello"}});
    assert_eq!(send(&mut s, "read_reply", reply.clone(), 180), 0);
    assert_eq!(s.take_read_result(C, 190).unwrap(), reply);
    s.request_read(
        C,
        "speech.heard",
        json!({"item":speak["id"]}),
        &"b".repeat(32),
        1000,
        200,
    )
    .unwrap();
    s.pop();
    reply["correlation_id"] = json!("b".repeat(32));
    reply["value"]["text"] = json!("é".repeat(16000));
    assert_eq!(send(&mut s, "read_reply", reply, 210), -2);
    assert!(s.take_read_result(C, 220).is_none());
    drop(s);
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn private_queued_input_is_not_revived_on_wal_reopen() {
    use std::sync::{Arc, Mutex};
    let path = wal("private-crash");
    let mut setup = setup();
    setup["manifest"]["subscriptions"] = json!(["speech.start"]);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut s = live(service.clone());
    speech(&mut s, 1);
    assert_eq!(send(&mut s, "event", speech_event(1), 130), 0);
    assert_eq!(service.lock().unwrap().queued(), 1);
    // Abandon the connection object without the normal disconnect callback,
    // modelling a process loss: recovery must still purge old private input.
    drop(s);
    drop(service);
    let recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    assert_eq!(recovered.queued(), 0);
    drop(recovered);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn native_read_pending_and_completed_share_one_bounded_queue() {
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    for n in 0..128 {
        s.request_read(
            C,
            "expression.state",
            json!({}),
            &format!("{n:032x}"),
            1000,
            130,
        )
        .unwrap();
        s.pop();
    }
    assert!(
        s.request_read(C, "expression.state", json!({}), &"f".repeat(32), 1000, 140)
            .is_err()
    );
    s.tick(1000);
    assert!(
        s.request_read(
            C,
            "expression.state",
            json!({}),
            &"f".repeat(32),
            2000,
            1000
        )
        .is_err()
    );
    assert_eq!(
        s.take_read_result(C, 1000).unwrap()["error"]["code"],
        "deadline_exceeded"
    );
    assert!(
        s.request_read(
            C,
            "expression.state",
            json!({}),
            &"f".repeat(32),
            2000,
            1000
        )
        .is_ok()
    );
    s.disconnect();
    assert!(s.take_read_result(C, 1001).is_none());
}
#[test]
fn durable_grant_epoch_survives_reconnect_and_restart() {
    use std::sync::{Arc, Mutex};
    let path = wal("epoch");
    let setup = setup();
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut first = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    assert_eq!(
        send(
            &mut first,
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            10
        ),
        0
    );
    caps(&mut first);
    grants(&mut first, 9);
    first.disconnect();
    drop(first);
    drop(service);
    let service = Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap(),
    ));
    let mut fresh = SurfaceSession::with_service(
        SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service.clone(),
    )
    .unwrap();
    assert_eq!(fresh.grant_epoch(), 9);
    assert_eq!(
        send(
            &mut fresh,
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
            10
        ),
        0
    );
    caps(&mut fresh);
    assert_eq!(
        send(
            &mut fresh,
            "grants",
            json!({"epoch":1,"entries":[],"correlation_id":"2".repeat(32)}),
            120
        ),
        -2
    );
    drop(fresh);
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn terminal_rejection_is_one_durable_append_and_zero_step_export_includes_it() {
    let path = wal("rejected");
    let setup = setup();
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    service
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 130)
        .unwrap();
    let rejection = json!({"id":action()["id"],"correlation_id":action()["correlation_id"],"accepted":false,"error":{"code":"permission_denied","message":"revoked","retryable":false}});
    service.record_surface_accept(&s, &rejection, 140).unwrap();
    service.record_surface_accept(&s, &rejection, 150).unwrap();
    assert_eq!(service.queued(), 1);
    drop(service);
    let mut recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let record = recovered
        .seal_boundary(&setup["inputs"]["trace_id"], 200, &[])
        .unwrap();
    assert_eq!(record["boundary"].as_array().unwrap().len(), 1);
    assert_eq!(recovered.queued(), 1, "seal duplicated the staged wake");
    drop(recovered);
    let recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    drop(recovered);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn retries_remain_reconcilable_until_deadline_plus_retention() {
    let path = wal("retention");
    let setup = setup();
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let id = action()["id"].as_str().unwrap().to_owned();
    let bytes = service.prepare_dispatch(&s, &id, 130).unwrap();
    let mut fresh = SurfaceSession::new(SurfaceBinding::new(S.into(), vec![C.into()]).unwrap());
    let now = 86_400_000_140;
    assert_eq!(
        send(
            &mut fresh,
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":now,"correlation_id":"1".repeat(32)}),
            now
        ),
        0
    );
    assert_eq!(service.prepare_dispatch(&fresh, &id, now).unwrap(), bytes);
    let cutoff = 86_400_020_000;
    assert!(service.prepare_dispatch(&fresh, &id, cutoff).is_err());
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn optional_work_requires_dual_audio_leases_and_semantic_value_byte_cap() {
    let mut s = session();
    let entries = json!([
        {"name":"speech.control","events":[],"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":{}},
        {"name":"speech.work","events":[],"reads":[],"actions":["speech.work_result"],"exclusive":[],"schemas":{"speech.work_result":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/work-result"}}
    ]);
    assert_eq!(
        send(
            &mut s,
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":entries,"correlation_id":"2".repeat(32)}),
            110
        ),
        0
    );
    assert_eq!(
        send(
            &mut s,
            "grants",
            json!({"epoch":1,"entries":[{"coglet":C,"operations":["speech.work_result"],"resources":[],"expires":1_000_000}],"correlation_id":"3".repeat(32)}),
            120
        ),
        0
    );
    let mut act = action();
    act["capability"] = json!("speech.work_result");
    act["args"] = json!({"turn":"6".repeat(32),"call":"7".repeat(32),"epoch":1,"status":"succeeded","value":{}});
    assert!(s.translate_action(&act, 130).is_err());
    assert_eq!(
        send(
            &mut s,
            "grants",
            json!({"epoch":2,"entries":[{"coglet":C,"operations":["speech.work_result"],"resources":["audio.mic","audio.speaker"],"expires":1_000_000}],"correlation_id":"3".repeat(32)}),
            140
        ),
        0
    );
    assert!(s.translate_action(&act, 150).is_ok());
    act["args"]["value"] = json!({"text":"x".repeat(32_001)});
    assert!(s.translate_action(&act, 150).is_err());
}
#[test]
fn byte_identical_retry_after_clock_advances() {
    let path = wal("clock");
    let setup = setup();
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let before = service
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 130)
        .unwrap();
    send(
        &mut s,
        "health",
        json!({"state":"ready","clock":50_000,"pending":1,"correlation_id":"8".repeat(32)}),
        1000,
    );
    let after = service
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 1100)
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(action()["deadline"], 20_000, "ABI intent was mutated");
    drop(service);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn reconnect_reconciles_old_request_instead_of_duplicating_act() {
    let path = wal("reconnect");
    let setup = setup();
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let id = action()["id"].as_str().unwrap().to_owned();
    let before = service.prepare_dispatch(&s, &id, 130).unwrap();
    s.disconnect();
    drop(service);
    let mut recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let reconnected = session(); // deliberately no new mapping or grants yet
    assert!(reconnected.translate_action(&action(), 2000).is_err());
    assert_eq!(
        before,
        recovered.prepare_dispatch(&reconnected, &id, 2000).unwrap()
    );
    drop(recovered);
    let store = coglet_runtime::DurableStore::open(&path).unwrap();
    assert_eq!(
        store.state().unwrap()["dispatches"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store.state().unwrap()["runtime"]["pending_actions"]
            .as_object()
            .unwrap()
            .len(),
        1
    );
    drop(store);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn outcome_requires_saved_admission_and_exact_authenticated_source() {
    let path = wal("binding");
    let setup = setup();
    let mut service = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let act = action();
    service
        .prepare_dispatch(&s, act["id"].as_str().unwrap(), 130)
        .unwrap();
    let mut event = json!({"source":S,"kind":"action.outcome","seq":1,"logical_time":1200,"payload":{"id":act["id"],"correlation_id":act["correlation_id"],"status":"succeeded","result":{}},"correlation_id":act["correlation_id"]});
    assert!(service.record_surface_outcome(&s, &event, 140).is_err());
    service
        .record_surface_accept(
            &s,
            &json!({"id":act["id"],"correlation_id":act["correlation_id"],"accepted":true}),
            140,
        )
        .unwrap();
    event["source"] =
        json!("surface:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc");
    assert!(service.record_surface_outcome(&s, &event, 150).is_err());
    event["source"] = json!(S);
    assert!(service.record_surface_outcome(&s, &event, 150).unwrap());
    event["seq"] = json!(2);
    assert!(!service.record_surface_outcome(&s, &event, 160).unwrap());
    event["payload"]["result"] = json!({"unexpected":true});
    assert!(service.record_surface_outcome(&s, &event, 170).is_err());
    assert_eq!(service.queued(), 1);
    drop(service);
    let mut recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    event["payload"]["result"] = json!({});
    event["seq"] = json!(3);
    assert!(!recovered.record_surface_outcome(&s, &event, 180).unwrap());
    let mut inputs = setup["inputs"].clone();
    inputs["logical_time"] = json!(200);
    let result = recovered
        .run_next(
            &inputs,
            &mut coglet_runtime::MockEngine::new(setup["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["boundary"].as_array().unwrap().len(), 1);
    assert_eq!(
        result["journal"]["boundary"][0]["method"],
        "runtime.receipt"
    );
    assert_eq!(result["journal"]["events"][0]["kind"], "runtime.inbox");
    assert_eq!(recovered.queued(), 0);
    drop(recovered);
    let recovered = coglet_runtime::RuntimeService::open(&path, &setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    drop(recovered);
    std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
}
#[test]
fn operational_catalog_grants_translate_and_kill_fences_new_actions() {
    let mut s = session();
    caps(&mut s);
    grants(&mut s, 1);
    let act = s.translate_action(&action(), 130).unwrap();
    assert_eq!(act["body"]["action"]["deadline"], 20_990);
    assert_eq!(act["body"]["grant_epoch"], 1);
    assert_eq!(
        send(
            &mut s,
            "kill",
            json!({"reason":"local stop","correlation_id":"6".repeat(32)}),
            140
        ),
        0
    );
    assert!(s.translate_action(&action(), 150).is_err());
    grants(&mut s, 2);
    assert!(
        s.translate_action(&action(), 150).is_err(),
        "grants cannot clear kill"
    );
}
#[test]
fn missing_mapping_rejects_act_and_invalid_sibling_payload_has_no_authority() {
    let mut s = session();
    assert!(s.translate_action(&action(), 120).is_err());
    caps(&mut s);
    grants(&mut s, 1);
    let mut bad = action();
    bad["args"] = json!({});
    assert!(s.translate_action(&bad, 130).is_err());
    s.disconnect();
    assert!(s.translate_action(&action(), 130).is_err());
}
#[test]
fn duplicate_owner_or_wrong_catalog_uri_does_not_install_grants() {
    let mut s = session();
    caps(&mut s);
    let entry = json!({"coglet":C,"operations":["expression.set_expression"],"resources":["display.face"],"expires":1_000_000});
    assert_eq!(
        send(
            &mut s,
            "grants",
            json!({"epoch":1,"entries":[entry.clone(),entry],"correlation_id":"7".repeat(32)}),
            120
        ),
        -2
    );
    assert!(s.translate_action(&action(), 130).is_err());
}
