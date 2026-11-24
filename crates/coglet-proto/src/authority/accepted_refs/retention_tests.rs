use super::*;
use iroh::SecretKey;
use rusqlite::TransactionBehavior;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct ScopeReceipt {
    store: AcceptedRefStore,
    path: PathBuf,
    peer: iroh::EndpointId,
    target: String,
    operation_id: String,
    scope: Scope,
    receipt: Value,
}
impl Drop for ScopeReceipt {
    fn drop(&mut self) {
        let path = self.path.clone();
        let wal = format!("{}-wal", path.display());
        let shm = format!("{}-shm", path.display());
        // Connection still lives in `store` until after Drop; best-effort cleanup.
        let _ = std::fs::remove_file(&wal);
        let _ = std::fs::remove_file(&shm);
        let _ = std::fs::remove_file(&path);
    }
}

fn seed_scope_receipt(expires: u64, generation: u64, mode: &str) -> ScopeReceipt {
    let path = std::env::temp_dir().join(format!(
        "coglet-push-receipt-scope-{}.sqlite",
        SecretKey::generate().public()
    ));
    let authority = Authority::open(&path).unwrap();
    let store = AcceptedRefStore::new(authority.clone()).unwrap();
    let peer = SecretKey::from([2; 32]).public();
    let target = "target".to_owned();
    let operation_id = "ab".repeat(16);
    let scope = Scope {
        actor: "actor".into(),
        ticket: Some("cd".repeat(16)),
    };
    let receipt = serde_json::json!({
        "operation_id": operation_id,
        "state": "queued",
        "step": null,
        "error": null,
    });
    {
        let state = authority.0.lock().unwrap();
        state
            .db
            .execute(
                "INSERT INTO proto_peers(peer) VALUES(?1)",
                [peer.to_string()],
            )
            .unwrap();
        state
            .db
            .execute(
                "INSERT INTO proto_grants(peer,method,target,expires,generation) VALUES(?1,'hub.git.push',?2,?3,?4)",
                params![peer.to_string(), target, expires.to_string(), generation.to_string()],
            )
            .unwrap();
        state
            .db
            .execute(
                "INSERT INTO proto_scopes(peer,actor,ticket,method,target,expires,generation) VALUES(?1,?2,?3,'hub.git.push',?4,?5,?6)",
                params![
                    peer.to_string(),
                    scope.actor,
                    scope.ticket.as_deref().unwrap_or_default(),
                    target,
                    expires.to_string(),
                    generation.to_string()
                ],
            )
            .unwrap();
        state
            .db
            .execute(
                "INSERT INTO proto_ref_audit(id,hub,coglet,ref,mode,signed,peer,actor,ticket,grant_generation,delegation_generation,accepted_at,bundle) VALUES(1,'hub',?1,'refs/heads/hub',?2,'{}',?3,?4,?5,?6,NULL,'10',X'')",
                params![
                    target,
                    mode,
                    peer.to_string(),
                    scope.actor,
                    scope.ticket,
                    generation.to_string()
                ],
            )
            .unwrap();
        state
            .db
            .execute(
                "INSERT INTO proto_git_operations(peer,operation_id,target,digest,receipt,audit_id) VALUES(?1,?2,?3,'digest',?4,1)",
                params![
                    peer.to_string(),
                    operation_id,
                    target,
                    serde_json::to_string(&receipt).unwrap()
                ],
            )
            .unwrap();
    }
    ScopeReceipt {
        store,
        path,
        peer,
        target,
        operation_id,
        scope,
        receipt,
    }
}

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

