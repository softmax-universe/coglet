//! Durable ABI 0.2 `RuntimeService` path: real WAMR and mock engines, real
//! SQLite, real WAL. Every scenario restarts through disk, never through shared
//! memory. No guest ever runs during recovery.
#![cfg(feature = "native-engine")]
use coglet_db::{
    Catalog, CellType, Column, Database, Grants, Mode, Pins, Recovered, Statement, StepId,
    TypedStatement,
};
use coglet_engine::{Engine as SharedEngine, GuestMemory, StepInput};
use coglet_engine_wamr::Wamr;
use coglet_mock::engine::{ClosureGuest, Context, MockEngine, NativeModule};
use coglet_runtime::database::{DatabaseStep, Deployment, JournalGate, Publication};
use coglet_runtime::{DurableStore, RuntimeService, cbor, digest};
use coglet_types::Value as V;
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

include!("support/database_backup.rs");
include!("support/database_properties.rs");

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
type Plan = Vec<Vec<(&'static str, Vec<u8>)>>;

fn text(s: &str) -> V {
    V::Text(s.into())
}
fn field<'a>(v: &'a V, key: &str) -> &'a V {
    v.as_map()
        .unwrap()
        .iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
        .unwrap()
}
fn catalog() -> Catalog {
    let col = |name: &str| Column {
        name: name.into(),
        kind: CellType::Text,
    };
    Catalog::approve_typed(vec![
        TypedStatement {
            statement: Statement::new(
                "put",
                "INSERT INTO objects(id,value) VALUES (?1,?2)",
                Mode::Write,
                2,
            ),
            parameters: vec![CellType::Text, CellType::Text],
            columns: vec![],
            tables: vec!["objects".into()],
            order: vec![],
        },
        TypedStatement {
            statement: Statement::new(
                "scan",
                "SELECT id,value FROM objects ORDER BY id",
                Mode::Read,
                0,
            ),
            parameters: vec![],
            columns: vec![col("id"), col("value")],
            tables: vec!["objects".into()],
            order: vec!["id".into()],
        },
        TypedStatement {
            statement: Statement::new(
                "stale",
                "UPDATE objects SET value=?1 WHERE id=?2 AND value=?3",
                Mode::Write,
                3,
            )
            .expect_affected(1),
            parameters: vec![CellType::Text, CellType::Text, CellType::Text],
            columns: vec![],
            tables: vec!["objects".into()],
            order: vec![],
        },
        TypedStatement {
            statement: Statement::new(
                "link",
                "INSERT INTO relations(id,parent) VALUES (?1,?2)",
                Mode::Write,
                2,
            ),
            parameters: vec![CellType::Text, CellType::Text],
            columns: vec![],
            tables: vec!["relations".into(), "objects".into()],
            order: vec![],
        },
    ])
    .unwrap()
}
fn statement(name: &str) -> Vec<u8> {
    cbor(&json!({"operation":"statement","statement":name,"mode":if name == "put" {"write"} else {"read"},
        "params":if name == "put" {json!(["a","value"])} else {json!([])},"max_rows":64,"max_bytes":65536}))
    .unwrap()
}
/// Native payload: a uint64 above i64::MAX must not pass through f64/i64.
fn enqueue() -> Vec<u8> {
    let payload = V::Map(vec![
        (text("blob"), text("deadbeef")),
        (text("big"), V::Integer(u64::MAX.into())),
    ]);
    coglet_types::encode(
        "database::db-enqueue",
        &V::Map(vec![
            (text("operation"), text("enqueue")),
            (text("intent_id"), text(&"1".repeat(32))),
            (text("idempotency_key"), text(&"2".repeat(32))),
            (text("kind"), text("worker.run")),
            (text("payload"), payload),
            (text("correlation_id"), text(&"3".repeat(32))),
        ]),
    )
    .unwrap()
}
fn action() -> Vec<u8> {
    cbor(
        &json!({"coglet":C,"id":"f".repeat(32),"capability":"expression.set_expression",
        "args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":50_000_000,
        "correlation_id":"f".repeat(32)}),
    )
    .unwrap()
}
fn plan() -> Plan {
    vec![
        vec![
            ("db", statement("put")),
            ("db", enqueue()),
            ("emit", action()),
            ("db", statement("scan")),
        ],
        vec![("db", statement("scan"))],
    ]
}
fn wasm(steps: &Plan) -> Vec<u8> {
    wasm_returning(steps, "i32.const 0")
}
/// `tail` leaves handle's i32 result on the stack after every recorded call.
fn wasm_returning(steps: &Plan, tail: &str) -> Vec<u8> {
    let (mut data, mut body) = (String::new(), String::new());
    for (i, (op, raw)) in steps[0].iter().enumerate() {
        let ptr = 1024 + i * 2048;
        let escaped = raw.iter().map(|b| format!("\\{b:02x}")).collect::<String>();
        data.push_str(&format!("(data (i32.const {ptr}) \"{escaped}\")"));
        body.push_str(&if *op == "db" {
            format!(
                "i32.const {ptr} i32.const {} i32.const 65536 i32.const 65540 call $db drop\n",
                raw.len()
            )
        } else {
            format!("i32.const {ptr} i32.const {} call $emit drop\n", raw.len())
        });
    }
    wat::parse_str(format!(
        r#"(module
      (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
      (import "coglet_v0" "emit" (func $emit (param i32 i32) (result i32)))
      (memory (export "memory") 3 3) {data}
      (func (export "abi_alloc") (param i32) (result i32) i32.const 160000)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body} {tail}))"#
    ))
    .unwrap()
}

