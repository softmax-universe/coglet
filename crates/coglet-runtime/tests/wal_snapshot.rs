//! Test-only synchronization before observing an unchanged lifecycle WAL.
#[path = "support/wal_snapshot.rs"]
mod wal_snapshot;

use coglet_runtime::DurableStore;
use serde_json::{Value, json};

fn snapshot(state: Value) -> Vec<u8> {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.wal");
    let mut store = DurableStore::open(&path).unwrap();
    store.commit(&state, None).unwrap();
    drop(store);
    std::fs::read(path).unwrap()
}

#[test]
fn wal_snapshot_requires_complete_validated_cleared_fence() {
    for state in [
        json!({}),
        json!({"managed": {}}),
        json!({"managed":{"code_pending":null}}),
        json!({"managed":{"code_pending":"false"}}),
        json!({"managed":{"code_pending":true}}),
    ] {
        assert!(wal_snapshot::validated_unfenced_snapshot(snapshot(state)).is_none());
    }
    let settled = snapshot(json!({"managed":{"code_pending":false}}));
    assert_eq!(
        wal_snapshot::validated_unfenced_snapshot(settled.clone()),
        Some(settled.clone())
    );
    let mut partial = settled.clone();
    partial.extend_from_slice(&[0, 0]);
    assert!(wal_snapshot::validated_unfenced_snapshot(partial).is_none());
    let mut partial = settled.clone();
    partial.extend_from_slice(&[0, 0, 0, 10, 0]);
    assert!(wal_snapshot::validated_unfenced_snapshot(partial).is_none());
    let mut corrupt = settled;
    *corrupt.last_mut().unwrap() ^= 1;
    assert!(wal_snapshot::validated_unfenced_snapshot(corrupt).is_none());
}

#[tokio::test]
async fn wal_snapshot_wait_is_bounded_and_returns_exact_validated_bytes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("state.wal");
    let mut store = DurableStore::open(&path).unwrap();
    store
        .commit(&json!({"managed":{"code_pending":true}}), None)
        .unwrap();
    assert!(
        wal_snapshot::wait_unfenced_wal(&path, std::time::Duration::from_millis(30))
            .await
            .is_err()
    );
    store
        .commit(&json!({"managed":{"code_pending":false}}), None)
        .unwrap();
    let expected = std::fs::read(&path).unwrap();
    assert_eq!(
        wal_snapshot::wait_unfenced_wal(&path, std::time::Duration::from_secs(1))
            .await
            .unwrap(),
        expected
    );
}
