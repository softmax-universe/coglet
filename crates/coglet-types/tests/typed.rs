//! Exercise the actual generated record, not a hand-written mirror.
use ciborium::Value;
use coglet_types::{Event, decode};
use std::collections::BTreeMap;

fn event() -> Event {
    Event {
        source: format!("surface:{}", "a".repeat(64)),
        kind: "speech.partial".into(),
        seq: u64::MAX,
        logical_time: 42,
        payload: BTreeMap::from([("n".into(), Value::Integer(i64::MIN.into()))]),
        correlation_id: "b".repeat(32),
    }
}

#[test]
fn typed_event_roundtrip() {
    let e = event();
    let raw = e.to_cbor().unwrap();
    let back = Event::from_cbor(&raw).unwrap();
    assert_eq!(back.seq, u64::MAX);
    assert_eq!(back.to_cbor().unwrap(), raw);
    let v = decode("event", &raw).unwrap();
    assert_eq!(coglet_types::encode("event", &v).unwrap(), raw);
}

#[test]
fn typed_rejects_invalid_shape() {
    let mut e = event();
    e.kind = "Not A Capability".into();
    assert!(e.to_cbor().is_err());
    let mut e = event();
    e.source = "nope".into();
    assert!(e.to_cbor().is_err());
    assert!(Event::from_cbor(&[0x01]).is_err());
}
