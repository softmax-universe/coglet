use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
use serde_json::{Value, json};
fn session() -> SurfaceSession {
    let surface = format!("surface:{}", "a".repeat(64));
    let mut session = SurfaceSession::new(SurfaceBinding::new(surface.clone(), vec![]).unwrap());
    assert_eq!(session.send(&cbor(&json!({"type":"hello","body":{"surface":surface,"clock":0,"offer":{"versions":["0.1"]},"correlation_id":"1".repeat(32)}})).unwrap(),0),0);
    session.pop();
    session
}
#[test]
fn idle_tick_emits_health_then_disconnects_at_three_seconds() {
    let mut s = session();
    s.tick(999_999);
    assert!(s.pop().is_none());
    s.tick(1_000_000);
    let h: Value = coglet_types::decode_profile(&s.pop().unwrap())
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(h["type"], "health");
    assert_eq!(h["body"]["state"], "stopped");
    s.tick(2_000_000);
    assert!(s.pop().is_some());
    s.tick(3_000_000);
    assert!(s.is_closed());
    assert!(s.pop().is_none());
}
#[test]
fn late_input_cannot_revive_expired_link_and_full_output_fails_closed() {
    let health = cbor(&json!({"type":"health","body":{"state":"ready","clock":0,"pending":0,"correlation_id":"2".repeat(32)}})).unwrap();
    let mut s = session();
    assert_eq!(s.send(&health, 3_000_000), -2);
    let mut s = session();
    for _ in 0..128 {
        assert_eq!(s.send(&health, 0), 0);
    }
    s.tick(1_000_000);
    assert!(s.is_closed());
}