struct Fx {
    root: tempfile::TempDir,
    wal: PathBuf,
    db_path: PathBuf,
    setup: Value,
    manifest: Value,
    grants: Vec<String>,
    code: Vec<u8>,
    cat: Catalog,
}
impl Fx {
    fn new(code: Vec<u8>, mode: &str) -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let cat = catalog();
        let manifest = json!({"version":"0.2","id":C,"subscriptions":[],
            "required_capabilities":["db.read","db.write","expression.set_expression"],
            "exclusive_outputs":[],"budgets":{"compute_mode":mode,"compute":1_000_000,"memory_pages":3,
            "host_bytes":1_000_000,"wall_time_us":30_000_000,"max_events":1},
            "database":{"schema_min":1,"schema_max":1,"catalog_hash":cat.digest().unwrap()}});
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
            .unwrap()["steps"][0]["send"]
            .clone();
        setup["manifest"] = manifest.clone();
        setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
        let raw = hex::decode(
            setup["initial_files"]["/code/build/provenance.cbor"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let mut provenance: Value = coglet_types::decode("hub-build-record", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        provenance["rewritten_sha256"] = json!(digest(&code));
        setup["initial_files"]["/code/build/provenance.cbor"] =
            json!(hex::encode(cbor(&provenance).unwrap()));
        let db = Database::open(&root.path().canonicalize().unwrap(), C).unwrap();
        let db_path = db.path().to_owned();
        let observer = rusqlite::Connection::open(&db_path).unwrap();
        observer
            .execute_batch(
                "CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL);
                 CREATE TABLE relations(id TEXT PRIMARY KEY NOT NULL,parent TEXT NOT NULL REFERENCES objects(id));
                 CREATE TABLE outbox (intent_id TEXT PRIMARY KEY NOT NULL, idempotency_key TEXT NOT NULL UNIQUE,
                   kind TEXT NOT NULL, payload_cbor_hex TEXT NOT NULL, correlation_id TEXT NOT NULL,
                   state TEXT NOT NULL CHECK (state IN ('pending','dispatching','uncertain','succeeded','failed','cancelled')),
                   attempt BIGINT NOT NULL CHECK (attempt >= 0), version BIGINT NOT NULL CHECK (version > 0));
                 PRAGMA user_version=1;",
            )
            .unwrap();
        drop((observer, db));
        Self {
            wal: root.path().join("runtime.wal"),
            root,
            db_path,
            setup,
            manifest,
            grants: [
                "db.read",
                "db.write",
                "worker.run",
                "expression.set_expression",
            ]
            .map(Into::into)
            .to_vec(),
            code,
            cat,
        }
    }
    fn service(&self) -> RuntimeService {
        RuntimeService::open(&self.wal, &self.setup, 4).unwrap()
    }
    fn db(&self) -> Database {
        Database::open(&self.root.path().canonicalize().unwrap(), C).unwrap()
    }
    fn observer(&self) -> rusqlite::Connection {
        rusqlite::Connection::open(&self.db_path).unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        self.observer()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
    fn event(&self, seq: u64) -> Value {
        let mut event = self.setup["inputs"]["events"][0].clone();
        event["seq"] = json!(seq);
        event["correlation_id"] = json!(format!("{seq:032x}"));
        event
    }
    fn context(&self) -> Value {
        json!({"trace_id":self.setup["inputs"]["trace_id"],"logical_time":10,"seed":7,
            "budgets":self.manifest["budgets"],"grants":["db.read","db.write","worker.run","expression.set_expression"],
            "boundary":[]})
    }
    fn step<E: SharedEngine>(
        &self,
        service: &mut RuntimeService,
        db: &mut Database,
        engine: &E,
        gate: Option<JournalGate<'_>>,
    ) -> Result<Option<Value>, String> {
        let (schema_version, schema_hash) = db.schema_pin().unwrap();
        let pins = Pins {
            code_hash: digest(&self.code),
            schema_version,
            schema_hash,
            catalog_hash: self.cat.digest().unwrap(),
            grants: Grants {
                read: true,
                write: true,
            },
            intent_kinds: ["worker.run".to_string()].into(),
        };
        service.run_next_database(
            &self.context(),
            DatabaseStep {
                engine,
                code: &self.code,
                deployment: Deployment {
                    manifest: &self.manifest,
                    grants: &self.grants,
                    pins,
                },
                db,
                catalog: &self.cat,
                runtime_bytes: 8 * 1024 * 1024,
                stack_bytes: 65536,
                gate,
            },
        )
    }
    fn journal(&self, service: &RuntimeService) -> Vec<Vec<u8>> {
        let mut raw = vec![];
        service.export_journal(&mut raw).unwrap();
        let mut frames = vec![];
        let mut rest = &raw[..];
        while !rest.is_empty() {
            let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            frames.push(rest[4..4 + n].to_vec());
            rest = &rest[4 + n..];
        }
        frames
    }
}

fn wamr() -> (Wamr, Fx, Option<Arc<AtomicUsize>>) {
    let code = wasm(&plan());
    (
        Wamr::interpreter().allow_db(),
        Fx::new(code, coglet_engine_wamr::COMPUTE_MODE),
        None,
    )
}
fn mock() -> (MockEngine, Fx, Option<Arc<AtomicUsize>>) {
    mock_with(plan(), false)
}
/// `echo` makes handle return the low byte of the last reply length, so the
/// guest outcome depends on the bytes the host wrote into guest memory.
fn mock_with(plan: Plan, echo: bool) -> (MockEngine, Fx, Option<Arc<AtomicUsize>>) {
    let plan = Arc::new(plan);
    let code = if echo {
        wasm_returning(&plan, ECHO)
    } else {
        wasm(&plan)
    };
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let mut engine = MockEngine::default();
    engine.allow_db();
    engine
        .register(
            code.clone(),
            NativeModule::new(3, 3, move || {
                let (plan, runs) = (plan.clone(), counter.clone());
                Box::new(ClosureGuest(
                    move |_, input: &StepInput, cx: &mut Context<'_>| {
                        runs.fetch_add(1, Ordering::SeqCst);
                        for (i, (op, raw)) in plan[input.step as usize].iter().enumerate() {
                            let ptr = 1024 + i as u32 * 2048;
                            cx.memory.write(ptr, raw)?;
                            let args = if *op == "db" {
                                vec![ptr as u64, raw.len() as u64, 65536, 65540]
                            } else {
                                vec![ptr as u64, raw.len() as u64]
                            };
                            cx.call("coglet_v0", op, &args)?;
                        }
                        if echo {
                            return Ok(u32::from(cx.memory.read(65539, 1)?[0]));
                        }
                        Ok(0)
                    },
                ))
            })
            .unwrap(),
        )
        .unwrap();
    (
        engine,
        Fx::new(code, coglet_mock::engine::COMPUTE_MODE),
        Some(runs),
    )
}
fn guest_runs(runs: &Option<Arc<AtomicUsize>>) -> Option<usize> {
    runs.as_ref().map(|r| r.load(Ordering::SeqCst))
}

/// The enqueue call's payload is exact native CBOR, not a JSON approximation.
fn assert_native_payload(frame: &[u8]) {
    let record = coglet_types::decode("database::db-step-record", frame).unwrap();
    let calls = field(&record, "calls").as_array().unwrap();
    let call = calls
        .iter()
        .find(|c| {
            field(c, "op").as_text() == Some("db")
                && field(field(c, "argument"), "operation").as_text() == Some("enqueue")
        })
        .unwrap();
    let payload = field(field(call, "argument"), "payload");
    assert_eq!(field(payload, "blob").as_text(), Some("deadbeef"));
    assert_eq!(
        field(payload, "big").as_integer().map(u128::try_from),
        Some(Ok(u64::MAX as u128))
    );
    // Calls remain invocation ordered, ordinary and DB calls interleaved.
    let ops: Vec<_> = calls
        .iter()
        .map(|c| field(c, "op").as_text().unwrap())
        .collect();
    assert_eq!(ops, ["db", "db", "emit", "db"]);
}

fn commit_and_reopen<E: SharedEngine>(engine: E, fx: Fx, runs: Option<Arc<AtomicUsize>>) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    let result = fx
        .step(&mut service, &mut db, &engine, None)
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["version"], "0.2");
    assert_eq!(result["journal"]["status"], "committed");
    assert_eq!(service.queued(), 0);
    db.dispatch_fence().unwrap();
    assert_eq!((fx.count("objects"), fx.count("outbox")), (1, 1));
    // Exact bytes: the result's raw record, the WAL export and the marker agree.
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 1);
    assert_eq!(hex::encode(&frames[0]), result["record_raw"]);
    assert_native_payload(&frames[0]);
    let marker: String = fx
        .observer()
        .query_row("SELECT record_hash FROM _coglet_markers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(marker, result["journal"]["record_hash"]);
    // The outbox row holds the same native payload bytes.
    let stored: String = fx
        .observer()
        .query_row("SELECT payload_cbor_hex FROM outbox", [], |r| r.get(0))
        .unwrap();
    assert!(stored.contains("1bffffffffffffffff"), "{stored}");
    // WAL projection is a derived view; the uint64 survives it exactly.
    assert!(
        service.records()[0]
            .to_string()
            .contains("18446744073709551615")
    );
    drop((service, db));

    // Restart: full validation of WAL state against the exact bytes.
    let mut service = fx.service();
    let mut db = fx.db();
    assert!(!service.has_unresolved_attempt());
    assert_eq!(service.queued(), 0);
    assert_eq!(fx.journal(&service), frames);
    assert_eq!(service.records()[0], result["journal"]);
    assert_eq!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Nothing
    );
    assert!(
        fx.step(&mut service, &mut db, &engine, None)
            .unwrap()
            .is_none()
    );
    if runs.is_some() {
        // Continue the chain across the restart with a read-only DB step.
        service.enqueue(fx.event(1)).unwrap();
        let second = fx
            .step(&mut service, &mut db, &engine, None)
            .unwrap()
            .unwrap();
        assert_eq!(
            second["journal"]["previous"],
            result["journal"]["record_hash"]
        );
        assert_eq!(
            second["journal"]["db_before"],
            second["journal"]["db_after"]
        );
        assert_eq!(fx.journal(&service).len(), 2);
        assert_eq!(guest_runs(&runs), Some(2));
    }
    // The ordinary effect is part of the prepared full state.
    assert_eq!(service.records()[0]["actions"].as_array().unwrap().len(), 1);
}

