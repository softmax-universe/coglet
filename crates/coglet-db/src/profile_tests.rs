//! Selected local naming through real SQLite calls, coordinator and archives.
use super::*;
use serde_json::{Value as Json, json};
const PROFILE: ContractProfile = ContractProfile::Surface02;
const KIND: &str = "softmax.app.invoke";

// Interpret JSON variants explicitly: workspace feature unification enables
// serde_json/arbitrary_precision, whose Serialize represents numbers as maps.
fn json_cbor(value: &Json) -> V {
    match value {
        Json::Null => V::Null,
        Json::Bool(value) => V::Bool(*value),
        Json::Number(value) => {
            if let Some(value) = value.as_u64() {
                V::Integer(value.into())
            } else {
                V::Integer(value.as_i64().expect("integer fixture").into())
            }
        }
        Json::String(value) => V::Text(value.clone()),
        Json::Array(values) => V::Array(values.iter().map(json_cbor).collect()),
        Json::Object(values) => V::Map(
            values
                .iter()
                .map(|(key, value)| (V::Text(key.clone()), json_cbor(value)))
                .collect(),
        ),
    }
}

fn request(kind: &str) -> Json {
    json!({"operation":"enqueue", "intent_id":"1".repeat(32),
        "idempotency_key":"2".repeat(32), "kind":kind,
        "payload":{"control":"send", "verb":"press"}, "correlation_id":"3".repeat(32)})
}
fn intent(kind: &str) -> Intent {
    Intent {
        intent_id: "1".repeat(32),
        idempotency_key: "2".repeat(32),
        kind: kind.into(),
        payload: json_cbor(&request(kind)["payload"]),
        correlation_id: "3".repeat(32),
    }
}
fn setup(root: &Path) -> Database {
    let mut db = Database::open(root, OWNER).unwrap();
    db.provision_for_test(include_str!("../../../coglets/factory/migrations/0001.sql"))
        .unwrap();
    db.provision_for_test("PRAGMA user_version=1;").unwrap();
    db
}
fn admit<'a>(
    db: &'a mut Database,
    cat: &'a Catalog,
    kind: &str,
    profile: ContractProfile,
) -> Unit<'a> {
    let mut a = admission(db, cat);
    a.pins.intent_kinds.insert(kind.into());
    db.admit_with_profile(cat, CODE, a, profile).unwrap()
}
fn wire(unit: &mut Unit<'_>, request: &Json) -> (CallResult, Vec<u8>) {
    let raw = coglet_types::encode_profile(&json_cbor(request)).unwrap();
    let mut memory = vec![0x55; 140000];
    memory[..raw.len()].copy_from_slice(&raw);
    let result = unit.call(
        &mut memory,
        0,
        raw.len() as i32,
        70000,
        65540,
        &mut HostBudget {
            remaining: 1_000_000,
        },
    );
    (result, memory)
}
fn prepared(cat: &Catalog, kind: &str) -> Decision {
    let d = decision(&cat.digest().unwrap(), 1, 0, 1, ARTIFACTS);
    let mut value: Json = coglet_types::decode_profile(&d.record)
        .unwrap()
        .deserialized()
        .unwrap();
    value["calls"] = json!([{"op":"db", "argument":request(kind), "result":0,
        "response":{"status":"enqueued", "intent_id":"1".repeat(32), "duplicate":false}}]);
    value["grants"] = json!(["db.write", kind]);
    value["events"][0]["kind"] = json!("softmax.app.view_changed");
    value["output_files"]["/data/count"] = json!("31");
    value["output_hash"] = json!("e".repeat(64));
    value["actions"] = json!([{"id":"4".repeat(32), "coglet":OWNER, "capability":kind,
        "args":{}, "deadline":20, "correlation_id":"3".repeat(32)}]);
    value["timers"] = json!([{"id":"5".repeat(32), "at":20}]);
    value["llm_requests"] = json!([{"id":"6".repeat(32), "correlation_id":"3".repeat(32),
        "capability":"llm.complete", "model":"fixture", "input":{}}]);
    value["logs"] = json!([{"level":"info", "message":"provisional", "fields":{}}]);
    value["record_hash"] = json!(
        coordinator::record_hash_of(&coglet_types::encode_profile(&json_cbor(&value)).unwrap())
            .unwrap()
    );
    Decision {
        record: coglet_types::encode_profile(&json_cbor(&value)).unwrap(),
        artifacts: d.artifacts,
    }
}

