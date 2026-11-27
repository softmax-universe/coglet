use super::*;
fn exercise_playback(ffi: bool, name: &str, provider: bool) {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut host = Host::new(ffi, name);
    drop(host.connect());
    host.stop();
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    let item = "4".repeat(32);
    let stream = "a".repeat(32);
    let session = "b".repeat(32);
    let capability = if provider {
        "speech.session_open"
    } else {
        "speech.speak_chunk"
    };
    let args = if provider {
        json!({"session":session,"route":"test"})
    } else {
        json!({"stream":stream,"chunk":0,"text":"proposal","final":true})
    };
    host.setup["initial_actions"] = json!([{"coglet":C,"id":"4".repeat(32),"capability":capability,"args":args,"deadline":50_000_000,"correlation_id":"5".repeat(32)}]);
    host.setup["initial_effect_ids"]["actions"] = json!([item]);
    host.setup["manifest"]["subscriptions"] = json!([
        "speech.start",
        "speech.end",
        "speech.position",
        "speech.barge_in",
        "speech.tool_call"
    ]);
    std::fs::write(
        host.dir.join("setup.json"),
        serde_json::to_vec(&host.setup).unwrap(),
    )
    .unwrap();
    host.start();
    let mut l = host.connect();
    l.hello();
    let mut schemas = json!({"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start","speech.end":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-end","speech.position":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-position","speech.barge_in":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-barge-in"});
    let mut entries = vec![];
    let mut actions = vec![];
    if !provider {
        actions.push("speech.speak_chunk");
        schemas["speech.speak_chunk"] =
            json!("https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk");
    }
    entries.push(json!({"name":"speech.control","events":["speech.start","speech.end","speech.position","speech.barge_in"],"reads":[],"actions":actions,"exclusive":["audio.mic","audio.speaker"],"schemas":schemas}));
    if provider {
        entries.push(json!({"name":"speech.session","events":["speech.tool_call"],"reads":[],"actions":["speech.session_open"],"exclusive":[],"schemas":{"speech.tool_call":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/tool-call","speech.session_open":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/session-open"}}));
        entries.push(json!({"name":"speech.history","events":[],"reads":["speech.heard"],"actions":[],"exclusive":[],"schemas":{"speech.heard":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/heard-record"}}));
    }
    l.send(
        "capabilities",
        json!({"revision":1,"clock":1100,"entries":entries,"correlation_id":"2".repeat(32)}),
    );
    let mut operations = vec![
        "speech.start",
        "speech.end",
        "speech.position",
        "speech.barge_in",
        capability,
    ];
    if provider {
        operations.push("speech.tool_call");
        operations.push("speech.heard");
    }
    l.send("grants",json!({"epoch":1,"entries":[{"coglet":C,"operations":operations,"resources":["audio.mic","audio.speaker"],"expires":100_000_000}],"correlation_id":"3".repeat(32)}));
    assert_eq!(l.recv_until("act")["type"], "act");
    l.send(
        "accept",
        json!({"id":item,"correlation_id":"5".repeat(32),"accepted":true}),
    );
    let mut seq = 0;
    if provider {
        seq += 1;
        l.send("event",json!({"source":S,"seq":seq,"logical_time":1200,"kind":"action.outcome","payload":{"id":item,"correlation_id":"5".repeat(32),"status":"succeeded","result":{}},"correlation_id":"5".repeat(32)}));
    }
    let item = if provider { "8".repeat(32) } else { item };
    let mut events = vec![
        (
            "speech.position",
            json!({"stream":stream,"chunk":0,"item":item,"played_ms":10,"position_ms":10,"state":"playing"}),
        ),
        (
            "speech.start",
            json!({"utterance":"c".repeat(32),"position_ms":0}),
        ),
        (
            "speech.barge_in",
            json!({"utterance":"c".repeat(32),"stream":stream,"item":item,"played_ms":10,"position_ms":10,"reflex":"ducked"}),
        ),
        (
            "speech.position",
            json!({"stream":stream,"chunk":0,"item":item,"played_ms":10,"position_ms":10,"state":"ducked"}),
        ),
        (
            "speech.position",
            json!({"stream":stream,"chunk":0,"item":item,"played_ms":20,"position_ms":20,"state":"playing"}),
        ),
        (
            "speech.end",
            json!({"utterance":"c".repeat(32),"revision":1,"text":"user","position_ms":30}),
        ),
    ];
    // Historical barge redelivery after capture ended/progress advanced must not
    // revive an interrupted snapshot or wake again.
    events.push(events[2].clone());
    if provider {
        let call =
            json!({"session":session,"call":"d".repeat(32),"name":"test.tool","arguments":{}});
        events.push(("speech.tool_call", call.clone()));
        events.push(("speech.tool_call", call));
    }
    events.push(("speech.position",json!({"stream":stream,"chunk":0,"item":item,"played_ms":30,"position_ms":30,"state":"stopped"})));
    for (index, (kind, payload)) in events.into_iter().enumerate() {
        seq += 1;
        l.send("event",json!({"source":S,"seq":seq,"logical_time":1200+seq,"kind":kind,"payload":payload,"correlation_id":"6".repeat(32)}));
        if provider && matches!(kind, "speech.position" | "speech.barge_in") && index != 6 {
            let read = l.recv_until("read");
            assert_eq!(read["body"]["args"]["item"], item);
            l.send("read_reply",json!({"correlation_id":read["body"]["correlation_id"],"ok":true,"snapshot":seq,"value":{"item":item,"played_ms":payload["played_ms"],"text":"heard"}}));
        }
    }
    l.health_barrier(&"9".repeat(32));
    if !ffi {
        host.stop();
    }
    drop(l);
    if ffi {
        host.stop();
    }
    let store = coglet_runtime::DurableStore::open(&host.dir.join("state.wal")).unwrap();
    let state = store.state().unwrap();
    let playback = &state["speech_temporal"]["playback"];
    let key = format!("{S}/{item}");
    assert_eq!(playback["items"][&key]["played"], 30);
    assert_eq!(playback["items"][&key]["state"], "stopped");
    assert_eq!(playback["barges"].as_object().unwrap().len(), 1);
    assert_eq!(
        playback["items"][&key]["provider"],
        if provider {
            json!(session)
        } else {
            Value::Null
        }
    );
    assert_eq!(
        state["speech_temporal"]["repeated_calls"],
        if provider { 1 } else { 0 }
    );
    if ffi {
        assert!(state["private_events"].as_object().unwrap().is_empty());
    } else {
        assert_eq!(
            state["private_events"].as_object().unwrap().len(),
            if provider { 8 } else { 7 }
        );
    }
    if provider {
        let raw = hex::decode(
            state["staged_boundary"]["changes"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["after"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let heard: Value = coglet_types::decode("heard-state", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(heard["items"][0]["heard"], "heard");
        assert_eq!(heard["items"][0]["played_ms"], 30);
        assert_eq!(heard["items"][0]["stopped"], true);
    }
    drop(store);
    // Independently reopen through production validation, not just raw WAL JSON.
    coglet_runtime::RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
    let store = coglet_runtime::DurableStore::open(&host.dir.join("state.wal")).unwrap();
    assert!(
        store.state().unwrap()["private_events"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}
#[test]
fn ffi_playback_duck_pair_and_terminal_binding() {
    exercise_playback(true, "playback-ffi", false);
}
#[test]
fn socket_playback_duck_pair_and_terminal_binding() {
    exercise_playback(false, "playback-socket", false);
}
#[test]
fn ffi_provider_native_item_and_call_owner() {
    exercise_playback(true, "provider-ffi", true);
}
#[test]
fn socket_provider_native_item_and_call_owner() {
    exercise_playback(false, "provider-socket", true);
}