#[test]
fn real_wamr_service_commits_exact_native_record_and_reopens() {
    let (engine, fx, runs) = wamr();
    commit_and_reopen(engine, fx, runs);
}
#[test]
fn mock_service_commits_exact_native_record_and_reopens() {
    let (engine, fx, runs) = mock();
    commit_and_reopen(engine, fx, runs);
}

/// Marker present: the journal is down after the DB commit. Recovery after a
/// restart rolls the exact prepared decision forward; the guest never reruns.
fn marker_present<E: SharedEngine>(engine: E, fx: Fx, runs: Option<Arc<AtomicUsize>>) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    let mut down = |kind: Publication, _: &StepId| -> Result<(), String> {
        assert_eq!(kind, Publication::Committed);
        Err("journal unavailable".into())
    };
    let error = fx
        .step(&mut service, &mut db, &engine, Some(&mut down))
        .unwrap_err();
    assert!(error.starts_with("held:"), "{error}");
    assert!(service.has_unresolved_attempt());
    assert_eq!(service.records().len(), 0, "nothing published yet");
    assert_eq!(
        fx.count("objects"),
        1,
        "DB commit is the decision, not undone"
    );
    assert!(db.dispatch_fence().is_err());
    // A held decision cannot start another step.
    service.enqueue(fx.event(1)).unwrap();
    assert!(
        fx.step(&mut service, &mut db, &engine, None).is_err(),
        "unreconciled decision must block admission"
    );
    drop((service, db));

    let mut service = fx.service();
    let mut db = fx.db();
    assert!(service.has_unresolved_attempt(), "fence survives restart");
    assert_eq!(service.queued(), 2);
    let id = match service.recover_database(&mut db, None).unwrap() {
        Recovered::RolledForward(id) => id,
        other => panic!("{other:?}"),
    };
    assert_eq!(id.step, 0);
    assert!(!service.has_unresolved_attempt());
    assert_eq!(service.queued(), 1);
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 1);
    assert_native_payload(&frames[0]);
    let marker: String = fx
        .observer()
        .query_row("SELECT record_hash FROM _coglet_markers", [], |r| r.get(0))
        .unwrap();
    assert_eq!(marker, service.records()[0]["record_hash"]);
    assert_eq!(service.records()[0]["status"], "committed");
    db.dispatch_fence().unwrap();
    // Idempotent: a second recovery and a third open change nothing.
    assert_eq!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Nothing
    );
    drop((service, db));
    let service = fx.service();
    assert_eq!(fx.journal(&service), frames);
    assert_eq!(service.queued(), 1, "the second event is still queued");
    assert_eq!(guest_runs(&runs).unwrap_or(1), 1, "no guest rerun");
}
#[test]
fn real_wamr_marker_present_recovers_forward_without_guest() {
    let (engine, fx, runs) = wamr();
    marker_present(engine, fx, runs);
}
#[test]
fn mock_marker_present_recovers_forward_without_guest() {
    let (engine, fx, runs) = mock();
    marker_present(engine, fx, runs);
}

