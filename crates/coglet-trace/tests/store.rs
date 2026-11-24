use coglet_runtime::{MockEngine, Runtime};
use coglet_trace::{Limits, Query, Store};
use serde_json::Value;

#[test]
fn quotas_are_atomic_deduplicated_and_query_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.sqlite");
    let bytes = serde_json::to_vec(&recording().0).unwrap();
    let limits = Limits {
        archive_bytes: bytes.len(),
        total_bytes: bytes.len() as u64,
        entries: 1,
        query_rows: 1,
    };
    let mut store = Store::open(&path, limits).unwrap();
    let id = store.ingest("one", 1, &bytes).unwrap();
    assert_eq!(store.ingest("one", 1, &bytes).unwrap(), id);
    assert!(store.ingest("two", 1, &bytes).is_err(), "entry quota");
    let mut another = bytes.clone();
    another.push(b' ');
    assert!(
        store.ingest("one", 1, &another).is_err(),
        "archive quota before parse"
    );
    assert!(
        store
            .query(Query {
                coglet: None,
                run: None,
                since: 0,
                until: 2,
                limit: 2
            })
            .is_err()
    );
    drop(store);
    let store = Store::open(&path, limits).unwrap();
    assert_eq!(
        store
            .query(Query {
                coglet: None,
                run: None,
                since: 0,
                until: 2,
                limit: 1
            })
            .unwrap()
            .len(),
        1
    );
    assert_eq!(store.fetch(&id).unwrap(), bytes);
}

#[test]
fn corrupt_archive_and_corrupt_stored_bytes_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.sqlite");
    let (trace, _) = recording();
    let mut store = Store::open(&path, Limits::default()).unwrap();
    assert!(store.ingest("r", 0, b"not json").is_err());
    let mut bad = trace.clone();
    bad["final_hash"] = serde_json::json!("00".repeat(32));
    assert!(
        store
            .ingest("r", 0, &serde_json::to_vec(&bad).unwrap())
            .is_err()
    );
    let id = store
        .ingest("r", 0, &serde_json::to_vec(&trace).unwrap())
        .unwrap();
    assert!(store.fetch("../invalid").is_err());
    assert!(store.fetch(&"f".repeat(64)).is_err());
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute(
        "UPDATE archives SET body=?1 WHERE id=?2",
        rusqlite::params![b"{}".as_slice(), id],
    )
    .unwrap();
    assert!(store.fetch(&id).is_err(), "never serve corrupted bytes");
}

#[test]
fn durable_policy_cannot_be_weakened_by_another_writer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.sqlite");
    let strict = Limits {
        entries: 1,
        ..Limits::default()
    };
    let _store = Store::open(&path, strict).unwrap();
    assert!(Store::open(&path, Limits::default()).is_err());
}

#[test]
fn concurrent_writers_share_the_total_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.sqlite");
    let bytes = serde_json::to_vec(&recording().0).unwrap();
    let limits = Limits {
        archive_bytes: bytes.len() + 1,
        total_bytes: bytes.len() as u64,
        ..Limits::default()
    };
    drop(Store::open(&path, limits).unwrap());
    let gate = std::sync::Arc::new(std::sync::Barrier::new(2));
    let handles: Vec<_> = (0..2)
        .map(|i| {
            let path = path.clone();
            let mut bytes = bytes.clone();
            let gate = gate.clone();
            if i == 1 {
                bytes.push(b' ');
            }
            std::thread::spawn(move || {
                let mut store = Store::open(path, limits).unwrap();
                gate.wait();
                store.ingest("r", 0, &bytes).is_ok()
            })
        })
        .collect();
    let successes = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .filter(|ok| *ok)
        .count();
    assert_eq!(successes, 1);
    let store = Store::open(&path, limits).unwrap();
    assert_eq!(
        store
            .query(Query {
                coglet: None,
                run: None,
                since: 0,
                until: 0,
                limit: 10
            })
            .unwrap()
            .len(),
        1
    );
    assert!(
        store
            .query(Query {
                coglet: Some("not-the-coglet"),
                run: None,
                since: 0,
                until: 0,
                limit: 10
            })
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .query(Query {
                coglet: None,
                run: Some("wrong-run"),
                since: 0,
                until: 0,
                limit: 10
            })
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .query(Query {
                coglet: None,
                run: None,
                since: 1,
                until: 2,
                limit: 10
            })
            .unwrap()
            .is_empty()
    );
}

fn recording() -> (Value, Value) {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let send = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let mut runtime = Runtime::from_fixture(&send).unwrap();
    runtime
        .step(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    (
        runtime.export_trace(&send["inputs"]["trace_id"]).unwrap(),
        send,
    )
}

#[test]
fn ingest_restart_query_fetch_independent_replay_has_identical_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("traces.sqlite");
    let (trace, send) = recording();
    let bytes = serde_json::to_vec(&trace).unwrap();
    let mut store = Store::open(&path, Limits::default()).unwrap();
    let id = store.ingest("run-1", 123, &bytes).unwrap();
    assert_eq!(store.ingest("run-1", 123, &bytes).unwrap(), id);
    drop(store);
    let store = Store::open(&path, Limits::default()).unwrap();
    let rows = store
        .query(Query {
            coglet: Some(trace["coglet"].as_str().unwrap()),
            run: Some("run-1"),
            since: 123,
            until: 123,
            limit: 10,
        })
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, id);
    let fetched: Value = serde_json::from_slice(&store.fetch(&id).unwrap()).unwrap();
    assert_eq!(fetched, trace);
    let mut baseline = Runtime::from_fixture(&send).unwrap();
    let replay = baseline
        .replay(
            fetched["records"].as_array().unwrap(),
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap();
    assert_eq!(
        replay.last().unwrap(),
        trace["records"][0]["output_hash"].as_str().unwrap()
    );
    assert_eq!(
        Runtime::import_trace(&fetched)
            .unwrap()
            .export_trace(&fetched["trace_id"])
            .unwrap()["final_hash"],
        trace["final_hash"]
    );
}