#[test]
fn selected_profile_four_buffer_and_direct_enqueue_default_refusal() {
    for direct in [false, true] {
        for profile in [ContractProfile::default(), PROFILE] {
            let root = tempfile::tempdir().unwrap();
            let mut db = setup(root.path());
            let cat = catalog();
            let mut u = if profile == ContractProfile::default() {
                let mut a = admission(&db, &cat);
                a.pins.intent_kinds.insert(KIND.into());
                db.admit(&cat, CODE, a).unwrap()
            } else {
                admit(&mut db, &cat, KIND, profile)
            };
            assert_eq!(u.contract_profile(), profile);
            if direct {
                assert_eq!(u.enqueue(&intent(KIND)).is_ok(), profile == PROFILE);
            } else {
                let (r, memory) = wire(&mut u, &request(KIND));
                if profile == PROFILE {
                    let CallResult::Return {
                        result: 0,
                        response: Some(response),
                    } = r
                    else {
                        panic!("{r:?}")
                    };
                    coglet_types::decode("database::db-reply", &response).unwrap();
                    assert_eq!(
                        &memory[70000..70004],
                        &(response.len() as u32).to_be_bytes()
                    );
                    assert_eq!(&memory[70004..70004 + response.len()], response.as_slice());
                    assert_eq!(memory[70004 + response.len()], 0x55);
                } else {
                    assert_eq!(
                        r,
                        CallResult::Return {
                            result: 1,
                            response: None
                        }
                    );
                    assert!(memory[70000..].iter().all(|b| *b == 0x55));
                }
            }
            assert_eq!(u.is_poisoned(), profile != PROFILE);
            if profile == PROFILE {
                u.commit_for_test().unwrap();
            } else {
                drop(u);
            }
            let count: i64 = db
                .connection
                .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, i64::from(profile == PROFILE));
        }
    }
}

#[test]
fn selected_profile_exact_grammar_negatives_and_old_name_positive() {
    for kind in [
        "other.app.invoke",
        "softmax.application.invoke",
        "softmax.app.invoke.extra",
        "softmax.app.",
        "softmax.app.Invoke",
        "softmax.app.1invoke",
        "softmax.app.invoke-",
        "softmax.app..invoke",
    ] {
        for direct in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let mut db = setup(root.path());
            let cat = catalog();
            let mut u = admit(&mut db, &cat, kind, PROFILE);
            if direct {
                assert!(u.enqueue(&intent(kind)).is_err(), "{kind}");
            } else {
                assert_eq!(
                    wire(&mut u, &request(kind)).0,
                    CallResult::Return {
                        result: 1,
                        response: None
                    },
                    "{kind}"
                );
            }
            assert!(u.is_poisoned());
            assert!(!u.transaction);
        }
    }
    for profile in [ContractProfile::default(), PROFILE] {
        let root = tempfile::tempdir().unwrap();
        let mut db = setup(root.path());
        let cat = catalog();
        admit(&mut db, &cat, "work.start", profile)
            .enqueue(&intent("work.start"))
            .unwrap();
    }
    for frozen in [
        "database::db-reply",
        "database::db-statement",
        "database::db-manifest",
        "abi::call",
        "common::capability-name",
    ] {
        assert_eq!(PROFILE.rule(frozen), frozen);
    }
    assert_eq!(
        PROFILE.rule("database::db-host-call"),
        "surface02::s02-db-host-call"
    );
}

#[test]
fn selected_profile_complete_and_marker_present_reopen_publish_exact_bytes() {
    for unavailable in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let mut db = setup(root.path());
        let cat = catalog();
        let d = prepared(&cat, KIND);
        assert!(coordinator::validate_committed(&d.record, ContractProfile::default()).is_err());
        let mut journal = FileJournal::new(&root.path().join("journal"));
        journal.fail_commit_once = unavailable;
        let mut u = admit(&mut db, &cat, KIND, PROFILE);
        u.enqueue(&intent(KIND)).unwrap();
        let completed = u.complete(&d, &mut journal);
        if unavailable {
            assert!(completed.unwrap_err().starts_with(HELD));
        } else {
            completed.unwrap();
        }
        drop(db);
        let mut reopened = Database::open(root.path(), OWNER).unwrap();
        let recovered = reopened
            .recover_with_profile(&mut journal, PROFILE)
            .unwrap();
        assert_eq!(
            matches!(recovered, Recovered::RolledForward(_)),
            unavailable
        );
        assert_eq!(
            journal.committed(1).unwrap(),
            format!("{}\n{}", hex::encode(&d.record), hex::encode(ARTIFACTS))
        );
        assert!(journal.aborted(1).is_none());
        assert_eq!(reopened.counters().unwrap(), (1, 1));
        reopened.dispatch_fence().unwrap();
        assert!(reopened.pending_files().unwrap().is_empty());
    }
}

