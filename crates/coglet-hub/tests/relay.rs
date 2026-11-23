use coglet_hub::Store;
use serde_json::json;

#[test]
fn batches_dedupe_before_watermarks_and_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("hub.db");
    let event = json!({"source":"surface:abc","seq":2,"logical_time":7,"type":"example.tick","correlation_id":"11".repeat(16),"payload":{"n":1}});
    {
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .admit_events("device-a", "coglet-a", 0, &[event.clone()])
                .unwrap(),
            1
        );
        let mut later = event.clone();
        later["seq"] = json!(7);
        assert_eq!(
            store
                .admit_events("device-a", "coglet-a", 1, &[later])
                .unwrap(),
            2
        );
    }
    let mut store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .admit_events("device-a", "coglet-a", 0, &[event.clone()])
            .unwrap(),
        1
    );
    assert_eq!(store.pending_events("coglet-a").unwrap().len(), 2);
    assert_eq!(store.audit().unwrap().len(), 2);
    assert_eq!(
        store.admit_events("device-a", "coglet-a", 2, &[]).unwrap(),
        2
    );
    let mut changed = event.clone();
    changed["seq"] = json!(3);
    assert!(
        store
            .admit_events("device-a", "coglet-a", 0, &[changed])
            .is_err()
    );
    assert!(
        store
            .admit_events("device-a", "coglet-a", 3, &[event])
            .is_err()
    );
    assert_eq!(store.audit().unwrap().len(), 2);
}
