use super::*;
use crate::phase_evidence::Phase;

#[test]
fn phase_wal_success_and_real_write_failure_preserve_order_and_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("phase.wal");
    let mut store = DurableStore::open(&path).unwrap();
    assert!(store.phase_sink().is_none());
    store.enable_phase_sink();
    let sink = store.phase_sink().unwrap();
    store.enable_phase_sink();
    assert!(std::sync::Arc::ptr_eq(&sink, &store.phase_sink().unwrap()));
    store.commit(&json!({"one":1}), None).unwrap();
    let snap = sink.snapshot();
    let phases: Vec<_> = snap.entries[..snap.count].iter().map(|e| e.phase).collect();
    assert_eq!(
        phases,
        vec![
            Phase::EncodeEnter,
            Phase::EncodeReturn,
            Phase::WalWriteEnter,
            Phase::WalWriteDone,
            Phase::WalSyncEnter,
            Phase::WalSyncDone,
            Phase::CacheEnter,
            Phase::CacheReturn
        ],
        "missing WAL phase evidence"
    );
    let previous = store.previous.clone();
    let state = store.state.clone();
    // Replacing our private file only AFTER the ordinary handle is dropped:
    // deliberately read-only descriptor yields an actual deterministic EBADF.
    store.file = LockedFile(OpenOptions::new().read(true).open(&path).unwrap());
    let error = store.commit(&json!({"two":2}), None).unwrap_err();
    assert_eq!(
        error,
        std::io::Error::from_raw_os_error(libc::EBADF).to_string()
    );
    assert!(store.poisoned);
    assert_eq!(store.previous, previous);
    assert_eq!(store.state, state);
    assert!(store.records().is_empty());
    let snap = sink.snapshot();
    let phases: Vec<_> = snap.entries[..snap.count].iter().map(|e| e.phase).collect();
    assert_eq!(
        &phases[8..],
        &[
            Phase::EncodeEnter,
            Phase::EncodeReturn,
            Phase::WalWriteEnter
        ]
    );
}