const BLOCK_MARKER: &str = "CREATE TRIGGER block_marker BEFORE INSERT ON _coglet_markers
    BEGIN SELECT RAISE(ABORT,'marker store unavailable'); END";

/// Marker absent: the DB commit failed after prepare. Recovery publishes the
/// host-decided abort for the SAME attempt; the guest is not rerun and the
/// retained wakes are consumed with all guest effects discarded.
fn marker_absent<E: SharedEngine>(engine: E, fx: Fx, runs: Option<Arc<AtomicUsize>>) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    fx.observer().execute_batch(BLOCK_MARKER).unwrap();
    let error = fx.step(&mut service, &mut db, &engine, None).unwrap_err();
    assert!(error.starts_with("held:"), "{error}");
    assert!(service.has_unresolved_attempt());
    assert_eq!(service.queued(), 1, "input is retained while held");
    assert_eq!(fx.count("objects"), 0, "unit rolled back with no marker");
    drop((service, db));
    fx.observer()
        .execute_batch("DROP TRIGGER block_marker")
        .unwrap();

    let mut service = fx.service();
    let mut db = fx.db();
    assert!(matches!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Aborted(id) if id.step == 0
    ));
    assert!(!service.has_unresolved_attempt());
    assert_eq!(service.queued(), 0, "attempt wakes consumed");
    assert_eq!(service.failed_attempts(), 1);
    assert_eq!((fx.count("objects"), fx.count("outbox")), (0, 0));
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 1);
    let record = coglet_types::decode("database::db-step-record", &frames[0]).unwrap();
    assert_eq!(field(&record, "status").as_text(), Some("aborted"));
    assert_eq!(
        field(field(&record, "failure"), "kind").as_text(),
        Some("db_recovery_abort")
    );
    assert_eq!(field(&record, "db_before"), field(&record, "db_after"));
    assert_eq!(field(&record, "input_hash"), field(&record, "output_hash"));
    assert_eq!(
        field(&record, "input_files"),
        field(&record, "output_files")
    );
    assert!(field(&record, "actions").as_array().unwrap().is_empty());
    assert_native_payload(&frames[0]); // provisional calls retained verbatim
    db.dispatch_fence().unwrap();
    assert_eq!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Nothing
    );
    drop((service, db));
    let mut service = fx.service();
    let mut db = fx.db();
    assert_eq!(fx.journal(&service), frames);
    assert_eq!(service.queued(), 0);
    if runs.is_some() {
        // The chain continues from the abort record; the dispatch effect of
        // the discarded attempt never existed.
        service.enqueue(fx.event(1)).unwrap();
        let next = fx
            .step(&mut service, &mut db, &engine, None)
            .unwrap()
            .unwrap();
        assert_eq!(next["journal"]["step"], 1);
        assert_eq!(
            next["journal"]["previous"],
            service.records()[0]["record_hash"]
        );
        assert_eq!(guest_runs(&runs), Some(2), "one run per admitted event");
    }
}
#[test]
fn real_wamr_marker_absent_aborts_same_attempt_without_guest() {
    let (engine, fx, runs) = wamr();
    marker_absent(engine, fx, runs);
}
#[test]
fn mock_marker_absent_aborts_same_attempt_without_guest() {
    let (engine, fx, runs) = mock();
    marker_absent(engine, fx, runs);
}

/// An unavailable DB (or journal) during recovery holds: the attempt is neither
/// completed nor lost, and nothing is inferred from the failure.
fn database_unavailable<E: SharedEngine>(engine: E, fx: Fx) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    fx.observer().execute_batch(BLOCK_MARKER).unwrap();
    assert!(
        fx.step(&mut service, &mut db, &engine, None)
            .unwrap_err()
            .starts_with("held:")
    );
    drop((service, db));
    let observer = fx.observer();
    observer.execute_batch("DROP TRIGGER block_marker").unwrap();

    let mut service = fx.service();
    let mut db = fx.db();
    observer
        .execute_batch("ALTER TABLE _coglet_markers RENAME TO _hidden")
        .unwrap();
    let error = service.recover_database(&mut db, None).unwrap_err();
    assert!(error.starts_with("held:"), "{error}");
    assert!(!db.halted());
    assert!(service.has_unresolved_attempt(), "not completed");
    assert_eq!(service.queued(), 1, "not lost");
    assert_eq!(service.records().len(), 0, "no inferred abort");
    // A held owner admits nothing.
    assert!(fx.step(&mut service, &mut db, &engine, None).is_err());
    // A journal outage during recovery is equally held and non-destructive.
    observer
        .execute_batch("ALTER TABLE _hidden RENAME TO _coglet_markers")
        .unwrap();
    let mut down =
        |_: Publication, _: &StepId| -> Result<(), String> { Err("journal down".into()) };
    let error = service
        .recover_database(&mut db, Some(&mut down))
        .unwrap_err();
    assert!(error.starts_with("held:"), "{error}");
    assert_eq!((service.queued(), service.records().len()), (1, 0));
    assert!(matches!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Aborted(_)
    ));
    assert_eq!((service.queued(), service.records().len()), (0, 1));
}
#[test]
fn real_wamr_database_unavailable_holds_and_loses_nothing() {
    let (engine, fx, _) = wamr();
    database_unavailable(engine, fx);
}
#[test]
fn mock_database_unavailable_holds_and_loses_nothing() {
    let (engine, fx, _) = mock();
    database_unavailable(engine, fx);
}

