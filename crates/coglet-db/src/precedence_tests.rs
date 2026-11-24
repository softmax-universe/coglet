use super::*;
use coglet_types::{DatabaseDbEnqueue, Value};

#[test]
fn enqueue_identity_conflicts_precede_fresh_resources_without_bypassing_call_cap() {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(root.path(), "precedence").unwrap();
    db.provision_for_test(include_str!("../../../coglets/factory/migrations/0001.sql"))
        .unwrap();
    let cat = Catalog::approve_typed(vec![TypedStatement {
        statement: Statement::new("scan", "SELECT id FROM objects ORDER BY id", Mode::Read, 0),
        parameters: vec![],
        columns: vec![Column {
            name: "id".into(),
            kind: CellType::Text,
        }],
        tables: vec!["objects".into()],
        order: vec!["id".into()],
    }])
    .unwrap();
    let code = "a".repeat(64);
    let admission = |db: &Database| {
        let (schema_version, schema_hash) = db.schema_pin().unwrap();
        Admission {
            pins: Pins {
                code_hash: code.clone(),
                schema_version,
                schema_hash,
                catalog_hash: cat.digest().unwrap(),
                grants: Grants {
                    read: true,
                    write: true,
                },
                intent_kinds: ["work.start".into()].into(),
            },
            schema_min: 0,
            schema_max: 0,
            deadline: Instant::now() + Duration::from_secs(10),
        }
    };
    let a = admission(&db);
    let mut u = db.admit(&cat, &code, a).unwrap();
    u.enqueue(&Intent {
        intent_id: "1".repeat(32),
        idempotency_key: "2".repeat(32),
        kind: "work.start".into(),
        payload: Value::Map(vec![]),
        correlation_id: "3".repeat(32),
    })
    .unwrap();
    u.commit_for_test().unwrap();
    for native in [false, true] {
        for scenario in 0..4 {
            let a = admission(&db);
            let mut u = db.admit(&cat, &code, a).unwrap();
            for _ in 0..32 {
                u.statement("scan", Mode::Read, &[], 64, 65536).unwrap();
            }
            let (id, key, correlation) = match scenario {
                0 => ("1", "2", "4"), // same key, changed content
                1 => ("1", "5", "3"), // new key, reused ID
                2 => ("9", "2", "3"), // identical retry, original ID wins
                _ => ("9", "5", "3"), // entirely fresh work
            };
            if native {
                let error = u
                    .enqueue(&Intent {
                        intent_id: id.repeat(32),
                        idempotency_key: key.repeat(32),
                        kind: "work.start".into(),
                        payload: Value::Map(vec![]),
                        correlation_id: correlation.repeat(32),
                    })
                    .unwrap_err();
                assert_eq!(
                    error,
                    match scenario {
                        0 => "idempotency conflict",
                        1 => "intent ID conflict",
                        _ => "resource exhausted",
                    }
                );
            } else {
                let request = DatabaseDbEnqueue {
                    operation: "enqueue".into(),
                    intent_id: id.repeat(32),
                    idempotency_key: key.repeat(32),
                    kind: "work.start".into(),
                    payload: Default::default(),
                    correlation_id: correlation.repeat(32),
                }
                .to_cbor()
                .unwrap();
                let mut memory = vec![0x55; 140000];
                memory[..request.len()].copy_from_slice(&request);
                let result = u.call(
                    &mut memory,
                    0,
                    request.len() as i32,
                    70000,
                    65540,
                    &mut HostBudget {
                        remaining: 1_000_000,
                    },
                );
                if scenario < 2 {
                    let CallResult::Return {
                        result: 0,
                        response: Some(response),
                    } = result
                    else {
                        panic!("{result:?}")
                    };
                    assert!(
                        format!("{:?}", coglet_types::decode_profile(&response).unwrap())
                            .contains("conflict")
                    );
                } else {
                    assert_eq!(
                        result,
                        CallResult::Return {
                            result: 3,
                            response: None
                        }
                    );
                    assert!(memory[70000..].iter().all(|b| *b == 0x55));
                }
            }
            assert_eq!(u.calls, 33);
            assert!(u.poisoned);
            assert!(u.db.connection.is_autocommit());
        }
    }
}
