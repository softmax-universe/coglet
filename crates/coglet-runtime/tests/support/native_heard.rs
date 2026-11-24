use super::*;
#[test]
fn ffi_out_of_order_reads_do_not_rewrite_stopped_history_or_duplicate_observations() {
    exercise_heard_order(true, "heard-reverse-ffi", false, true);
}
#[test]
fn socket_out_of_order_reads_do_not_rewrite_stopped_history_or_duplicate_observations() {
    exercise_heard_order(false, "heard-reverse-socket", false, true);
}
fn exercise_heard(ffi: bool, name: &str, revoke: bool) {
    exercise_heard_order(ffi, name, revoke, false);
}
fn exercise_heard_order(ffi: bool, name: &str, revoke: bool, reverse: bool) {
    let _guard = FFI_LOCK.lock().unwrap();
    let mut host = Host::new(ffi, name);
    drop(host.connect());
    host.stop();
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    #[cfg(feature = "native-engine")]
    {
        let wasm = wat::parse_str("(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))").unwrap();
        host.setup = native_engine_cases::with_wasm(host.setup.clone(), &wasm);
        host.setup["manifest"]["required_capabilities"] = json!(["speech.heard"]);
        host.setup["manifest"]["exclusive_outputs"] = json!(["audio.speaker"]);
        host.setup["initial_timers"] = json!([]);
        host.setup["initial_effect_ids"]["timers"] = json!([]);
    }
    host.setup["initial_actions"] = json!([{"coglet":C,"id":"4".repeat(32),"capability":"speech.speak_chunk","args":{"stream":"a".repeat(32),"chunk":0,"text":"héllo unheard proposal","final":true},"deadline":50_000_000,"correlation_id":"5".repeat(32)}]);
    host.setup["initial_effect_ids"]["actions"] = json!(["4".repeat(32)]);
    host.setup["manifest"]["subscriptions"] = json!(["speech.position"]);
    std::fs::write(
        host.dir.join("setup.json"),
        serde_json::to_vec(&host.setup).unwrap(),
    )
    .unwrap();
    host.start();
    let mut link = host.connect();
    link.hello();
    link.send("capabilities",json!({"revision":1,"clock":1100,"entries":[
        {"name":"speech.control","events":["speech.position"],"reads":[],"actions":["speech.speak_chunk"],"exclusive":["audio.speaker"],"schemas":{"speech.position":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-position","speech.speak_chunk":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk"}},
        {"name":"speech.history","events":[],"reads":["speech.heard"],"actions":[],"exclusive":[],"schemas":{"speech.heard":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/heard-record"}}
    ],"correlation_id":"2".repeat(32)}));
    link.send("grants",json!({"epoch":1,"entries":[{"coglet":C,"operations":["speech.position","speech.heard","speech.speak_chunk"],"resources":["audio.speaker"],"expires":100_000_000}],"correlation_id":"3".repeat(32)}));
    assert_eq!(link.recv_until("act")["type"], "act");
    link.send(
        "accept",
        json!({"id":"4".repeat(32),"correlation_id":"5".repeat(32),"accepted":true}),
    );
    let mut delayed = Vec::new();
    for (index, (played, text)) in [(10, "hé"), (20, "revision"), (30, "héllo")]
        .into_iter()
        .enumerate()
    {
        link.send("event",json!({"source":S,"seq":index+1,"logical_time":1200+index,"kind":"speech.position","payload":{"stream":"a".repeat(32),"chunk":0,"item":"4".repeat(32),"played_ms":played,"position_ms":played,"state":if index==2 {"stopped"} else {"playing"}},"correlation_id":"6".repeat(32)}));
        let read = link.recv_until("read");
        assert_eq!(
            read["type"], "read",
            "native route did not initiate owner read"
        );
        assert_eq!(read["body"]["capability"], "speech.heard");
        assert_eq!(read["body"]["args"], json!({"item":"4".repeat(32)}));
        if revoke {
            link.send(
                "grants",
                json!({"epoch":2,"entries":[],"correlation_id":"7".repeat(32)}),
            );
        }
        let reply = json!({"correlation_id":read["body"]["correlation_id"],"ok":true,"value":{"item":"4".repeat(32),"played_ms":if index < 2 {played+2} else {played},"text":text},"snapshot":index+1});
        if reverse {
            delayed.push(reply);
        } else {
            link.send("read_reply", reply);
        }
        if revoke {
            break;
        }
    }
    for reply in delayed.into_iter().rev() {
        link.send("read_reply", reply);
    }
    if reverse {
        link.send("event",json!({"source":S,"seq":4,"logical_time":1300,"kind":"speech.position","payload":{"stream":"a".repeat(32),"chunk":0,"item":"4".repeat(32),"played_ms":30,"position_ms":30,"state":"stopped"},"correlation_id":"6".repeat(32)}));
    }
    link.send(
        "health",
        json!({"state":"ready","clock":1500,"pending":1,"correlation_id":"9".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["type"], "health");
    drop(link);
    host.stop();
    if revoke {
        let store = coglet_runtime::DurableStore::open(&host.dir.join("state.wal")).unwrap();
        assert!(store.state().unwrap()["staged_boundary"].is_null());
        assert!(
            store.state().unwrap()["heard_pending"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        assert!(
            store.state().unwrap()["private_events"]
                .as_object()
                .unwrap()
                .is_empty()
        );
        return;
    }
    let mut service =
        coglet_runtime::RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128)
            .unwrap();
    let changes: Vec<Value> = if service.records().is_empty() {
        service
            .seal_boundary(&host.setup["inputs"]["trace_id"], 1_000_000, &[])
            .unwrap()["boundary"]
            .as_array()
            .unwrap()
            .clone()
    } else {
        assert_eq!(service.records().len(), if reverse { 1 } else { 3 });
        assert!(
            service
                .records()
                .iter()
                .all(
                    |r| r["events"].as_array().unwrap().len() == if reverse { 3 } else { 1 }
                        && r["events"][0]["kind"] == "speech.position"
                )
        );
        service
            .records()
            .iter()
            .flat_map(|r| r["boundary"].as_array().unwrap().iter().cloned())
            .collect()
    };
    assert_eq!(
        changes.len(),
        if revoke {
            0
        } else if reverse {
            1
        } else {
            3
        }
    );
    assert!(
        changes
            .iter()
            .all(|v| v["method"] == "runtime.heard" && v["path"] == "/data/runtime/heard.cbor")
    );
    if !revoke {
        let raw = hex::decode(changes.last().unwrap()["after"].as_str().unwrap()).unwrap();
        let heard: Value = coglet_types::decode("heard-state", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(heard["items"][0]["heard"], "héllo");
        assert_eq!(
            heard["items"][0]["spans"],
            if reverse {
                json!([{"end_ms":30,"text":"héllo"}])
            } else {
                json!([{"end_ms":12,"text":"hé"},{"end_ms":30,"text":"llo"}])
            }
        );
        assert_eq!(heard["items"][0]["stopped"], true);
    }
    assert_eq!(
        service.queued(),
        0,
        "close revived observations or heard queued a synthetic wake"
    );
}
fn session_with_heard(
    service: std::sync::Arc<Mutex<coglet_runtime::RuntimeService>>,
) -> coglet_runtime::SurfaceSession {
    let mut s = coglet_runtime::SurfaceSession::with_service(
        coglet_runtime::SurfaceBinding::new(S.into(), vec![C.into()]).unwrap(),
        service,
    )
    .unwrap();
    for (kind, body) in [
        (
            "hello",
            json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
        ),
        (
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[{"name":"speech.control","events":["speech.position"],"reads":[],"actions":["speech.speak_chunk"],"exclusive":["audio.speaker"],"schemas":{"speech.position":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-position","speech.speak_chunk":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk"}},{"name":"speech.history","events":[],"reads":["speech.heard"],"actions":[],"exclusive":[],"schemas":{"speech.heard":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/heard-record"}}],"correlation_id":"2".repeat(32)}),
        ),
        (
            "grants",
            json!({"epoch":1,"entries":[{"coglet":C,"operations":["speech.position","speech.heard","speech.speak_chunk"],"resources":["audio.speaker"],"expires":2000}],"correlation_id":"3".repeat(32)}),
        ),
    ] {
        assert_eq!(
            s.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), 100),
            0
        );
    }
    s.pop();
    s
}
#[test]
fn heard_barrier_blocks_reasoning_then_expires_without_a_reply_or_transport_tick() {
    let dir = std::env::temp_dir().join(format!("heard-barrier-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut setup = setup();
    setup["manifest"]["subscriptions"] = json!(["speech.position"]);
    setup["initial_actions"] = json!([{"coglet":C,"id":"4".repeat(32),"capability":"speech.speak_chunk","args":{"stream":"a".repeat(32),"chunk":0,"text":"proposal","final":true},"deadline":5000,"correlation_id":"5".repeat(32)}]);
    setup["initial_effect_ids"]["actions"] = json!(["4".repeat(32)]);
    let service = std::sync::Arc::new(Mutex::new(
        coglet_runtime::RuntimeService::open(&dir.join("state.wal"), &setup, 128).unwrap(),
    ));
    let mut session = session_with_heard(service.clone());
    session.pump_dispatch(110).unwrap();
    session.pop();
    let accept = json!({"type":"accept","body":{"id":"4".repeat(32),"correlation_id":"5".repeat(32),"accepted":true}});
    assert_eq!(session.send(&cbor(&accept).unwrap(), 120), 0);
    let event = json!({"type":"event","body":{"source":S,"seq":1,"logical_time":1200,"kind":"speech.position","payload":{"stream":"a".repeat(32),"chunk":0,"item":"4".repeat(32),"played_ms":10,"position_ms":10,"state":"playing"},"correlation_id":"6".repeat(32)}});
    assert_eq!(session.send(&cbor(&event).unwrap(), 130), 0);
    assert_eq!(decode(&session.pop().unwrap())["type"], "read");
    let mut context = setup["inputs"].clone();
    context["logical_time"] = json!(140);
    let mut engine = coglet_runtime::MockEngine::new(json!({"operations":[]}));
    assert!(
        service
            .lock()
            .unwrap()
            .run_next(&context, &mut engine)
            .unwrap()
            .is_none()
    );
    assert_eq!(service.lock().unwrap().queued(), 1);
    // Runtime expiry must not depend on polling the session that owns the read.
    context["logical_time"] = json!(2000);
    assert!(
        service
            .lock()
            .unwrap()
            .run_next(&context, &mut engine)
            .unwrap()
            .is_none()
    );
    assert_eq!(service.lock().unwrap().queued(), 0);
    let mut later = event["body"].clone();
    later["seq"] = json!(2);
    later["logical_time"] = json!(2200);
    service.lock().unwrap().enqueue(later).unwrap();
    context["logical_time"] = json!(2100);
    let mut ready_engine = coglet_runtime::MockEngine::new(setup["guest_script"].clone());
    assert!(
        service
            .lock()
            .unwrap()
            .run_next(&context, &mut ready_engine)
            .unwrap()
            .is_some(),
        "expired heard barrier stranded unrelated admitted input"
    );
    drop(session);
    drop(service);
    let store = coglet_runtime::DurableStore::open(&dir.join("state.wal")).unwrap();
    assert!(
        store.state().unwrap()["heard_pending"]
            .as_object()
            .unwrap()
            .is_empty()
    );
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn ffi_owner_heard_reads_materialize_before_reasoning() {
    exercise_heard(true, "ffi-heard", false);
}
#[test]
fn socket_owner_heard_reads_materialize_before_reasoning() {
    exercise_heard(false, "socket-heard", false);
}
#[test]
fn ffi_revoked_heard_reply_never_enters_archive() {
    exercise_heard(true, "ffi-heard-revoke", true);
}
#[test]
fn socket_revoked_heard_reply_never_enters_archive() {
    exercise_heard(false, "socket-heard-revoke", true);
}