/// ABI 0.1 stays closed: a JSON frame cannot carry a 0.2 record, and the exact
/// byte frame refuses tampering.
#[test]
fn wal_keeps_zero_one_closed_and_database_bytes_exact() {
    let (engine, fx, _) = mock();
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    fx.step(&mut service, &mut db, &engine, None)
        .unwrap()
        .unwrap();
    let frames = fx.journal(&service);
    let projection = service.records()[0].clone();
    drop((service, db));
    let dir = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let mut store = DurableStore::open(&dir.path().join("w.wal")).unwrap();
    assert!(store.commit(&json!({}), Some(&projection)).is_err());
    let mut tampered = frames[0].clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(store.commit_database(&json!({}), &tampered).is_err());
    store.commit_database(&json!({}), &frames[0]).unwrap();
    drop(store);
    let store = DurableStore::open(&dir.path().join("w.wal")).unwrap();
    assert_eq!(store.database_record_bytes(0), Some(&frames[0][..]));
    assert_eq!(store.records()[0], projection);
}

/// A deployment whose manifest or grants differ from the durable runtime's
/// manifest and the attempt's context grants is refused before any fence.
#[test]
fn deployment_pins_must_match_runtime_manifest_and_context_grants() {
    let (engine, fx, _) = mock();
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    let mut fx = fx;
    let good = (fx.manifest.clone(), fx.grants.clone());
    for field in ["catalog_hash", "schema_min"] {
        fx.manifest = good.0.clone();
        fx.manifest["database"][field] = if field == "schema_min" {
            json!(0)
        } else {
            json!("sha256:".to_string() + &"0".repeat(64))
        };
        let error = fx.step(&mut service, &mut db, &engine, None).unwrap_err();
        assert!(error.contains("manifest"), "{field}: {error}");
        assert!(!service.has_unresolved_attempt());
        assert_eq!(service.queued(), 1);
    }
    fx.manifest = good.0.clone();
    fx.grants = good
        .1
        .iter()
        .filter(|g| *g != "db.write")
        .cloned()
        .collect();
    let error = fx.step(&mut service, &mut db, &engine, None).unwrap_err();
    assert!(error.contains("grants"), "{error}");
    assert!(!service.has_unresolved_attempt());
    fx.grants = good.1;
    assert!(
        fx.step(&mut service, &mut db, &engine, None)
            .unwrap()
            .is_some()
    );
}

/// Poison is observed explicitly: a guest returning zero after a typed failed
/// call yields guest_return/conflict, not a trap/unavailable abort.
#[test]
fn poisoned_unit_with_zero_return_is_guest_return_conflict() {
    let steps: Plan = vec![vec![
        ("db", statement("missing")),
        ("db", statement("scan")),
    ]];
    let fx = Fx::new(wasm(&steps), coglet_engine_wamr::COMPUTE_MODE);
    let engine = Wamr::interpreter().allow_db();
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    let result = fx
        .step(&mut service, &mut db, &engine, None)
        .unwrap()
        .unwrap();
    let record = &result["journal"];
    assert_eq!(record["status"], "aborted");
    assert_eq!(record["failure"]["kind"], "guest_return");
    assert_eq!(record["failure"]["error"]["code"], "conflict");
}

/// Post-fence host metadata (clock anchor, new events) survives committed
/// recovery; only the attempt's own prefix is consumed.
#[test]
fn committed_recovery_preserves_post_fence_host_metadata() {
    let (engine, fx, _) = mock();
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    let mut down =
        |_: Publication, _: &StepId| -> Result<(), String> { Err("journal unavailable".into()) };
    assert!(
        fx.step(&mut service, &mut db, &engine, Some(&mut down))
            .unwrap_err()
            .starts_with("held:")
    );
    assert!(service.has_unresolved_attempt());
    service.supervised_now().unwrap();
    service.enqueue(fx.event(1)).unwrap();
    assert!(matches!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::RolledForward(_)
    ));
    assert_eq!(service.queued(), 1);
    drop((service, db));
    let store = DurableStore::open(&fx.wal).unwrap();
    let state = store.state().unwrap();
    assert!(
        !state["clock_anchor"].is_null(),
        "post-fence clock anchor lost"
    );
    drop(store);
    assert_eq!(fx.service().queued(), 1);
}

// ---------------------------------------------------------------------------
// Offline replay (`Runtime::replay_database`): exact recorded native results,
// no database/catalog/cursor, mixed call order, sticky divergence, and the
// recovery-abort overlay only after a reproduced clean success.
// ---------------------------------------------------------------------------
const ECHO: &str = "i32.const 65539 i32.load8_u";
const RECORD: &str = "database::db-step-record";

fn echo_plan() -> Plan {
    vec![vec![("db", statement("put")), ("db", statement("scan"))]]
}
fn wamr_echo() -> (Wamr, Fx) {
    let code = wasm_returning(&echo_plan(), ECHO);
    (
        Wamr::interpreter().allow_db(),
        Fx::new(code, coglet_engine_wamr::COMPUTE_MODE),
    )
}

