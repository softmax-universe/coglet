use super::tests::{OWNER, bundle, provision, replay};
use super::*;

fn archived() -> ArchivedDecision {
    use coglet_types::Value as V;
    let cases: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../spec/fixtures/database/cases.json"
    ))
    .unwrap();
    let raw = hex::decode(
        cases
            .as_array()
            .unwrap()
            .iter()
            .find(|x| x["name"] == "db-step-record")
            .unwrap()["cbor_hex"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let V::Map(mut m) = coglet_types::decode_profile(&raw).unwrap() else {
        panic!()
    };
    for (k, v) in &mut m {
        if matches!(k,V::Text(s) if s == "coglet") {
            *v = V::Text(OWNER.into());
        }
    }
    let bytes = coglet_types::encode_profile(&V::Map(m.clone())).unwrap();
    let digest = coordinator::record_hash_of(&bytes).unwrap();
    for (k, v) in &mut m {
        if matches!(k,V::Text(s) if s == "record_hash") {
            *v = V::Text(digest.clone());
        }
    }
    ArchivedDecision {
        record: coglet_types::encode_profile(&V::Map(m)).unwrap(),
        artifacts: b"immutable-files-effects-input".to_vec(),
    }
}
#[test]
fn snapshot_restores_nonempty_markers_receipts_and_immutable_artifacts() {
    let root = tempfile::tempdir().unwrap();
    let mut db = provision(root.path());
    db.connection.execute_batch("CREATE TABLE outbox(intent_id TEXT PRIMARY KEY NOT NULL, idempotency_key TEXT UNIQUE NOT NULL, state TEXT NOT NULL, receipt TEXT); INSERT INTO outbox VALUES('one','stable-key','succeeded','receipt-one'); INSERT INTO outbox VALUES('two','uncertain-key','uncertain',NULL);").unwrap();
    let a = archived();
    let r = coglet_types::DatabaseDbStepRecord1::from_cbor(&a.record).unwrap();
    db.connection
        .execute(
            "INSERT INTO _coglet_markers VALUES(?1,?2,?3,?4,?5,?6,1)",
            rusqlite::params![
                OWNER,
                r.trace_id,
                r.step.to_be_bytes().to_vec(),
                r.record_hash,
                r.db_catalog_hash,
                r.db_after.to_be_bytes().to_vec()
            ],
        )
        .unwrap();
    let mut b = bundle();
    b.decisions.push(a.clone());
    b.blobs.insert(hash(&a.artifacts), a.artifacts.clone());
    struct ExactReplay {
        expected: ArchivedDecision,
        head: String,
        calls: usize,
    }
    impl RestoreVerifier for ExactReplay {
        fn verify_replay_and_references(&mut self, _: &str, b: &RecoveryBundle) -> Result<()> {
            self.calls += 1;
            if b.journal_head != self.head
                || b.decisions.len() != 1
                || b.decisions[0].record != self.expected.record
                || b.decisions[0].artifacts != self.expected.artifacts
                || b.blobs.get(&hash(&self.expected.artifacts)) != Some(&self.expected.artifacts)
            {
                return Err("replay/artifact divergence".into());
            }
            // Offline recorded-observation decode. Full engine replay remains D.
            let r = coglet_types::DatabaseDbStepRecord1::from_cbor(&b.decisions[0].record)?;
            let expected = coglet_types::DatabaseDbStepRecord1::from_cbor(&self.expected.record)?;
            if r.calls != expected.calls {
                return Err("recorded call divergence".into());
            }
            Ok(())
        }
    }
    let mut v = ExactReplay {
        expected: a,
        head: b.journal_head.clone(),
        calls: 0,
    };
    let snapshot = db
        .snapshot(&root.path().join("backup"), &b, &mut v)
        .unwrap();
    let target = root.path().join("copy");
    Database::restore_isolated(&target, OWNER, &snapshot, &mut v).unwrap();
    assert_eq!(v.calls, 2);
    let copy = Database::open(&target, OWNER).unwrap();
    assert_eq!(
        copy.connection
            .query_row(
                "SELECT receipt FROM outbox WHERE idempotency_key='stable-key'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "receipt-one"
    );
    assert_eq!(
        copy.connection
            .query_row(
                "SELECT state FROM outbox WHERE idempotency_key='uncertain-key'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        "uncertain"
    );
    assert!(copy.dispatch_fence().is_err());
    b.decisions.clear();
    assert!(
        db.snapshot(&root.path().join("bad"), &b, &mut replay())
            .is_err()
    );
}
#[test]
fn approved_additive_outbox_ddl_allows_constraints_but_never_row_mutation() {
    let root = tempfile::tempdir().unwrap();
    let mut db = provision(root.path());
    let mut p = super::tests::plan(&db);
    p.sql="CREATE TABLE outbox(intent_id TEXT PRIMARY KEY NOT NULL,idempotency_key TEXT UNIQUE NOT NULL); CREATE INDEX outbox_key ON outbox(idempotency_key);".into();
    let future_root = tempfile::tempdir().unwrap();
    let mut future = provision(future_root.path());
    future.provision_for_test(&p.sql).unwrap();
    future.provision_for_test(super::tests::HISTORY).unwrap();
    p.to_schema_hash = future.schema_pin().unwrap().1;
    super::tests::apply(&mut db, &p, root.path()).unwrap();
    let mut forbidden = p.clone();
    forbidden.sql = "INSERT INTO outbox VALUES('bad','bad')".into();
    assert!(
        db.apply_migration(&forbidden, &forbidden.digest().unwrap())
            .is_err()
    );
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}
