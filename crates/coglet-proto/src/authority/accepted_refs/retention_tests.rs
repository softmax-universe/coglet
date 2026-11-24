use super::*;

#[test]
fn operation_receipt_storage_is_completion_only_and_terminal_immutable() {
    let authority = Authority::open(":memory:").unwrap();
    let _store = AcceptedRefStore::new(authority.clone()).unwrap();
    {
        let state = authority.0.lock().unwrap();
        state.db.execute_batch("DROP TRIGGER proto_git_terminal_immutable;
            DROP TRIGGER proto_git_completion_only;
            CREATE TRIGGER proto_git_terminal_immutable BEFORE UPDATE ON proto_git_operations
            WHEN json_extract(OLD.receipt,'$.state') != 'queued' BEGIN SELECT RAISE(ABORT,'old terminal'); END;
            CREATE TRIGGER proto_git_completion_only BEFORE UPDATE ON proto_git_operations
            WHEN json_extract(NEW.receipt,'$.state') NOT IN ('applied','failed','cancelled') BEGIN SELECT RAISE(ABORT,'old completion'); END;").unwrap();
    }
    // Opening retained pre-hardening storage replaces weak triggers atomically.
    let _reopened = AcceptedRefStore::new(authority.clone()).unwrap();
    let state = authority.0.lock().unwrap();
    let db = &state.db;
    // Trusted storage-boundary probe, not wire admission/verification evidence.
    db.execute_batch("INSERT INTO proto_ref_audit VALUES(1,'hub','target','refs/heads/hub','authoritative','{}','peer','actor',NULL,'1',NULL,'10',X'');
        INSERT INTO proto_git_operations VALUES('peer','id','target','digest','{\"operation_id\":\"id\",\"state\":\"queued\"}',1);
        INSERT INTO proto_git_intents VALUES('peer','id','target','{}');").unwrap();
    for receipt in [
        "{\"operation_id\":\"id\"}",
        "{\"operation_id\":\"id\",\"state\":null}",
        "{\"operation_id\":\"id\",\"state\":\"queued\"}",
        "{\"operation_id\":\"other\",\"state\":\"applied\"}",
    ] {
        assert!(
            db.execute("UPDATE proto_git_operations SET receipt=?1", [receipt])
                .is_err()
        );
    }
    assert!(db.execute("UPDATE proto_git_operations SET digest='other',receipt='{\"operation_id\":\"id\",\"state\":\"applied\"}'", []).is_err());
    db.execute(
        "UPDATE proto_git_operations SET receipt='{\"operation_id\":\"id\",\"state\":\"applied\"}'",
        [],
    )
    .unwrap();
    db.execute(
        "INSERT INTO proto_preinstall_boundaries VALUES(1,'peer','id','target','10')",
        [],
    )
    .unwrap();
    for table in [
        "proto_git_operations",
        "proto_git_intents",
        "proto_preinstall_boundaries",
    ] {
        assert!(db.execute(&format!("DELETE FROM {table}"), []).is_err());
        assert!(
            db.execute(
                &format!("INSERT OR REPLACE INTO {table} SELECT * FROM {table}"),
                []
            )
            .is_err()
        );
        let count: i64 = db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
    assert!(
        db.execute("UPDATE proto_git_operations SET receipt=receipt", [])
            .is_err()
    );
    assert!(
        db.execute("UPDATE proto_git_intents SET request=request", [])
            .is_err()
    );
    assert!(
        db.execute(
            "UPDATE proto_preinstall_boundaries SET completed_at=completed_at",
            []
        )
        .is_err()
    );
    let receipt: String = db
        .query_row("SELECT receipt FROM proto_git_operations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&receipt).unwrap()["state"],
        "applied"
    );
}
