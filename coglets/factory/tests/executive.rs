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
mod goals;
mod research;
struct TestHost {
    db: Connection,
    staged: Vec<LlmRequest>,
    calls: usize,
    files: std::collections::BTreeMap<String, Vec<u8>>,
    timers: Vec<serde_json::Value>,
    revision: u64,
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
            files: Default::default(),
            timers: vec![],
            revision: 0,
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
        let files_before = self.files.clone();
        let timers_before = self.timers.len();
        let changes_before = self.db.total_changes();
        self.db.execute_batch("BEGIN IMMEDIATE").unwrap();
        let result = coglet_factory::wake(&mut Context::new(self, e.seq, e.logical_time, 3), &[e]);
        if result.is_err() || abort {
            self.db.execute_batch("ROLLBACK").unwrap();
            self.staged.truncate(before);
            self.files = files_before;
            self.timers.truncate(timers_before);
        } else {
            self.db.execute_batch("COMMIT").unwrap();
            if self.db.total_changes() != changes_before {
                self.revision += 1;
            }
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
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.files.get(path).cloned())
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
        self.files.insert(path.into(), bytes.into());
        Ok(())
    }
    fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32 {
        if operation == Operation::Timer {
            let value = coglet_types::decode_profile(bytes).unwrap();
            self.timers.push(serde_json::to_value(value).unwrap());
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
                revision: self.revision,
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
        files: h.files.clone(),
        timers: h.timers.clone(),
        revision: h.revision,
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
    h.db.execute("UPDATE objects SET status='leased' WHERE id=?1", [TASK])
        .unwrap();
    h.db.execute(
        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'leased',0,'mock',3,1,1)",
        [TASK],
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
    h.run(event(
        "task.completed",
        3,
        [("task_id", Value::Text(diagnostic.into()))],
    ));
    assert_eq!(h.get(diagnostic).0, "completed");
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
fn backlog_completes_after_restart() {
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
        files: h.files.clone(),
        timers: h.timers.clone(),
        revision: h.revision,
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
    h.db.execute("UPDATE objects SET status='leased' WHERE id=?1", [TASK])
        .unwrap();
    h.db.execute(
        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'leased',0,'mock',100,1,1)",
        [TASK],
    )
    .unwrap();
    h.run(event(
        "task.heartbeat",
        2,
        [
            ("task_id", Value::Text(TASK.into())),
            ("lease", Value::Text("mock".into())),
            ("attempt", Value::Integer(1.into())),
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
        files: h.files.clone(),
        timers: h.timers.clone(),
        revision: h.revision,
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
                priority: 0,
                budget: 1,
            },
            Decision::Prioritize { priority: 101 },
        ],
    );
    h.run(response);
    assert_eq!(h.count("objects"), 5);
    assert_eq!(h.count("outbox"), 0);
}
#[test]
fn stale_attempt_does_not_release_current_capacity() {
    let mut h = TestHost::new();
    h.db.execute(
        "UPDATE objects SET status='dispatched',data_json=?1 WHERE id=?2",
        params![
            json!({"capacity_reserved": true, "context_ids": []}).to_string(),
            TASK
        ],
    )
    .unwrap();
    h.db.execute(
        "UPDATE objects SET data_json=?1 WHERE id='factory-policy'",
        [serde_json::to_string(&Policy {
            model: "mock".into(),
            remaining_budget: 9,
            available_workers: 1,
            max_priority: 100,
        })
        .unwrap()],
    )
    .unwrap();
    h.db.execute(
        "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'leased',0,'current',100,2,1)",
        [TASK],
    )
    .unwrap();
    h.run(event(
        "task.completed",
        4,
        [
            ("task_id", Value::Text(TASK.into())),
            ("lease", Value::Text("old".into())),
            ("attempt", Value::Integer(1.into())),
        ],
    ));
    assert_eq!(h.get(TASK).0, "dispatched");
    assert_eq!(h.get("factory-policy").1["available_workers"], 1);
}
#[test]
fn cancellation_result_fences_attempt_and_releases_capacity_once() {
    for state in ["leased", "done"] {
        let mut h = TestHost::new();
        h.db.execute(
            "UPDATE objects SET status='dispatched',data_json=?1 WHERE id=?2",
            params![
                json!({"capacity_reserved":true,"context_ids":[]}).to_string(),
                TASK
            ],
        )
        .unwrap();
        h.db.execute(
            "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,?2,0,'current',100,2,1)",
            params![TASK, state],
        ).unwrap();
        let before = h.get(TASK);
        let policy = h.get("factory-policy");
        // Neither a wrong token nor a wrong attempt (nor missing identity) can
        // cancel the currently reserved worker or mutate its task.
        for (seq, token, attempt) in [(1, "old", 2), (2, "current", 1)] {
            h.run(event(
                "task.cancelled",
                seq,
                [
                    ("task_id", Value::Text(TASK.into())),
                    ("lease", Value::Text(token.into())),
                    ("attempt", Value::Integer(attempt.into())),
                ],
            ));
            assert_eq!(h.get(TASK), before);
            assert_eq!(h.get("factory-policy"), policy);
        }
        h.run(task_event("task.cancelled", 3));
        assert_eq!(h.get(TASK), before);
        assert_eq!(h.get("factory-policy"), policy);
        let matching = event(
            "task.cancelled",
            4,
            [
                ("task_id", Value::Text(TASK.into())),
                ("lease", Value::Text("current".into())),
                ("attempt", Value::Integer(2.into())),
            ],
        );
        h.run(matching.clone());
        assert_eq!(h.get(TASK).0, "cancelled");
        assert_eq!(h.get(TASK).1["capacity_reserved"], false);
        assert_eq!(h.get("factory-policy").1["available_workers"], 3);
        let cancelled = h.get(TASK);
        let released = h.get("factory-policy");
        h.run(matching);
        h.run(event(
            "task.cancelled",
            5,
            [
                ("task_id", Value::Text(TASK.into())),
                ("lease", Value::Text("current".into())),
                ("attempt", Value::Integer(2.into())),
            ],
        ));
        assert_eq!(h.get(TASK), cancelled);
        assert_eq!(h.get("factory-policy"), released);
        assert!(h.staged.is_empty());
    }
}

#[test]
fn cancellation_cannot_regress_terminal_tasks() {
    for terminal in ["completed", "failed", "cancelled"] {
        let mut h = TestHost::new();
        h.db.execute(
            "UPDATE objects SET status=?1 WHERE id=?2",
            params![terminal, TASK],
        )
        .unwrap();
        let before = h.get(TASK);
        let policy = h.get("factory-policy");
        h.run(task_event("task.cancelled", 1));
        assert_eq!(h.get(TASK), before);
        assert_eq!(h.get("factory-policy"), policy);
        assert!(h.staged.is_empty());
    }
}

#[test]
fn host_terminal_done_row_is_consumed_and_stale_attempt_rejected() {
    let mut h = TestHost::new();
    h.db.execute(
        "UPDATE objects SET status='dispatched',data_json=?1 WHERE id=?2",
        params![
            json!({"capacity_reserved": true, "context_ids": []}).to_string(),
            TASK
        ],
    )
    .unwrap();
    h.db.execute(
        "UPDATE objects SET data_json=?1 WHERE id='factory-policy'",
        [serde_json::to_string(&Policy {
            model: "mock".into(),
            remaining_budget: 9,
            available_workers: 1,
            max_priority: 100,
        })
        .unwrap()],
    )
    .unwrap();
    let token = "factory-host:1:kremote";
    h.db
        .execute(
            "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'done',0,?2,NULL,1,2)",
            params![TASK, token],
        )
        .unwrap();
    h.run(event(
        "task.completed",
        5,
        [
            ("task_id", Value::Text(TASK.into())),
            ("attempt", Value::Integer(1.into())),
            ("lease", Value::Text(token.into())),
        ],
    ));
    assert_eq!(h.get(TASK).0, "completed");
    assert_eq!(h.get("factory-policy").1["available_workers"], 2);
    let mut stale = TestHost::new();
    stale
        .db
        .execute("UPDATE objects SET status='dispatched' WHERE id=?1", [TASK])
        .unwrap();
    stale
        .db
        .execute(
            "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,'done',0,?2,NULL,1,2)",
            params![TASK, token],
        )
        .unwrap();
    stale.run(event(
        "task.completed",
        6,
        [
            ("task_id", Value::Text(TASK.into())),
            ("attempt", Value::Integer(1.into())),
            ("lease", Value::Text("factory-host:9:other".into())),
        ],
    ));
    assert_eq!(stale.get(TASK).0, "dispatched");
}

#[test]
fn dispatcher_terminal_result_is_consumed_by_executive() {
    use coglet_db::Database;
    use coglet_factory_host::{FactoryHost, MockAdapter, Outcome, WorkerReport};
    const OWNER: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const KEY: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(root.path(), OWNER).unwrap();
    let path = db.path().to_owned();
    drop(db);
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA foreign_keys=ON;").unwrap();
    conn.execute_batch(coglet_factory::SCHEMA_SQL).unwrap();
    conn.execute(
        "INSERT INTO objects(id,type,status,data_json,version,created_at,updated_at) VALUES (?1,'task','dispatched','{\"capacity_reserved\":true}',1,1,1)",
        [TASK],
    )
    .unwrap();
    let payload = coglet_types::encode_profile(&Value::Map(vec![(
        Value::Text("task_id".into()),
        Value::Text(TASK.into()),
    )]))
    .unwrap();
    let hex: String = payload.iter().map(|b| format!("{b:02x}")).collect();
    conn.execute(
        "INSERT INTO outbox(intent_id,idempotency_key,kind,payload_cbor_hex,correlation_id,state,attempt,version) VALUES (?1,?1,'factory.dispatch',?2,?3,'pending',0,1)",
        params![KEY, hex, TASK],
    )
    .unwrap();
    drop(conn);
    let mut db = Database::open(root.path(), OWNER).unwrap();
    let mut host = FactoryHost::from_adapter(MockAdapter::default());
    host.dispatcher().dispatch_pending(&mut db, 10, 8).unwrap();
    let queue = db.get_work_queue(TASK).unwrap().unwrap();
    let follow = host
        .dispatcher()
        .apply_result(
            &mut db,
            KEY,
            WorkerReport {
                attempt: queue.attempt,
                lease: queue.lease_owner.clone().unwrap(),
                outcome: Outcome::Failed {
                    note: "trace says \"failed\"\nline".into(),
                },
            },
        )
        .unwrap();
    let done = db.get_work_queue(TASK).unwrap().unwrap();
    assert_eq!(done.state, "done");
    assert_eq!(done.lease_owner.as_deref(), Some(follow.lease.as_str()));
    assert_eq!(done.attempt, follow.attempt);
    let mut guest = TestHost::new();
    guest
        .db
        .execute(
            "UPDATE objects SET status='dispatched',data_json=?1 WHERE id=?2",
            params![
                json!({"capacity_reserved": true, "context_ids": []}).to_string(),
                TASK
            ],
        )
        .unwrap();
    guest
        .db
        .execute(
            "INSERT INTO work_queue(task_id,state,priority,lease_owner,lease_until,attempt,version) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![
                TASK,
                done.state,
                done.priority,
                done.lease_owner,
                done.lease_until,
                done.attempt as i64,
                done.version as i64
            ],
        )
        .unwrap();
    guest.run(event(
        &follow.kind,
        7,
        [
            ("task_id", Value::Text(follow.task_id)),
            ("attempt", Value::Integer((follow.attempt as i64).into())),
            ("lease", Value::Text(follow.lease)),
        ],
    ));
    assert_eq!(guest.get(TASK).0, "failed");
    assert_eq!(guest.get("factory-policy").1["available_workers"], 3);
    assert_eq!(guest.staged.len(), 1, "failure deliberation must start");
}