#[test]
fn push_receipt_for_scope_denies_missing_clock_unknown_and_expiry_indistinguishably() {
    let fixture = seed_scope_receipt(20, 1, "authoritative");
    let found = fixture
        .store
        .push_receipt_for_scope(
            fixture.peer,
            &fixture.target,
            &fixture.operation_id,
            &fixture.scope,
            1,
            || Some(1),
        )
        .unwrap();
    assert_eq!(found.as_ref(), Some(&fixture.receipt));
    assert!(
        fixture
            .store
            .push_receipt_for_scope(
                fixture.peer,
                &fixture.target,
                &fixture.operation_id,
                &fixture.scope,
                1,
                || None,
            )
            .unwrap()
            .is_none()
    );
    let samples = std::cell::Cell::new(0);
    assert!(
        fixture
            .store
            .push_receipt_for_scope(
                fixture.peer,
                &fixture.target,
                &fixture.operation_id,
                &fixture.scope,
                1,
                || {
                    let n = samples.get();
                    samples.set(n + 1);
                    if n == 0 { Some(1) } else { None }
                },
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(samples.get(), 2);
    let samples = std::cell::Cell::new(0);
    assert!(
        fixture
            .store
            .push_receipt_for_scope(
                fixture.peer,
                &fixture.target,
                &fixture.operation_id,
                &fixture.scope,
                1,
                || {
                    let n = samples.get();
                    samples.set(n + 1);
                    Some(if n == 0 { 1 } else { 20 })
                },
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(samples.get(), 2);
    let unknown = "ef".repeat(16);
    let unknown_none = fixture
        .store
        .push_receipt_for_scope(
            fixture.peer,
            &fixture.target,
            &unknown,
            &fixture.scope,
            1,
            || Some(1),
        )
        .unwrap();
    let expired = fixture
        .store
        .push_receipt_for_scope(
            fixture.peer,
            &fixture.target,
            &fixture.operation_id,
            &fixture.scope,
            1,
            || Some(20),
        )
        .unwrap();
    let wrong_generation = fixture
        .store
        .push_receipt_for_scope(
            fixture.peer,
            &fixture.target,
            &fixture.operation_id,
            &fixture.scope,
            2,
            || Some(1),
        )
        .unwrap();
    assert!(unknown_none.is_none() && expired.is_none() && wrong_generation.is_none());
    let observed = seed_scope_receipt(20, 1, "observation");
    assert!(
        observed
            .store
            .push_receipt_for_scope(
                observed.peer,
                &observed.target,
                &observed.operation_id,
                &observed.scope,
                1,
                || Some(1),
            )
            .unwrap()
            .is_none()
    );
}

#[test]
fn push_receipt_for_scope_samples_clock_after_lock_wait_and_denies_stale_expiry() {
    let fixture = seed_scope_receipt(20, 1, "authoritative");
    let mut holder = rusqlite::Connection::open(&fixture.path).unwrap();
    holder.busy_timeout(Duration::from_secs(5)).unwrap();
    let guard = holder
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .unwrap();
    let sampled = Arc::new(AtomicBool::new(false));
    let now = Arc::new(AtomicU64::new(1));
    let store = fixture.store.clone();
    let peer = fixture.peer;
    let target = fixture.target.clone();
    let operation_id = fixture.operation_id.clone();
    let scope = fixture.scope.clone();
    let sampled_thread = sampled.clone();
    let now_thread = now.clone();
    let lookup = thread::spawn(move || {
        store.push_receipt_for_scope(peer, &target, &operation_id, &scope, 1, || {
            sampled_thread.store(true, Ordering::SeqCst);
            Some(now_thread.load(Ordering::SeqCst))
        })
    });
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(300) {
        assert!(
            !sampled.load(Ordering::SeqCst),
            "clock sampled before Immediate custody"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!lookup.is_finished());
    now.store(20, Ordering::SeqCst);
    drop(guard);
    let result = lookup.join().unwrap().unwrap();
    assert!(sampled.load(Ordering::SeqCst));
    assert!(result.is_none());
}

#[test]
fn push_receipt_for_scope_cross_connection_revoke_cannot_interleave_authorized_read() {
    let fixture = seed_scope_receipt(20, 1, "authoritative");
    let (blocked_tx, blocked_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let path = fixture.path.clone();
    let samples = Arc::new(AtomicU64::new(0));
    let samples_clock = samples.clone();
    let found = fixture
        .store
        .push_receipt_for_scope(
            fixture.peer,
            &fixture.target,
            &fixture.operation_id,
            &fixture.scope,
            1,
            || {
                let n = samples_clock.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    let blocked_tx = blocked_tx.clone();
                    let done_tx = done_tx.clone();
                    let path = path.clone();
                    thread::spawn(move || {
                        let other = rusqlite::Connection::open(&path).unwrap();
                        other.busy_timeout(Duration::from_secs(5)).unwrap();
                        blocked_tx.send(()).unwrap();
                        other.execute("DELETE FROM proto_grants", []).unwrap();
                        done_tx.send(()).unwrap();
                    });
                    blocked_rx.recv().unwrap();
                    let start = Instant::now();
                    while start.elapsed() < Duration::from_millis(200) {
                        assert!(
                            done_rx.try_recv().is_err(),
                            "cross-connection revoke completed under Immediate"
                        );
                        thread::sleep(Duration::from_millis(10));
                    }
                }
                Some(1)
            },
        )
        .unwrap();
    assert_eq!(found.as_ref(), Some(&fixture.receipt));
    assert_eq!(samples.load(Ordering::SeqCst), 2);
    done_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        fixture
            .store
            .push_receipt_for_scope(
                fixture.peer,
                &fixture.target,
                &fixture.operation_id,
                &fixture.scope,
                1,
                || Some(1),
            )
            .unwrap()
            .is_none()
    );
}
