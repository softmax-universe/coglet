use super::*;

pub(super) fn runtime_grant_negatives() {
    let setup = setup(&wasm());
    for mode in [
        "missing",
        "wrong_actor",
        "expired",
        "revoked",
        "valid",
        "unknown",
        "schema",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let service = Arc::new(Mutex::new(
            RuntimeService::open_echo_fixture(&dir.path().join("wal"), &setup, 128).unwrap(),
        ));
        let mut session = SurfaceSession::with_echo_fixture(binding(), service.clone()).unwrap();
        let send = |s: &mut SurfaceSession, kind: &str, body: Value, now| {
            s.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now)
        };
        assert_eq!(
            send(
                &mut session,
                "hello",
                json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"11".repeat(16)}),
                0
            ),
            0
        );
        let op = if mode == "unknown" {
            "test.other"
        } else {
            "test.echo"
        };
        let schema = if mode == "schema" {
            "https://attacker.invalid/map"
        } else {
            "https://coglet.dev/spec/messages/common.schema.json#/$defs/profile-map"
        };
        let caps = json!({"revision":1,"clock":1000,"entries":[{"name":"test.echo","events":[op],"reads":[],"actions":[op],"exclusive":[],"schemas":{op:schema}}],"correlation_id":"22".repeat(16)});
        let status = send(&mut session, "capabilities", caps, 0);
        if matches!(mode, "unknown" | "schema") {
            assert_eq!(status, -2);
            continue;
        }
        assert_eq!(status, 0);
        let coglet = if mode == "wrong_actor" {
            format!("coglet:{}", "c".repeat(64))
        } else {
            C.into()
        };
        if mode != "missing" {
            let expires = if mode == "expired" { 1000 } else { 1_000_000 };
            let status = send(
                &mut session,
                "grants",
                json!({"epoch":1,"entries":[{"coglet":coglet,"operations":["test.echo"],"resources":[],"expires":expires}],"correlation_id":"33".repeat(16)}),
                0,
            );
            if matches!(mode, "wrong_actor" | "expired") {
                assert_eq!(status, -2);
                continue;
            }
            assert_eq!(status, 0);
        }
        if mode == "revoked" {
            assert_eq!(
                send(
                    &mut session,
                    "grants",
                    json!({"epoch":2,"entries":[],"correlation_id":"44".repeat(16)}),
                    0
                ),
                0
            );
        }
        let action = json!({"id":"55".repeat(16),"coglet":C,"capability":"test.echo","args":{"x":1},"deadline":1000,"correlation_id":"66".repeat(16)});
        assert_eq!(
            session.translate_action(&action, 0).is_ok(),
            mode == "valid",
            "{mode}"
        );
        if mode == "valid" {
            let mut wrong = action.clone();
            wrong["coglet"] = json!(format!("coglet:{}", "c".repeat(64)));
            assert!(session.translate_action(&wrong, 0).is_err());
            wrong = action;
            wrong["args"] = json!([]);
            assert!(session.translate_action(&wrong, 0).is_err());
            let expired = json!({"id":"55".repeat(16),"coglet":C,"capability":"test.echo","args":{},"deadline":2_000_000,"correlation_id":"66".repeat(16)});
            assert!(
                session.translate_action(&expired, 1_000_000).is_err(),
                "grant expiry must gate action"
            );
        }
        if matches!(mode, "missing" | "revoked") {
            let event = json!({"source":S,"kind":"test.echo","seq":1,"logical_time":1,"payload":{},"correlation_id":"77".repeat(16)});
            assert_eq!(send(&mut session, "event", event, 0), -2);
            assert_eq!(service.lock().unwrap().queued(), 0);
        }
    }
}
