use super::*;
fn dispatch(id: &str, session: &str, epoch: u64, operation: &str) -> Dispatch {
    let c = format!("coglet:{}", "a".repeat(64));
    let intent = json!({"coglet":c,"id":id,"capability":operation,"args":{"session":session,"route":"test"},"deadline":5000,"correlation_id":id});
    Dispatch {
        intent: intent.clone(),
        surface: format!("surface:{}", "b".repeat(64)),
        envelope_hex: hex::encode(
            cbor(&json!({"type":"act","body":{"grant_epoch":epoch,"action":intent}})).unwrap(),
        ),
        prepared_at: 0,
        terminal_at: Some(10),
        accept: Some(json!({"accepted":true})),
        outcome: Some(json!({"status":"succeeded"})),
        stopped: false,
    }
}
#[test]
fn native_provider_item_requires_unambiguous_historical_session_and_current_epoch() {
    let dir = std::env::temp_dir().join(format!("provider-owner-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let owner = format!("coglet:{}", "a".repeat(64));
    let surface = format!("surface:{}", "b".repeat(64));
    setup["manifest"]["id"] = json!(owner);
    let mut service = RuntimeService::open(&dir.join("wal"), &setup, 128).unwrap();
    let mut s = SurfaceSession::new(
        crate::SurfaceBinding::new(surface.clone(), vec![owner.clone()]).unwrap(),
    );
    for (kind, body) in [
        (
            "hello",
            json!({"surface":surface,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
        ),
        (
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[],"correlation_id":"2".repeat(32)}),
        ),
        (
            "grants",
            json!({"epoch":1,"entries":[],"correlation_id":"3".repeat(32)}),
        ),
    ] {
        assert_eq!(
            s.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), 100),
            0
        );
    }
    let payload = json!({"item":"8".repeat(32),"stream":"9".repeat(32),"chunk":0,"played_ms":0,"position_ms":0,"state":"playing"});
    assert!(service.playback_owner(&s, &owner, &payload).is_err());
    service.state.dispatches.insert(
        "4".repeat(32),
        dispatch(&"4".repeat(32), &"b".repeat(32), 1, "speech.session_open"),
    );
    assert_eq!(
        service.playback_owner(&s, &owner, &payload).unwrap(),
        Some("b".repeat(32))
    );
    service.state.dispatches.insert(
        "5".repeat(32),
        dispatch(&"5".repeat(32), &"c".repeat(32), 1, "speech.session_open"),
    );
    assert!(
        service.playback_owner(&s, &owner, &payload).is_err(),
        "late unseen callback was assigned to a new session"
    );
    service.state.dispatches.remove(&"5".repeat(32));
    service.state.dispatches.insert(
        "6".repeat(32),
        dispatch(&"6".repeat(32), &"b".repeat(32), 1, "speech.session_close"),
    );
    assert!(service.playback_owner(&s, &owner, &payload).is_err());
    service.state.dispatches.remove(&"6".repeat(32));
    service.state.dispatches.insert(
        "4".repeat(32),
        dispatch(&"4".repeat(32), &"b".repeat(32), 2, "speech.session_open"),
    );
    assert!(
        service.playback_owner(&s, &owner, &payload).is_err(),
        "old epoch provider acquired current item"
    );
    drop(service);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn tool_result_pending_or_terminal_dispatch_fences_new_ids_but_rejection_does_not() {
    let mut temporal = speech_temporal::SpeechTemporal::default();
    let surface = format!("surface:{}", "b".repeat(64));
    let args = json!({"session":"b".repeat(32),"call":"c".repeat(32),"ok":true,"value":{}});
    let mut ds = BTreeMap::new();
    assert!(temporal.check_tool_result(&surface, &args, &ds).is_err());
    temporal.admit(&surface,"owner",1,&json!({"kind":"speech.tool_call","payload":{"session":args["session"],"call":args["call"],"name":"test.tool","arguments":{}}})).unwrap();
    temporal.check_tool_result(&surface, &args, &ds).unwrap();
    let mut d = dispatch(&"4".repeat(32), &"b".repeat(32), 1, "speech.tool_result");
    d.intent["args"] = args.clone();
    d.accept = None;
    d.outcome = None;
    ds.insert("4".repeat(32), d.clone());
    assert!(temporal.check_tool_result(&surface, &args, &ds).is_err());
    d.accept = Some(json!({"accepted":true}));
    ds.insert("4".repeat(32), d.clone());
    assert!(temporal.check_tool_result(&surface, &args, &ds).is_err());
    d.accept = Some(json!({"accepted":false}));
    ds.insert("4".repeat(32), d);
    temporal.check_tool_result(&surface, &args, &ds).unwrap();
}
