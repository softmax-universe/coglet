use coglet_hub::Store;
use serde_json::json;

#[test]
fn admission_rolls_back_entire_batch_and_preserves_audit() {
    let mut store = Store::open(":memory:").unwrap();
    let e = json!({"source":"s","seq":2});
    assert!(
        store
            .admit_events("d", "c", 0, &[e.clone(), e.clone()])
            .is_err()
    );
    assert!(store.pending_events("c").unwrap().is_empty());
    assert!(store.audit().unwrap().is_empty());
    assert_eq!(store.admit_events("d", "c", 0, &[e]).unwrap(), 1);
}

#[test]
fn mapped_requests_are_immutable_across_restart_and_never_retranslated() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let request = json!({"method":"hub.actions","target":"c","correlation_id":"op","params":{"action":{"id":"action-1","deadline":50}}});
    {
        let mut store = Store::open(&path).unwrap();
        store
            .retain_action("device-b", "c", "action-1", &request)
            .unwrap();
        store
            .retain_action("device-b", "c", "action-1", &request)
            .unwrap();
        let mut changed = request.clone();
        changed["params"]["action"]["deadline"] = json!(51);
        assert!(
            store
                .retain_action("device-b", "c", "action-1", &changed)
                .is_err()
        );
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.action("c", "action-1").unwrap(),
        Some(("device-b".into(), request))
    );
    assert_eq!(store.audit().unwrap().len(), 1);
    let db = rusqlite::Connection::open(path).unwrap();
    assert!(db.execute("DELETE FROM audit", []).is_err());
    assert!(db.execute("UPDATE audit SET record='{}'", []).is_err());
}