/// Live-run `events` attempts and return the exact WAL record bytes.
fn live_frames<E: SharedEngine>(engine: &E, fx: &Fx, events: u64) -> Vec<Vec<u8>> {
    let mut db = fx.db();
    let mut service = fx.service();
    for seq in 0..events {
        service.enqueue(fx.event(seq)).unwrap();
        fx.step(&mut service, &mut db, engine, None)
            .unwrap()
            .unwrap();
    }
    fx.journal(&service)
}
/// Marker absent: the exact `db_recovery_abort` record the recovery published.
fn abort_frames<E: SharedEngine>(engine: &E, fx: &Fx) -> Vec<Vec<u8>> {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0)).unwrap();
    fx.observer().execute_batch(BLOCK_MARKER).unwrap();
    assert!(
        fx.step(&mut service, &mut db, engine, None)
            .unwrap_err()
            .starts_with("held:")
    );
    drop((service, db));
    fx.observer()
        .execute_batch("DROP TRIGGER block_marker")
        .unwrap();
    let mut service = fx.service();
    let mut db = fx.db();
    assert!(matches!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Aborted(_)
    ));
    fx.journal(&service)
}
fn replay<E: SharedEngine>(fx: &Fx, engine: &E, frames: &[Vec<u8>]) -> Result<Vec<String>, String> {
    let frames: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    coglet_runtime::Runtime::from_fixture(&fx.setup)
        .unwrap()
        .replay_database(&frames, engine, 8 * 1024 * 1024, 65536)
}
fn entry<'a>(entries: &'a mut [(V, V)], key: &str) -> &'a mut V {
    &mut entries
        .iter_mut()
        .find(|(k, _)| k.as_text() == Some(key))
        .unwrap()
        .1
}
/// Edit a record's native fields and re-seal it with a correct hash.
fn reseal(frame: &[u8], edit: impl FnOnce(&mut Vec<(V, V)>)) -> Vec<u8> {
    let V::Map(mut entries) = coglet_types::decode(RECORD, frame).unwrap() else {
        panic!("record is not a map")
    };
    entries.retain(|(k, _)| k.as_text() != Some("record_hash"));
    edit(&mut entries);
    let hash = digest(&coglet_types::encode_profile(&V::Map(entries.clone())).unwrap());
    entries.push((text("record_hash"), text(&hash)));
    coglet_types::encode(RECORD, &V::Map(entries)).unwrap()
}
fn edit_calls(frame: &[u8], edit: impl FnOnce(&mut Vec<V>)) -> Vec<u8> {
    reseal(frame, |entries| {
        let V::Array(calls) = entry(entries, "calls") else {
            panic!("calls")
        };
        edit(calls);
    })
}
fn call_field<'a>(call: &'a mut V, key: &str) -> &'a mut V {
    let V::Map(entries) = call else {
        panic!("call")
    };
    entry(entries, key)
}
fn drop_response(call: &mut V) {
    let V::Map(entries) = call else {
        panic!("call")
    };
    entries.retain(|(k, _)| k.as_text() != Some("response"));
}
fn assert_divergence(error: Result<Vec<String>, String>, what: &str) {
    let error = error.expect_err(what);
    assert!(
        error.contains("replay divergence") || error.contains("replay mismatch"),
        "{what}: {error}"
    );
}

/// Mixed stream [db put, db enqueue(u64::MAX), emit, db scan]: the recorded
/// results are replayed with NO database present, and every changed, missing,
/// extra or reordered call is a divergence the guest cannot swallow.
fn replay_mixed_order<E: SharedEngine>(engine: E, fx: Fx) {
    let frames = live_frames(&engine, &fx, 2);
    assert_native_payload(&frames[0]);
    let hashes = replay(&fx, &engine, &frames).unwrap();
    assert_eq!(hashes.len(), 2);
    // The database is gone: replay has no way to consult it, and still agrees.
    std::fs::remove_file(&fx.db_path).unwrap();
    assert_eq!(replay(&fx, &engine, &frames).unwrap(), hashes);

    let first = &frames[0];
    let mutated: Vec<(&str, Vec<u8>)> = vec![
        // missing: the guest makes a call the record does not have
        ("missing", edit_calls(first, |c| drop(c.pop()))),
        ("missing-first", edit_calls(first, |c| drop(c.remove(0)))),
        // extra: the record holds a call the guest never makes
        (
            "extra",
            edit_calls(first, |c| {
                let again = c.last().unwrap().clone();
                c.push(again);
            }),
        ),
        // reordered across the ordinary/database boundary
        ("reordered", edit_calls(first, |c| c.swap(1, 2))),
        (
            "changed-statement",
            edit_calls(first, |c| {
                let V::Map(entries) = call_field(&mut c[0], "argument") else {
                    panic!()
                };
                *entry(entries, "statement") = text("scan");
            }),
        ),
        (
            "changed-enqueue-payload",
            edit_calls(first, |c| {
                let V::Map(entries) = call_field(&mut c[1], "argument") else {
                    panic!()
                };
                let V::Map(payload) = entry(entries, "payload") else {
                    panic!()
                };
                *entry(payload, "big") = V::Integer((u64::MAX - 1).into());
            }),
        ),
        (
            "changed-ordinary-argument",
            edit_calls(first, |c| {
                let V::Map(args) = call_field(&mut c[2], "argument") else {
                    panic!()
                };
                let V::Map(argument) = entry(args, "args") else {
                    panic!()
                };
                *entry(argument, "expression") = V::Text("sad".into());
            }),
        ),
        (
            "changed-result",
            edit_calls(first, |c| {
                *call_field(&mut c[0], "result") = V::Integer(4.into());
                drop_response(&mut c[0]);
            }),
        ),
    ];
    for (what, frame) in mutated {
        assert_divergence(replay(&fx, &engine, &[frame]), what);
    }
    // Replay is all or nothing: a divergent second record fails the batch.
    let bad = edit_calls(&frames[1], |c| {
        let again = c.last().unwrap().clone();
        c.push(again);
    });
    assert_divergence(
        replay(&fx, &engine, &[frames[0].clone(), bad]),
        "second record",
    );
    // A record that no longer hashes is refused before any guest runs, and a
    // record cannot be replayed out of its chain position.
    let mut tampered = first.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    assert!(replay(&fx, &engine, &[tampered]).is_err());
    assert!(replay(&fx, &engine, &[frames[1].clone()]).is_err());
}
#[test]
fn real_wamr_replay_enforces_mixed_call_order_without_database() {
    let (engine, fx, _) = wamr();
    replay_mixed_order(engine, fx);
}
#[test]
fn mock_replay_enforces_mixed_call_order_without_database() {
    let (engine, fx, _) = mock();
    replay_mixed_order(engine, fx);
}

