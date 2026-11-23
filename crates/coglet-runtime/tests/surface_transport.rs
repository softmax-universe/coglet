use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
use serde_json::{Value, json};

fn binding() -> SurfaceBinding {
    SurfaceBinding::new(
        format!("surface:{}", "a".repeat(64)),
        vec![format!("coglet:{}", "b".repeat(64))],
    )
    .unwrap()
}
fn hello(surface: &str, versions: Value) -> Vec<u8> {
    cbor(&json!({"type":"hello","body":{"surface":surface,"offer":{"versions":versions},"clock":0,"correlation_id":"1".repeat(32)}})).unwrap()
}
#[test]
fn negotiated_reply_and_terminal_version_error_are_schema_valid() {
    for (versions, expected) in [
        (json!(["0.1", "0.2"]), "hello_reply"),
        (json!(["0.2"]), "error"),
    ] {
        let b = binding();
        let mut session = SurfaceSession::new(b.clone());
        assert_eq!(session.send(&hello(b.surface(), versions), 0), 0);
        let bytes = session.pop().unwrap();
        let response: Value = coglet_types::decode_profile(&bytes)
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(response["type"], expected);
        coglet_runtime::validate("surface-message", &response).unwrap();
        if expected == "error" {
            assert!(session.is_closed());
        }
    }
}
#[test]
fn hello_identity_is_not_authentication_and_failure_cannot_negotiate() {
    let b = binding();
    let mut session = SurfaceSession::new(b.clone());
    assert_eq!(
        session.send(
            &hello(&format!("surface:{}", "c".repeat(64)), json!(["0.1"])),
            0
        ),
        0
    );
    let response: Value = coglet_types::decode_profile(&session.pop().unwrap())
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(response["body"]["error"]["code"], "unauthenticated");
    assert_eq!(session.send(&hello(b.surface(), json!(["0.1"])), 0), -2);
}
#[test]
fn malformed_noncanonical_trailing_and_wrong_direction_are_fatal() {
    for bytes in [vec![0xff], vec![0xa0, 0x00], cbor(&json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":"1".repeat(32)}})).unwrap(), cbor(&json!({"type":"hello","body":{},"extra":0})).unwrap()] {
        let mut session = SurfaceSession::new(binding());
        assert_eq!(session.send(&bytes, 0), -2);
        assert!(session.is_closed());
        assert!(session.pop().is_none());
    }
}
#[test]
fn health_queue_backpressure_does_not_copy_or_advance_clock() {
    let b = binding();
    let mut session = SurfaceSession::new(b.clone());
    session.send(&hello(b.surface(), json!(["0.1"])), 0);
    session.pop();
    let health = |clock| {
        cbor(&json!({"type":"health","body":{"state":"ready","clock":clock,"pending":0,"correlation_id":"2".repeat(32)}})).unwrap()
    };
    for clock in 1..=128 {
        assert_eq!(session.send(&health(clock), clock), 0);
    }
    assert_eq!(session.send(&health(1000), 1000), 1);
    session.pop();
    assert_eq!(
        session.send(&health(129), 129),
        0,
        "blocked send must not advance peer clock"
    );
    session.pop();
    assert_eq!(
        session.send(&health(128), 130),
        -2,
        "clock rollback is fatal"
    );
}
#[test]
fn operational_input_before_hello_and_duplicate_hello_fail_closed() {
    let b = binding();
    let h = hello(b.surface(), json!(["0.1"]));
    let mut session = SurfaceSession::new(b.clone());
    let early_health = cbor(&json!({"type":"health","body":{"state":"ready","clock":0,"pending":0,"correlation_id":"2".repeat(32)}})).unwrap();
    assert_eq!(session.send(&early_health, 0), -2);
    let mut session = SurfaceSession::new(b);
    assert_eq!(session.send(&h, 0), 0);
    assert_eq!(session.send(&h, 0), -2);
    assert!(session.pop().is_none(), "fatal link clears stale replies");
}
