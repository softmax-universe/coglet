use coglet_runtime::Snapshot;
use std::collections::BTreeMap;

#[test]
fn deserialization_cannot_bypass_snapshot_path_validation() {
    let invalid = serde_json::json!({"/data/../escape":"00"});
    assert!(serde_json::from_value::<Snapshot>(invalid).is_err());
}

#[test]
fn filesystem_overlay_enforces_mounts_and_abort_isolation() {
    let base = Snapshot::new(BTreeMap::from([
        ("/code/bin/step.wasm".into(), "0061736d".into()),
        ("/data/value".into(), "31".into()),
        ("/data/inbox.jsonl".into(), "".into()),
        ("/logs/stdout".into(), "61".into()),
    ]))
    .unwrap();
    let mut step = base.clone();
    step.guest_write("/data/value", b"2").unwrap();
    assert!(step.guest_write("/code/bin/step.wasm", b"x").is_err());
    assert!(step.guest_write("/data/inbox.jsonl", b"x").is_err());
    assert!(step.guest_write("/data/runtime/heard.cbor", b"x").is_err());
    assert!(step.guest_write("/logs/events.jsonl", b"x").is_err());
    assert!(step.guest_write("/logs/stdout", b"x").is_err());
    step.guest_write("/logs/stdout", b"ab").unwrap();
    assert_eq!(base.files()["/data/value"], "31");
    assert_ne!(base.tree_hash().unwrap(), step.tree_hash().unwrap());
    for path in [
        "/data/../escape",
        "/data//x",
        "/data/./x",
        "/data/x/",
        "/data/a\\b",
        "/data/\u{7f}",
    ] {
        assert!(step.guest_write(path, b"x").is_err(), "accepted {path:?}");
    }
}
