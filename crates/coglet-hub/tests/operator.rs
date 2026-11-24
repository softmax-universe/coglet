use coglet_hub::{
    Store,
    operator::{PeerIdentity, Request},
};
use serde_json::{Value, json};

fn request(id: &str, timeout: u64) -> Request {
    Request::parse(
        &serde_json::to_vec(&json!({
            "version":"local-1", "operation_id":id, "op":"status", "timeout_ms":timeout,
            "params":{"device":"11".repeat(32),"target":format!("coglet:{}", "22".repeat(32))}
        }))
        .unwrap(),
    )
    .unwrap()
}
fn peer(uid: u32) -> PeerIdentity {
    PeerIdentity {
        uid,
        gid: Some(9),
        pid: Some(42),
    }
}

#[test]
fn operator_frame_limits() {
    let good = request(&"ab".repeat(16), 100).value().clone();
    let mut bad = vec![
        Vec::new(),
        vec![b' '; 1048577],
        b"{} {}".to_vec(),
        vec![255],
    ];
    for (pointer, value) in [
        ("/extra", json!(true)),
        ("/params/actor", json!("coglet:spoof")),
        ("/operation_id", json!("AB".repeat(16))),
        ("/timeout_ms", json!(0)),
        ("/timeout_ms", json!(300001)),
        ("/timeout_ms", json!(1.5)),
        ("/params/device", json!("00")),
        ("/params/target", json!("echo")),
        ("/version", json!("0.1")),
        ("/op", json!("hub.start")),
    ] {
        let mut v = good.clone();
        if pointer == "/extra" {
            v["extra"] = value;
        } else if pointer == "/params/actor" {
            v["params"]["actor"] = value;
        } else {
            *v.pointer_mut(pointer).unwrap() = value;
        }
        bad.push(serde_json::to_vec(&v).unwrap());
    }
    let text = serde_json::to_string(&good).unwrap();
    bad.push(
        text.replacen("\"version\":", "\"version\":\"local-1\",\"version\":", 1)
            .into_bytes(),
    );
    bad.push(
        text.replacen("\"device\":", "\"device\":\"11\",\"device\":", 1)
            .into_bytes(),
    );
    for bytes in bad {
        assert!(
            Request::parse(&bytes).is_err(),
            "accepted {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }
}

#[test]
fn operator_operation_contracts() {
    let good = request(&"ac".repeat(16), 100).value().clone();
    let node = "11".repeat(32);
    let target = format!("coglet:{}", "22".repeat(32));
    let grant = json!({"peer":node,"method":"hub.status","target":target,"generation":1,"expires_unix_s":1234});
    for (op, params) in [
        ("stop", good["params"].clone()),
        (
            "push",
            json!({"device":node,"target":target,"source_commit":"aa".repeat(20),"build_profile":"trusted-v1","delegation_id":"echo"}),
        ),
        (
            "deploy",
            json!({"device":node,"target":target,"source_commit":"aa".repeat(20),"build_profile":"trusted-v1","delegation_id":"echo"}),
        ),
        ("grant.provision", grant.clone()),
        (
            "grant.revoke",
            json!({"peer":node,"method":"hub.status","target":target,"generation":2}),
        ),
        ("enrollment.window", json!({"duration_ms":300000})),
        (
            "enrollment.invite",
            json!({"window_id":"bb".repeat(16),"expected_device":node,"secret_sha256":"cc".repeat(32),"ttl_ms":10,"grants":[grant]}),
        ),
    ] {
        let mut v = good.clone();
        v["op"] = json!(op);
        v["params"] = params;
        assert!(
            Request::parse(&serde_json::to_vec(&v).unwrap()).is_ok(),
            "refused {op}"
        );
        let keys: Vec<_> = v["params"].as_object().unwrap().keys().cloned().collect();
        for k in keys {
            let mut bad = v.clone();
            bad["params"].as_object_mut().unwrap().remove(&k);
            assert!(
                Request::parse(&serde_json::to_vec(&bad).unwrap()).is_err(),
                "missing {op}/{k}"
            );
        }
        v["params"]["unknown"] = json!(true);
        assert!(Request::parse(&serde_json::to_vec(&v).unwrap()).is_err());
    }
}

#[test]
fn operator_status_snapshot_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("router.db");
    let mut store = Store::open(&path).unwrap();
    let req = request(&"cd".repeat(16), 10);
    store.operator_admit(peer(7), &req).unwrap();
    let snapshot = json!({"state":"running","live_commit":null,"step":4,"pending":0,"error":null});
    let done = store
        .operator_finish(7, &req, None, |_| Ok(snapshot.clone()))
        .unwrap();
    assert_eq!(done["receipt"]["state"], "applied");
    assert!(done["receipt"]["step"].as_u64().unwrap() > 0);
    assert_eq!(done["result"], snapshot);
    drop(store);
    let mut store = Store::open(path).unwrap();
    assert_eq!(store.operator_admit(peer(7), &req).unwrap(), done);
    assert_eq!(
        store
            .operator_finish(7, &req, None, |_| panic!("must not resample"))
            .unwrap(),
        done
    );
}

#[test]
fn operator_receipt_audit_atomicity() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("router.db");
    let mut store = Store::open(&path).unwrap();
    let sql = rusqlite::Connection::open(&path).unwrap();
    sql.execute_batch(
        "CREATE TRIGGER fail_admit BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'fault'); END;",
    )
    .unwrap();
    let req = request(&"de".repeat(16), 10);
    assert!(store.operator_admit(peer(7), &req).is_err());
    assert_eq!(
        sql.query_row("SELECT count(*) FROM operator_operations", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    sql.execute_batch("DROP TRIGGER fail_admit;").unwrap();
    let queued = store.operator_admit(peer(7), &req).unwrap();
    sql.execute_batch("CREATE TABLE config(value TEXT); CREATE TRIGGER fail_finish BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'fault'); END;").unwrap();
    let result = store.operator_finish(
        7,
        &req,
        Some(json!({"code":"permission_denied","message":"denied","retryable":false,"details":{}})),
        |tx| {
            tx.execute("INSERT INTO config VALUES('changed')", [])?;
            Ok(Value::Null)
        },
    );
    assert!(result.is_err());
    assert_eq!(
        sql.query_row("SELECT count(*) FROM config", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(store.operator_admit(peer(7), &req).unwrap(), queued);
    sql.execute_batch("DROP TRIGGER fail_finish;").unwrap();
    let done = store.operator_finish(7, &req, Some(json!({"code":"permission_denied","message":"denied","retryable":false,"details":{}})), |_| Ok(Value::Null)).unwrap();
    assert_eq!(done["receipt"]["state"], "failed");
    assert!(done["receipt"]["step"].is_null());
    drop(store);
    let mut store = Store::open(path).unwrap();
    assert_eq!(store.operator_admit(peer(7), &req).unwrap(), done);
}

#[test]
fn operator_idempotency_conflict() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("router.db");
    let mut store = Store::open(&path).unwrap();
    let req = request(&"ab".repeat(16), 100);
    let first = store.operator_admit(peer(7), &req).unwrap();
    assert_eq!(first["receipt"]["state"], "queued");
    assert_eq!(first["result"], Value::Null);
    assert_eq!(
        store
            .operator_admit(peer(7), &request(&"ab".repeat(16), 200))
            .unwrap(),
        first
    );
    let mut changed = req.value().clone();
    changed["params"]["target"] = json!(format!("coglet:{}", "33".repeat(32)));
    let changed = Request::parse(&serde_json::to_vec(&changed).unwrap()).unwrap();
    assert!(store.operator_admit(peer(7), &changed).is_err());
    // An identical ID under another authenticated identity cannot read this result.
    let other = store.operator_admit(peer(8), &changed).unwrap();
    assert_eq!(other["receipt"]["state"], "queued");
    drop(store);
    let mut store = Store::open(path).unwrap();
    assert_eq!(store.operator_admit(peer(7), &req).unwrap(), first);
}