#[test]
fn selected_profile_marker_absent_abort_retains_observations_discards_effects() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let d = prepared(&cat, KIND);
    // A prepared but uncommitted transaction is rolled back on drop, as at restart.
    {
        let mut u = admit(&mut db, &cat, KIND, PROFILE);
        u.enqueue(&intent(KIND)).unwrap();
        coordinator::forge_prepared_with_profile(u.db, &d, PROFILE).unwrap();
    }
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut journal = FileJournal::new(&root.path().join("journal"));
    assert!(matches!(
        db.recover_with_profile(&mut journal, PROFILE).unwrap(),
        Recovered::Aborted(_)
    ));
    let raw = journal.aborted(1).unwrap();
    assert_eq!(
        raw,
        recovery_abort_record_with_profile(&d.record, PROFILE).unwrap()
    );
    assert!(recovery_abort_record(&d.record).is_err());
    let c: Json = coglet_types::decode_profile(&d.record)
        .unwrap()
        .deserialized()
        .unwrap();
    let a: Json = coglet_types::decode(PROFILE.rule("database::db-step-record"), &raw)
        .unwrap()
        .deserialized()
        .unwrap();
    for field in [
        "calls",
        "events",
        "boundary",
        "input_files",
        "input_hash",
        "usage",
        "grants",
        "seed",
        "trace_id",
        "step",
    ] {
        assert_eq!(a[field], c[field], "{field}");
    }
    for field in ["actions", "timers", "llm_requests", "logs"] {
        assert!(!c[field].as_array().unwrap().is_empty());
        assert!(a[field].as_array().unwrap().is_empty());
    }
    assert_ne!(c["output_files"], c["input_files"]);
    assert_eq!(a["output_files"], c["input_files"]);
    assert_eq!(a["output_hash"], c["input_hash"]);
    assert_eq!(a["db_after"], c["db_before"]);
    assert_eq!(a["failure"]["prepared_record_hash"], c["record_hash"]);
    assert_eq!(a["record_hash"], coordinator::record_hash_of(&raw).unwrap());
    assert_ne!(a["record_hash"], c["record_hash"]);
    assert!(journal.committed(1).is_none());
    assert_eq!(db.counters().unwrap(), (0, 1));
    assert_eq!(
        db.connection
            .query_row("SELECT COUNT(*) FROM outbox", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.dispatch_fence().unwrap();
}

#[test]
fn default_complete_and_recovery_never_infer_profile_from_prepared_record() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let d = prepared(&cat, KIND);
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let mut a = admission(&db, &cat);
    a.pins.intent_kinds.insert("work.start".into());
    let mut u = db.admit(&cat, CODE, a).unwrap();
    u.enqueue(&intent("work.start")).unwrap();
    assert!(u.complete(&d, &mut journal).is_err());
    assert!(db.pending_files().unwrap().is_empty());
    assert_eq!(db.unpublished_markers().unwrap(), 0);
    assert!(journal.committed(1).is_none());
    coordinator::forge_prepared_with_profile(&db, &d, PROFILE).unwrap();
    drop(db);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    assert!(
        db.recover(&mut journal)
            .unwrap_err()
            .starts_with(CORRUPTION)
    );
    assert!(db.halted());
    assert!(journal.aborted(1).is_none());
    assert!(journal.committed(1).is_none());
}

struct ExactArchive(RecoveryBundle);
impl RestoreVerifier for ExactArchive {
    fn verify_replay_and_references(&mut self, owner: &str, bundle: &RecoveryBundle) -> Result<()> {
        if owner != OWNER
            || serde_json::to_vec(bundle).unwrap() != serde_json::to_vec(&self.0).unwrap()
        {
            return Err("archive observations/artifacts differ".into());
        }
        coordinator::validate_committed(&bundle.decisions[0].record, PROFILE)?;
        Ok(())
    }
}
#[test]
fn selected_profile_snapshot_restore_alignment_and_default_refusal() {
    let root = tempfile::tempdir().unwrap();
    let mut db = setup(root.path());
    let cat = catalog();
    let d = prepared(&cat, KIND);
    let mut journal = FileJournal::new(&root.path().join("journal"));
    let mut u = admit(&mut db, &cat, KIND, PROFILE);
    u.enqueue(&intent(KIND)).unwrap();
    u.complete(&d, &mut journal).unwrap();
    let bundle = RecoveryBundle {
        journal_head: coordinator::record_hash_of(&d.record).unwrap(),
        catalog_hash: cat.digest().unwrap(),
        decisions: vec![ArchivedDecision {
            record: d.record.clone(),
            artifacts: d.artifacts.clone(),
        }],
        blobs: [(
            hex::encode(Sha256::digest(&d.artifacts)),
            d.artifacts.clone(),
        )]
        .into(),
    };
    let mut verifier = ExactArchive(bundle.clone());
    assert!(
        db.snapshot(&root.path().join("refused"), &bundle, &mut verifier)
            .is_err()
    );
    let snapshot = db
        .snapshot_with_profile(
            &root.path().join("archive"),
            &bundle,
            &mut verifier,
            PROFILE,
        )
        .unwrap();
    assert!(
        Database::restore_isolated(
            &root.path().join("default-restore"),
            OWNER,
            &snapshot,
            &mut verifier
        )
        .is_err()
    );
    let restored = root.path().join("selected-restore");
    let report = Database::restore_isolated_with_profile(
        &restored,
        OWNER,
        &snapshot,
        &mut verifier,
        PROFILE,
    )
    .unwrap();
    assert_eq!(report.revision, 1);
    let reopened = Database::open(&restored, OWNER).unwrap();
    assert!(reopened.dispatch_fence().unwrap_err().contains("isolated"));
    assert_eq!(
        reopened
            .connection
            .query_row("SELECT kind FROM outbox", [], |r| r.get::<_, String>(0))
            .unwrap(),
        KIND
    );
}
