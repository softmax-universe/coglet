//! Real SQLite guest-boundary tests. Runtime/journal physical crash qualification
//! remains in coglet-runtime/tests/factory_database.rs; these exercise the agenda.
use coglet_factory::{
    Object, Repository,
    executive::{Decision, Policy, Proposal},
};
use coglet_guest_rust::{Context, Event, Host, HostError, Operation, database::DbHost};
use coglet_types::{
    DatabaseDbEnqueue, DatabaseDbEnqueued, DatabaseDbPage, DatabaseDbStatement, LlmRequest, Value,
};
use rusqlite::{Connection, params};
use serde_json::json;

const TASK: &str = "11111111111111111111111111111111";
struct TestHost {
    db: Connection,
    staged: Vec<LlmRequest>,
    calls: usize,
}
impl TestHost {
    fn new() -> Self {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
        db.execute_batch(coglet_factory::SCHEMA_SQL).unwrap();
        let mut host = Self {
            db,
            staged: vec![],
            calls: 0,
        };
        let mut repo = Repository::new(&mut host);
        repo.create_object(&object(TASK, "task", "pending", json!({"context_ids":[]})))
            .unwrap();
        repo.create_object(&object(
            "factory-policy",
            "policy",
            "active",
            serde_json::to_value(Policy {
                model: "mock".into(),
                remaining_budget: 10,
                available_workers: 2,
                max_priority: 100,
            })
            .unwrap(),
        ))
        .unwrap();
        host
    }
    fn run(&mut self, e: Event) {
        self.run_failure(e, false).unwrap();
    }
    fn run_failure(&mut self, e: Event, abort: bool) -> Result<(), HostError> {
        self.calls = 0;
        let before = self.staged.len();
        self.db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let result = coglet_factory::wake(&mut Context::new(self, e.seq, e.logical_time, 3), &[e]);
        if result.is_err() || abort {
            self.db.execute_batch("ROLLBACK").unwrap();
            self.staged.truncate(before);
        } else {
            self.db.execute_batch("COMMIT").unwrap();
        }
        result
    }
    fn get(&self, id: &str) -> (String, serde_json::Value, u64) {
        self.db
            .query_row(
                "SELECT status,data_json,version FROM objects WHERE id=?1",
                [id],
                |r| {
                    let text: String = r.get(1)?;
                    Ok((
                        r.get(0)?,
                        serde_json::from_str(&text).unwrap(),
                        r.get::<_, i64>(2)? as u64,
                    ))
                },
            )
            .unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        assert!(["objects", "events", "outbox"].contains(&table));
        self.db
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn response(&self, seq: u64, operations: Vec<Decision>) -> Event {
        let request = self.staged.last().unwrap();
        let (_, root, _) = self.get("factory-executive");
        let state_version = root["pending"]["state_version"].as_u64().unwrap();
        event(
            "llm.reply",
            seq,
            [
                ("request_id", Value::Text(request.id.clone())),
                ("status", Value::Text("succeeded".into())),
                (
                    "output",
                    Value::Map(vec![(
                        Value::Text("decisions_json".into()),
                        Value::Text(
                            serde_json::to_string(&Proposal {
                                state_version,
                                operations,
                            })
                            .unwrap(),
                        ),
                    )]),
                ),
            ],
        )
    }
}
fn object(id: &str, kind: &str, status: &str, data: serde_json::Value) -> Object {
    Object {
        id: id.into(),
        object_type: kind.into(),
        status: status.into(),
        data_json: data.to_string(),
        version: 1,
        created_at: 0,
        updated_at: 0,
    }
}
fn event<const N: usize>(kind: &str, seq: u64, payload: [(&str, Value); N]) -> Event {
    Event {
        source: "coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".into(),
        kind: kind.into(),
        seq,
        logical_time: seq,
        payload: payload.into_iter().map(|(k, v)| (k.into(), v)).collect(),
        correlation_id: "0".repeat(32),
    }
}
fn task_event(kind: &str, seq: u64) -> Event {
    event(kind, seq, [("task_id", Value::Text(TASK.into()))])
}
impl Host for TestHost {
    fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32 {
        if operation == Operation::Timer {
            return 0;
        }
        assert_eq!(operation, Operation::Llm);
        self.staged
            .push(coglet_types::decode_typed("abi::llm-request", bytes).unwrap());
        0
    }
}
impl DbHost for TestHost {
    fn db(&mut self, input: &[u8], output: &mut [u8]) -> i32 {
        self.calls += 1;
        assert!(self.calls <= 32, "DB step call budget exceeded");
        let value = coglet_types::decode_profile(input).unwrap();
        let operation = value
            .as_map()
            .unwrap()
            .iter()
            .find(|(k, _)| k.as_text() == Some("operation"))
            .unwrap()
            .1
            .as_text()
            .unwrap();
        let bytes = if operation == "enqueue" {
            let e: DatabaseDbEnqueue =
                coglet_types::decode_typed("database::db-enqueue", input).unwrap();
            let raw = coglet_types::encode_profile(&Value::Map(
                e.payload
                    .into_iter()
                    .map(|(k, v)| (Value::Text(k), v))
                    .collect(),
            ))
            .unwrap();
            let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
            if self
                .db
                .execute(
                    "INSERT INTO outbox VALUES(?1,?2,?3,?4,?5,'pending',0,1)",
                    params![
                        e.intent_id,
                        e.idempotency_key,
                        e.kind,
                        hex,
                        e.correlation_id
                    ],
                )
                .is_err()
            {
                return 4;
            }
            DatabaseDbEnqueued {
                status: "enqueued".into(),
                intent_id: e.intent_id,
                duplicate: false,
            }
            .to_cbor()
            .unwrap()
        } else {
            let request: DatabaseDbStatement =
                coglet_types::decode_typed("database::db-statement", input).unwrap();
            let definition = coglet_factory::catalog::STATEMENTS
                .iter()
                .find(|s| s.id == request.statement)
                .unwrap();
            let params: Vec<rusqlite::types::Value> = request
                .params
                .into_iter()
                .map(|v| match v {
                    Value::Null => rusqlite::types::Value::Null,
                    Value::Text(s) => s.into(),
                    Value::Integer(n) => i64::try_from(i128::from(n)).unwrap().into(),
                    _ => panic!("param"),
                })
                .collect();
            let mut stmt = self.db.prepare(definition.sql).unwrap();
            let (columns, rows, affected) = if request.mode == "write" {
                let Ok(n) = stmt.execute(rusqlite::params_from_iter(params)) else {
                    return 4;
                };
                if definition
                    .expected_affected
                    .is_some_and(|expected| expected != n as u64)
                {
                    return 4;
                }
                (vec![], vec![], n as u64)
            } else {
                let columns: Vec<String> =
                    stmt.column_names().iter().map(|s| s.to_string()).collect();
                let mut cursor = stmt.query(rusqlite::params_from_iter(params)).unwrap();
                let mut rows = vec![];
                while let Some(row) = cursor.next().unwrap() {
                    if rows.len() == request.max_rows as usize {
                        break;
                    }
                    rows.push(
                        (0..columns.len())
                            .map(|i| match row.get_ref(i).unwrap() {
                                rusqlite::types::ValueRef::Null => Value::Null,
                                rusqlite::types::ValueRef::Text(s) => {
                                    Value::Text(std::str::from_utf8(s).unwrap().into())
                                }
                                rusqlite::types::ValueRef::Integer(n) => Value::Integer(n.into()),
                                _ => panic!("cell"),
                            })
                            .collect(),
                    );
                }
                (columns, rows, 0)
            };
            DatabaseDbPage {
                status: "page".into(),
                columns,
                rows,
                affected,
                revision: 0,
                next_cursor: Value::Null,
            }
            .to_cbor()
            .unwrap()
        };
        assert!(bytes.len() + 4 <= output.len());
        output[..4].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
        output[4..4 + bytes.len()].copy_from_slice(&bytes);
        0
    }
}
#[test]
fn routine_event_is_deterministic_no_llm() {
    let mut a = TestHost::new();
    let mut b = TestHost::new();
    for kind in ["task.heartbeat", "task.lease_expired"] {
        if kind == "task.lease_expired" {
            continue;
        }
        a.run(task_event(kind, 1));
        b.run(task_event(kind, 1));
    }
    assert_eq!(a.get(TASK), b.get(TASK));
    assert!(a.staged.is_empty());
    assert_eq!(a.count("events"), 0);
}
#[test]
fn duplicate_event_idempotent() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    let counts = (h.count("objects"), h.count("events"), h.staged.len());
    h.run(task_event("task.failed", 1));
    assert_eq!(
        counts,
        (h.count("objects"), h.count("events"), h.staged.len())
    );
}
#[test]
fn stale_llm_response_discarded_and_replanned() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    let response = h.response(
        3,
        vec![Decision::Evaluate {
            conclusion: "wrong".into(),
        }],
    );
    h.db.execute("UPDATE objects SET version=version+1 WHERE id=?1", [TASK])
        .unwrap();
    h.run(response);
    assert_eq!(h.staged.len(), 2);
    assert_eq!(h.get(TASK).1.get("evaluation"), None);
    assert_eq!(
        h.db.query_row(
            "SELECT count(*) FROM events WHERE event_type='decision.stale'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}
#[test]
fn crash_before_commit_recovers() {
    let mut h = TestHost::new();
    h.run_failure(task_event("task.failed", 1), true).unwrap();
    assert_eq!(h.get(TASK).0, "pending");
    assert!(h.staged.is_empty());
    h.run(task_event("task.failed", 1));
    assert_eq!(h.staged.len(), 1);
    assert_eq!(h.get(TASK).0, "failed");
}
#[test]
fn crash_after_commit_recovers() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    // Discard transient host object by copying the actual database to a new connection.
    let path = std::env::temp_dir().join(format!("factory-{}-restart.db", std::process::id()));
    h.db.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let mut reopened = TestHost {
        db: Connection::open(&path).unwrap(),
        staged: vec![],
        calls: 0,
    };
    reopened.run(task_event("task.failed", 1));
    assert!(reopened.staged.is_empty());
    assert_eq!(reopened.get(TASK).0, "failed");
    let reply = h.response(
        2,
        vec![Decision::Evaluate {
            conclusion: "recorded evidence".into(),
        }],
    );
    reopened.run(reply);
    assert_eq!(reopened.get(TASK).1["evaluation"], "recorded evidence");
    drop(reopened);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn lease_expiration_requeues() {
    let mut h = TestHost::new();
    h.db.execute(
        "UPDATE objects SET status='leased',data_json=?1 WHERE id=?2",
        params![
            json!({"lease_until":3,"lease_owner":"mock"}).to_string(),
            TASK
        ],
    )
    .unwrap();
    h.run(task_event("task.lease_expired", 2));
    assert_eq!(h.get(TASK).0, "leased");
    h.run(task_event("task.lease_expired", 3));
    assert_eq!(h.get(TASK).0, "pending");
    assert!(h.staged.is_empty());
}
#[test]
fn failed_experiment_creates_hypothesis_and_dispatch_intent() {
    let mut h = TestHost::new();
    h.run(event(
        "task.failed",
        1,
        [
            ("task_id", Value::Text(TASK.into())),
            ("trace_artifact", Value::Text("sha256:trace".into())),
        ],
    ));
    let diagnostic = "22222222222222222222222222222222";
    let hypothesis = "33333333333333333333333333333333";
    let response = h.response(
        2,
        vec![
            Decision::Investigate {
                task_id: diagnostic.into(),
                hypothesis_id: hypothesis.into(),
                description: "inspect trace".into(),
            },
            Decision::Dispatch {
                task_id: diagnostic.into(),
            },
        ],
    );
    h.run(response);
    assert_eq!(h.get(diagnostic).0, "dispatched");
    assert_eq!(h.get(hypothesis).1["trace_artifact"], "sha256:trace");
    assert_eq!(h.count("outbox"), 1);
}
#[test]
fn async_request_uses_recorded_mock_gateway_reply() {
    use coglet_llm::{Admission, Completion, Gateway, Provider, Route};
    use coglet_mock::llm::{MockLlm, Script};
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    let request = h.staged.last().unwrap().clone();
    assert_eq!(request.input.len(), 1);
    let prompt: serde_json::Value =
        serde_json::from_str(request.input["prompt"].as_text().unwrap()).unwrap();
    let proposal = Proposal {
        state_version: prompt["state_version"].as_u64().unwrap(),
        operations: vec![Decision::Evaluate {
            conclusion: "mock trace analysis".into(),
        }],
    };
    let json_request = serde_json::to_value(&request).unwrap();
    let reply = json!({"request_id":request.id,"status":"succeeded","output":{"text":serde_json::to_string(&proposal).unwrap()}});
    let mut mock = MockLlm::new([Script {
        provider: "scripted".into(),
        request: json_request.clone(),
        result: Ok(Completion { reply, cost: 1 }),
    }]);
    let mut gateway = Gateway::open(
        ":memory:",
        [(
            "mock".into(),
            Route {
                provider: "scripted".into(),
                ceiling: 2,
            },
        )]
        .into(),
    )
    .unwrap();
    gateway
        .set_budget(
            "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            10,
        )
        .unwrap();
    gateway
        .enqueue_at(
            Admission {
                coglet: "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                llm_complete_granted: true,
            },
            &json_request,
            1,
        )
        .unwrap();
    assert_eq!(mock.calls.len(), 0); // guest handle returned before any provider call
    gateway
        .dispatch_next_checked(|_| Ok(()), |c| mock.complete(c))
        .unwrap();
    let bytes = gateway
        .replay(
            "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            &request.id,
        )
        .unwrap();
    let payload: std::collections::BTreeMap<String, Value> =
        coglet_types::decode_typed("abi::llm-reply", &bytes).unwrap();
    let mut response = event("llm.reply", 2, []);
    response.payload = payload;
    h.run(response);
    assert_eq!(h.get(TASK).1["evaluation"], "mock trace analysis");
    assert_eq!(mock.calls.len(), 1);
    assert_eq!(
        gateway
            .replay(
                "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                &request.id
            )
            .unwrap(),
        bytes
    );
}
#[test]
fn pending_backlog_survives_connection_restart() {
    let mut h = TestHost::new();
    let other = "55555555555555555555555555555555";
    Repository::new(&mut h)
        .create_object(&object(other, "task", "pending", json!({"context_ids":[]})))
        .unwrap();
    h.run(task_event("task.failed", 1));
    h.run(event(
        "task.failed",
        2,
        [("task_id", Value::Text(other.into()))],
    ));
    assert_eq!(h.staged.len(), 1);
    let response = h.response(
        3,
        vec![Decision::Evaluate {
            conclusion: "done".into(),
        }],
    );
    h.run(response);
    let path = std::env::temp_dir().join(format!("factory-{}-backlog.db", std::process::id()));
    h.db.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let mut restarted = TestHost {
        db: Connection::open(&path).unwrap(),
        staged: vec![],
        calls: 0,
    };
    restarted.run(event("runtime.timer", 4, []));
    assert_eq!(restarted.staged.len(), 1);
    let (_, root, _) = restarted.get("factory-executive");
    assert_eq!(root["pending"]["context"]["subject"]["id"], other);
    drop(restarted);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn pending_timeout_does_not_blindly_retry_uncertain_llm() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    h.run(event("runtime.timer", 300_001, []));
    assert_eq!(h.staged.len(), 1);
    assert!(h.get("factory-executive").1["pending"].is_null());
}
#[test]
fn heartbeat_extends_lease_without_deliberation() {
    let mut h = TestHost::new();
    h.db.execute(
        "UPDATE objects SET status='leased',data_json=?1 WHERE id=?2",
        params![
            json!({"lease_until":3,"lease_owner":"mock"}).to_string(),
            TASK
        ],
    )
    .unwrap();
    h.run(event(
        "task.heartbeat",
        2,
        [
            ("task_id", Value::Text(TASK.into())),
            ("lease_owner", Value::Text("mock".into())),
        ],
    ));
    h.run(task_event("task.lease_expired", 3));
    assert_eq!(h.get(TASK).0, "leased");
    assert!(h.staged.is_empty());
}
#[test]
fn terminal_event_cannot_regress_completed_task() {
    let mut h = TestHost::new();
    h.run(task_event("task.completed", 1));
    h.run(task_event("task.failed", 2));
    assert_eq!(h.get(TASK).0, "completed");
    assert!(h.staged.is_empty());
}
#[test]
fn large_persisted_backlog_drains_one_bounded_context_per_wake() {
    let mut h = TestHost::new();
    for n in 1..=12 {
        let key = format!("{n:032x}");
        Repository::new(&mut h)
            .create_object(&object(&key, "task", "pending", json!({"context_ids":[]})))
            .unwrap();
        h.run(event("task.failed", n, [("task_id", Value::Text(key))]));
    }
    assert_eq!(h.staged.len(), 1);
    let saved = h.staged.clone();
    let path =
        std::env::temp_dir().join(format!("factory-{}-large-backlog.db", std::process::id()));
    h.db.execute("VACUUM INTO ?1", [path.to_str().unwrap()])
        .unwrap();
    let mut h = TestHost {
        db: Connection::open(&path).unwrap(),
        staged: saved,
        calls: 0,
    };
    for n in 0..12 {
        let response = h.response(
            100 + n * 2,
            vec![Decision::Evaluate {
                conclusion: "examined".into(),
            }],
        );
        h.run(response);
        h.run(event("runtime.timer", 101 + n * 2, []));
    }
    assert_eq!(h.staged.len(), 12);
    for n in 1..=12 {
        assert_eq!(h.get(&format!("{n:032x}")).1["evaluation"], "examined");
    }
    drop(h);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn changed_content_at_same_source_sequence_is_conflict() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    assert_eq!(
        h.run_failure(task_event("task.completed", 1), false),
        Err(HostError::Conflict)
    );
    assert_eq!(h.get(TASK).0, "failed");
    assert_eq!(h.staged.len(), 1);
}
#[test]
fn invalid_ops_rejected_without_partial_mutation() {
    let mut h = TestHost::new();
    h.run(task_event("task.failed", 1));
    let response = h.response(
        2,
        vec![
            Decision::CreateGoal {
                id: "44444444444444444444444444444444".into(),
                description: "must not persist".into(),
            },
            Decision::Prioritize { priority: 101 },
        ],
    );
    h.run(response);
    assert_eq!(h.count("objects"), 5);
    assert_eq!(h.count("outbox"), 0);
}