/// The guest outcome depends on the reply bytes it reads back from guest
/// memory. Replay must feed it the RECORDED reply: a record whose reply (and
/// consequent guest outcome) is edited consistently replays, an inconsistent
/// edit diverges, and no database exists in either case.
fn replay_uses_recorded_results<E: SharedEngine>(engine: E, fx: Fx) {
    let frames = live_frames(&engine, &fx, 1);
    let record = coglet_types::decode(RECORD, &frames[0]).unwrap();
    assert_eq!(field(&record, "status").as_text(), Some("aborted"));
    let old_message = field(field(field(&record, "failure"), "error"), "message")
        .as_text()
        .unwrap()
        .to_owned();
    let recorded_len = |frame: &[u8]| {
        let record = coglet_types::decode(RECORD, frame).unwrap();
        let last = field(&record, "calls").as_array().unwrap().last().unwrap();
        coglet_types::encode("database::db-reply", field(last, "response"))
            .unwrap()
            .len()
    };
    let old_len = recorded_len(&frames[0]);
    assert_eq!(
        old_message,
        coglet_engine::Error::GuestReturn((old_len & 0xff) as u32).to_string()
    );
    std::fs::remove_file(&fx.db_path).unwrap();
    replay(&fx, &engine, &frames).unwrap();

    // Lengthen the recorded scan reply by 3 bytes (`value` -> `valueXYZ`).
    let longer = |failure_message: Option<String>| {
        reseal(&frames[0], |entries| {
            let V::Array(calls) = entry(entries, "calls") else {
                panic!()
            };
            let V::Map(response) = call_field(calls.last_mut().unwrap(), "response") else {
                panic!()
            };
            let V::Array(rows) = entry(response, "rows") else {
                panic!()
            };
            let V::Array(row) = &mut rows[0] else {
                panic!()
            };
            row[1] = text("valueXYZ");
            if let Some(message) = failure_message {
                // The longer reply is copied (and charged) into guest memory.
                let V::Map(usage) = entry(entries, "usage") else {
                    panic!()
                };
                let bytes = u64::try_from(entry(usage, "host_bytes").as_integer().unwrap());
                *entry(usage, "host_bytes") = V::Integer((bytes.unwrap() + 6).into());
                let V::Map(failure) = entry(entries, "failure") else {
                    panic!()
                };
                let V::Map(error) = entry(failure, "error") else {
                    panic!()
                };
                *entry(error, "message") = text(&message);
            }
        })
    };
    let edited_message =
        coglet_engine::Error::GuestReturn(((old_len + 3) & 0xff) as u32).to_string();
    let consistent = longer(Some(edited_message));
    assert_eq!(recorded_len(&consistent), old_len + 3);
    replay(&fx, &engine, &[consistent]).unwrap();
    assert!(replay(&fx, &engine, &[longer(None)]).is_err());
}
#[test]
fn real_wamr_replay_uses_recorded_results() {
    let (engine, fx) = wamr_echo();
    replay_uses_recorded_results(engine, fx);
}
#[test]
fn mock_replay_uses_recorded_results() {
    let (engine, fx, _) = mock_with(echo_plan(), true);
    replay_uses_recorded_results(engine, fx);
}

/// `db_recovery_abort` is applied only after the guest reproduced handle==0
/// with every recorded call and no poisoned unit.
fn replay_recovery_abort<E: SharedEngine>(engine: E, fx: Fx) {
    let frames = abort_frames(&engine, &fx);
    let record = coglet_types::decode(RECORD, &frames[0]).unwrap();
    assert_eq!(
        field(field(&record, "failure"), "kind").as_text(),
        Some("db_recovery_abort")
    );
    assert_eq!(replay(&fx, &engine, &frames).unwrap().len(), 1);

    let first = &frames[0];
    for (what, frame) in [
        ("missing", edit_calls(first, |c| drop(c.pop()))),
        (
            "extra",
            edit_calls(first, |c| {
                let again = c.last().unwrap().clone();
                c.push(again);
            }),
        ),
        ("reordered", edit_calls(first, |c| c.swap(1, 2))),
        (
            "changed-argument",
            edit_calls(first, |c| {
                let V::Map(entries) = call_field(&mut c[0], "argument") else {
                    panic!()
                };
                *entry(entries, "statement") = text("scan");
            }),
        ),
        (
            "changed-result",
            edit_calls(first, |c| {
                *call_field(&mut c[0], "result") = V::Integer(2.into());
                drop_response(&mut c[0]);
            }),
        ),
    ] {
        assert_divergence(replay(&fx, &engine, &[frame]), what);
    }
    // The abort invariants are checked: the revision cannot advance.
    let advanced = reseal(first, |entries| {
        let before = u64::try_from(entry(entries, "db_before").as_integer().unwrap()).unwrap();
        *entry(entries, "db_after") = V::Integer((before + 1).into());
    });
    assert!(replay(&fx, &engine, &[advanced]).is_err());
}
#[test]
fn real_wamr_replay_applies_recovery_abort_only_after_clean_success() {
    let (engine, fx, _) = wamr();
    replay_recovery_abort(engine, fx);
}
#[test]
fn mock_replay_applies_recovery_abort_only_after_clean_success() {
    let (engine, fx, _) = mock();
    replay_recovery_abort(engine, fx);
}

/// A recovery abort recorded for a guest that does NOT return success (a
/// poisoned unit) is a divergence, not an alternative justification.
fn recovery_abort_for_failed_guest<E: SharedEngine>(engine: E, fx: Fx) {
    let frames = live_frames(&engine, &fx, 1);
    let record = coglet_types::decode(RECORD, &frames[0]).unwrap();
    assert_eq!(
        field(field(&record, "failure"), "kind").as_text(),
        Some("guest_return")
    );
    replay(&fx, &engine, &frames).unwrap();
    let forged = reseal(&frames[0], |entries| {
        *entry(entries, "failure") = V::Map(vec![
            (text("kind"), text("db_recovery_abort")),
            (text("prepared_record_hash"), text(&"0".repeat(64))),
        ]);
    });
    assert_divergence(replay(&fx, &engine, &[forged]), "forged recovery abort");
}
fn poisoned_plan() -> Plan {
    vec![vec![
        ("db", statement("missing")),
        ("db", statement("scan")),
    ]]
}
#[test]
fn real_wamr_recovery_abort_for_poisoned_guest_diverges() {
    let fx = Fx::new(wasm(&poisoned_plan()), coglet_engine_wamr::COMPUTE_MODE);
    recovery_abort_for_failed_guest(Wamr::interpreter().allow_db(), fx);
}
#[test]
fn mock_recovery_abort_for_poisoned_guest_diverges() {
    let (engine, fx, _) = mock_with(poisoned_plan(), false);
    recovery_abort_for_failed_guest(engine, fx);
}

/// Successful `db` then a terminal ABI failure that live does not journal as a
/// second call: overlapping or out-of-range `db` ranges, or an emit whose
/// buffer is not in guest memory. Replay must reproduce `invalid_argument`
/// from the local check, not invent/consume a recorded call. A later successful
/// extra call is still sticky divergence. Whole-record compare must see failure.
fn wasm_put_then(kind: &str) -> Vec<u8> {
    let raw = statement("put");
    let ptr = 1024;
    let escaped = raw.iter().map(|b| format!("\\{b:02x}")).collect::<String>();
    let first = format!(
        "i32.const {ptr} i32.const {} i32.const 65536 i32.const 65540 call $db drop\n",
        raw.len()
    );
    let second = match kind {
        "db-overlap" => format!(
            "i32.const {ptr} i32.const {} i32.const {ptr} i32.const 65540 call $db drop\n",
            raw.len()
        ),
        "db-oor" => format!(
            "i32.const {ptr} i32.const {} i32.const 196608 i32.const 65540 call $db drop\n",
            raw.len()
        ),
        "emit-oor" => "i32.const 196608 i32.const 4 call $emit drop\n".into(),

        other => panic!("unknown follow-up {other}"),
    };
    wat::parse_str(format!(
        r#"(module
      (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
      (import "coglet_v0" "emit" (func $emit (param i32 i32) (result i32)))
      (memory (export "memory") 3 3)
      (data (i32.const {ptr}) "{escaped}")
      (func (export "abi_alloc") (param i32) (result i32) i32.const 160000)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {first}{second} i32.const 0))"#
    ))
    .unwrap()
}
fn mock_put_then(kind: &'static str) -> (MockEngine, Fx) {
    let code = wasm_put_then(kind);
    let put = statement("put");
    let mut engine = MockEngine::default();
    engine.allow_db();
    engine
        .register(
            code.clone(),
            NativeModule::new(3, 3, move || {
                let put = put.clone();
                Box::new(ClosureGuest(
                    move |_, _: &StepInput, cx: &mut Context<'_>| {
                        let ptr = 1024u32;
                        cx.memory.write(ptr, &put)?;
                        cx.call(
                            "coglet_v0",
                            "db",
                            &[ptr as u64, put.len() as u64, 65536, 65540],
                        )?;
                        match kind {
                            "db-overlap" => {
                                cx.call(
                                    "coglet_v0",
                                    "db",
                                    &[ptr as u64, put.len() as u64, ptr as u64, 65540],
                                )?;
                            }
                            "db-oor" => {
                                cx.call(
                                    "coglet_v0",
                                    "db",
                                    &[ptr as u64, put.len() as u64, 196608, 65540],
                                )?;
                            }
                            "emit-oor" => {
                                cx.call("coglet_v0", "emit", &[196608, 4])?;
                            }

                            other => panic!("unknown follow-up {other}"),
                        }
                        Ok(0)
                    },
                ))
            })
            .unwrap(),
        )
        .unwrap();
    (engine, Fx::new(code, coglet_mock::engine::COMPUTE_MODE))
}
fn wamr_put_then(kind: &str) -> (Wamr, Fx) {
    (
        Wamr::interpreter().allow_db(),
        Fx::new(wasm_put_then(kind), coglet_engine_wamr::COMPUTE_MODE),
    )
}

fn replay_terminal_abi_failure<E: SharedEngine>(engine: E, fx: Fx) {
    let frames = live_frames(&engine, &fx, 1);
    let record = coglet_types::decode(RECORD, &frames[0]).unwrap();
    assert_eq!(field(&record, "status").as_text(), Some("aborted"));
    assert_eq!(
        field(field(&record, "failure"), "kind").as_text(),
        Some("invalid_argument")
    );
    let calls = field(&record, "calls").as_array().unwrap();
    assert_eq!(
        calls.len(),
        1,
        "terminal ABI failure must not invent a call"
    );
    assert_eq!(field(&calls[0], "op").as_text(), Some("db"));
    assert_eq!(field(&calls[0], "result").as_integer(), Some(0.into()));
    // Offline: the live record replays without a database.
    std::fs::remove_file(&fx.db_path).unwrap();
    replay(&fx, &engine, &frames).unwrap();

    // Whole-record compare must catch a dropped/changed failure even when calls match.
    let committed = reseal(&frames[0], |entries| {
        *entry(entries, "status") = text("committed");
        entries.retain(|(k, _)| k.as_text() != Some("failure"));
    });
    let error = replay(&fx, &engine, &[committed]).expect_err("dropped failure");
    assert!(
        error.contains("replay mismatch") || error.contains("replay divergence"),
        "failure bypassed whole-record compare: {error}"
    );
    assert!(
        !error.contains("extra"),
        "dropped failure must not be reported as an extra call: {error}"
    );

    // Sticky: a genuine extra successful call still cannot be swallowed.
    let extra = edit_calls(&frames[0], |c| {
        let again = c[0].clone();
        c.push(again);
    });
    assert_divergence(replay(&fx, &engine, &[extra]), "extra successful call");
}

#[test]
fn real_wamr_replay_records_invalid_db_ranges_as_invalid_argument() {
    for kind in ["db-overlap", "db-oor"] {
        let (engine, fx) = wamr_put_then(kind);
        replay_terminal_abi_failure(engine, fx);
    }
}
#[test]
fn mock_replay_records_invalid_db_ranges_as_invalid_argument() {
    for kind in ["db-overlap", "db-oor"] {
        let (engine, fx) = mock_put_then(kind);
        replay_terminal_abi_failure(engine, fx);
    }
}
#[test]
fn real_wamr_replay_records_invalid_ordinary_ranges_as_invalid_argument() {
    let (engine, fx) = wamr_put_then("emit-oor");
    replay_terminal_abi_failure(engine, fx);
}
#[test]
fn mock_replay_records_invalid_ordinary_ranges_as_invalid_argument() {
    let (engine, fx) = mock_put_then("emit-oor");
    replay_terminal_abi_failure(engine, fx);
}
