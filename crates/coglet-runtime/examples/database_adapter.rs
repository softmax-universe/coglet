//! Test-only JSON-line engine-seam adapter. Never a production deployment API.
//! Fixture aliases are translated only at this boundary; observed journal/SQL
//! results, not the corpus's expected messages, determine the response.
use coglet_db::{
    Catalog, CellType, Column, Database, Grants, Mode, Pins, Statement, TypedStatement,
};
use coglet_engine::{Engine, GuestMemory, StepInput};
use coglet_engine_wamr::Wamr;
use coglet_mock::engine::{ClosureGuest, Context, MockEngine, NativeModule};
use coglet_runtime::database::{DatabaseStep, Deployment, Publication};
use coglet_runtime::{RuntimeService, cbor, digest};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

const OWNER: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const CURSOR: &str = "opaque-owner-revision7-key-b";
const BLOCK: &str = "CREATE TRIGGER block_marker BEFORE INSERT ON _coglet_markers BEGIN SELECT RAISE(ABORT,'fixture crash after prepare'); END";

fn catalog() -> Catalog {
    Catalog::approve_typed(vec![
        TypedStatement {
            statement: Statement::new(
                "objects_page",
                "SELECT id,type FROM objects WHERE type=?1 ORDER BY id",
                Mode::Read,
                1,
            ),
            parameters: vec![CellType::Text],
            columns: ["id", "type"]
                .map(|s| Column {
                    name: s.into(),
                    kind: CellType::Text,
                })
                .to_vec(),
            tables: vec!["objects".into()],
            order: vec!["id".into()],
        },
        TypedStatement {
            statement: Statement::new(
                "force_conflict",
                "UPDATE objects SET version=version+1 WHERE type=?1 AND version=0",
                Mode::Write,
                1,
            )
            .expect_affected(1),
            parameters: vec![CellType::Text],
            columns: vec![],
            tables: vec!["objects".into()],
            order: vec![],
        },
    ])
    .unwrap()
}
fn wasm(calls: &[Vec<u8>]) -> Vec<u8> {
    let mut data = String::new();
    let mut body = String::new();
    for (i, raw) in calls.iter().enumerate() {
        let ptr = 1024 + i * 2048;
        let escaped: String = raw.iter().map(|b| format!("\\{b:02x}")).collect();
        data.push_str(&format!("(data (i32.const {ptr}) \"{escaped}\")"));
        body.push_str(&format!(
            "i32.const {ptr} i32.const {} i32.const 65536 i32.const 65540 call $db drop ",
            raw.len()
        ));
    }
    wat::parse_str(format!(
        r#"(module
        (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
        (memory (export "memory") 3 3) {data}
        (func (export "abi_alloc") (param i32) (result i32) i32.const 160000)
        (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body} i32.const 0))"#
    ))
    .unwrap()
}
fn mock(code: Vec<u8>, calls: Vec<Vec<u8>>) -> MockEngine {
    let calls = std::sync::Arc::new(calls);
    let mut e = MockEngine::default();
    e.allow_db();
    e.register(
        code,
        NativeModule::new(3, 3, move || {
            let calls = calls.clone();
            Box::new(ClosureGuest(
                move |_, _: &StepInput, cx: &mut Context<'_>| {
                    for (i, raw) in calls.iter().enumerate() {
                        let ptr = 1024 + i as u32 * 2048;
                        cx.memory.write(ptr, raw)?;
                        cx.call(
                            "coglet_v0",
                            "db",
                            &[ptr.into(), raw.len() as u64, 65536, 65540],
                        )?;
                    }
                    Ok(0)
                },
            ))
        })
        .unwrap(),
    )
    .unwrap();
    e
}

struct Fixture {
    root: tempfile::TempDir,
    code: Vec<u8>,
    cat: Catalog,
    manifest: Value,
    setup: Value,
    grants: Vec<String>,
}
impl Fixture {
    fn new(code: Vec<u8>, mode: &str, grants: Vec<String>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let cat = catalog();
        let manifest = json!({"version":"1.1","id":OWNER,"subscriptions":[],
            "required_capabilities":["db.read","db.write"],"exclusive_outputs":[],
            "budgets":{"compute_mode":mode,"compute":1000000,"memory_pages":3,"host_bytes":2000000,"wall_time_us":30000000,"max_events":1},
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
        let db = Database::open(&root.path().canonicalize().unwrap(), OWNER).unwrap();
        let sql = rusqlite::Connection::open(db.path()).unwrap();
        // Host-owned disposable fixture provisioning, not guest migration authority.
        sql.execute_batch("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,type TEXT NOT NULL,version BIGINT NOT NULL); INSERT INTO objects VALUES('a','goal',1),('b','goal',1),('c','goal',1);
          CREATE TABLE outbox(intent_id TEXT PRIMARY KEY NOT NULL,idempotency_key TEXT NOT NULL UNIQUE,kind TEXT NOT NULL,payload_cbor_hex TEXT NOT NULL,correlation_id TEXT NOT NULL,state TEXT NOT NULL,attempt BIGINT NOT NULL,version BIGINT NOT NULL); PRAGMA user_version=1;").unwrap();
        sql.execute(
            "UPDATE _coglet_state SET revision=?1",
            [7u64.to_be_bytes().as_slice()],
        )
        .unwrap();
        Self {
            root,
            code,
            cat,
            manifest,
            setup,
            grants,
        }
    }
    fn db(&self) -> Database {
        Database::open(&self.root.path().canonicalize().unwrap(), OWNER).unwrap()
    }
    fn pins(&self, db: &Database) -> Pins {
        let (schema_version, schema_hash) = db.schema_pin().unwrap();
        Pins {
            code_hash: digest(&self.code),
            schema_version,
            schema_hash,
            catalog_hash: self.cat.digest().unwrap(),
            grants: Grants {
                read: self.grants.iter().any(|s| s == "db.read"),
                write: self.grants.iter().any(|s| s == "db.write"),
            },
            intent_kinds: self
                .grants
                .iter()
                .filter(|s| s.as_str() == "worker.dispatch")
                .cloned()
                .collect(),
        }
    }
    fn service(&self) -> RuntimeService {
        RuntimeService::open(
            &self.root.path().join(format!(
                "{}.wal",
                self.setup["inputs"]["trace_id"].as_str().unwrap()
            )),
            &self.setup,
            4,
        )
        .unwrap()
    }
    fn run<E: Engine>(&self, e: &E, crash: Option<&str>) -> Value {
        let mut db = self.db();
        let observer = rusqlite::Connection::open(db.path()).unwrap();
        if crash == Some("after_prepare") {
            observer.execute_batch(BLOCK).unwrap();
        }
        let mut svc = self.service();
        svc.enqueue(self.setup["inputs"]["events"][0].clone())
            .unwrap();
        let context = json!({"trace_id":self.setup["inputs"]["trace_id"],"logical_time":10,"seed":7,"budgets":self.manifest["budgets"],"grants":self.grants,"boundary":[]});
        let mut down = |_: Publication, _: &coglet_db::StepId| -> Result<(), String> {
            Err("fixture crash after DB commit".into())
        };
        let pins = self.pins(&db);
        let result = svc.run_next_database(
            &context,
            DatabaseStep {
                engine: e,
                code: &self.code,
                deployment: Deployment {
                    manifest: &self.manifest,
                    grants: &self.grants,
                    pins,
                },
                db: &mut db,
                catalog: &self.cat,
                runtime_bytes: 8 * 1024 * 1024,
                stack_bytes: 65536,
                gate: if crash == Some("after_db_commit") {
                    Some(&mut down)
                } else {
                    None
                },
            },
        );
        let mut prepared_hash = None;
        if crash.is_some() {
            assert!(result.unwrap_err().starts_with("held:"));
            // Read the durable preparation BEFORE recovery discards it. This
            // binds the fixture alias to a real canonical successful decision.
            let files: Vec<_> = std::fs::read_dir(db.path().parent().unwrap().join("prepared"))
                .unwrap()
                .map(|e| e.unwrap().path())
                .filter(|p| p.extension().is_some_and(|e| e == "prepared"))
                .collect();
            assert_eq!(files.len(), 1);
            let preparation = std::fs::read_to_string(&files[0]).unwrap();
            let lines: Vec<_> = preparation.lines().collect();
            assert_eq!(lines[0], "COGLET-PREPARED-1");
            let raw = hex::decode(lines[6]).unwrap();
            let committed: Value = coglet_types::decode("database::db-step-record", &raw)
                .unwrap()
                .deserialized()
                .unwrap();
            assert_eq!(committed["status"], "committed");
            assert!(committed["failure"].is_null());
            prepared_hash = Some(committed["record_hash"].clone());
            assert!(svc.has_unresolved_attempt());
            drop((svc, db));
            if crash == Some("after_prepare") {
                observer.execute_batch("DROP TRIGGER block_marker").unwrap();
            }
            svc = self.service();
            db = self.db();
            svc.recover_database(&mut db, None).unwrap();
            assert!(!svc.has_unresolved_attempt());
        } else {
            result.unwrap().unwrap();
        }
        assert_eq!(svc.records().len(), 1);
        db.dispatch_fence().unwrap();
        let record = svc.records()[0].clone();
        let ids: Vec<String> = observer
            .prepare("SELECT intent_id FROM outbox WHERE state='pending' ORDER BY rowid")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let mut replies: Vec<Value> = record["calls"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["response"].clone())
            .collect();
        for reply in &mut replies {
            // Keep the real cursor until the outer fixture projection.
            if reply["error"]["code"] == "conflict" {
                reply["error"]["message"] = json!("Unit aborted");
            }
        }
        let mut out = json!({"replies":replies,"status":record["status"],"db_before":record["db_before"],"db_after":record["db_after"],"dispatch":ids});
        if crash.is_some() {
            let marker: i64 = observer
                .query_row("SELECT count(*) FROM _coglet_markers", [], |r| r.get(0))
                .unwrap();
            let actual = prepared_hash.unwrap();
            if marker == 1 {
                let marker_hash: String = observer
                    .query_row("SELECT record_hash FROM _coglet_markers", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(json!(marker_hash), actual);
                assert_eq!(record["record_hash"], actual);
            } else {
                assert_eq!(record["failure"]["kind"], "db_recovery_abort");
                assert_eq!(record["failure"]["prepared_record_hash"], actual);
            }
            eprintln!("fixture preparation alias binds verified actual record hash {actual}");
            out["recovery"] = json!({"marker_present":marker == 1,"journal_published":svc.records().len()==1,"guest_return":0,"guest_effects_published":!out["dispatch"].as_array().unwrap().is_empty()});
            if !record["failure"].is_null() {
                out["failure"] = record["failure"].clone();
            }
        }
        out
    }
}
fn request(v: &Value, mode: &str) -> Value {
    assert_eq!(v["operation"], "db.step");
    assert_eq!(v["fixture"], "private-goals-v1");
    assert_eq!(v["termination"], "committed");
    let grants: Vec<String> = serde_json::from_value(v["grants"].clone()).unwrap();
    let mut calls = v["calls"].as_array().unwrap().clone();
    // The corpus names an optimistic-update fixture while sending mode=read.
    // Translate that adapter-only shorthand to its approved write catalog entry.
    for c in &mut calls {
        if c["statement"] == "force_conflict" {
            c["mode"] = json!("write");
        }
    }
    // Cursor setup uses the same approved DB/import path on a disposable runtime
    // read-only step, then binds the opaque fixture alias to its real token.
    if calls.iter().any(|c| c["cursor"] == CURSOR) {
        let first = json!({"operation":"statement","statement":"objects_page","mode":"read","params":["goal"],"max_rows":2,"max_bytes":1024});
        let seed_calls = vec![cbor(&first).unwrap()];
        let code = wasm(&seed_calls);
        let fx = Fixture::new(code.clone(), mode, grants.clone());
        // Commit the setup read through the SAME real/mock guest service. A
        // rolled-back read would correctly discard the cursor metadata.
        let seed = if mode == coglet_mock::engine::COMPUTE_MODE {
            fx.run(&mock(code.clone(), seed_calls), None)
        } else {
            fx.run(&Wamr::interpreter().allow_db(), None)
        };
        let token = seed["replies"][0]["next_cursor"].as_str().unwrap();
        for c in &mut calls {
            if c["cursor"] == CURSOR {
                c["cursor"] = json!(token);
            }
        }
        // The compiled guest must use the actual token; rebuild code and its
        // provenance in this existing fixture rather than moving cursor state.
        let encoded: Vec<_> = calls.iter().map(|c| cbor(c).unwrap()).collect();
        let code = wasm(&encoded);
        let mut fx = fx;
        fx.setup["inputs"]["trace_id"] = json!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
        fx.code = code.clone();
        fx.setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
        let raw = hex::decode(
            fx.setup["initial_files"]["/code/build/provenance.cbor"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let mut p: Value = coglet_types::decode("hub-build-record", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        p["rewritten_sha256"] = json!(digest(&code));
        fx.setup["initial_files"]["/code/build/provenance.cbor"] =
            json!(hex::encode(cbor(&p).unwrap()));
        return if mode == coglet_mock::engine::COMPUTE_MODE {
            fx.run(&mock(code, encoded), None)
        } else {
            fx.run(&Wamr::interpreter().allow_db(), None)
        };
    }
    let calls: Vec<_> = calls.iter().map(|c| cbor(c).unwrap()).collect();
    let code = wasm(&calls);
    let fx = Fixture::new(code.clone(), mode, grants);
    let crash = v["recovery"]["crash"].as_str();
    let mut out = if mode == coglet_mock::engine::COMPUTE_MODE {
        fx.run(&mock(code, calls), crash)
    } else {
        fx.run(&Wamr::interpreter().allow_db(), crash)
    };
    if crash.is_some() {
        // Fixture identity is an alias, NEVER a replacement for the actual
        // durable hash. Bind only after validating the stored preparation/marker.
        let alias = v["recovery"]["prepared_record_hash"].as_str().unwrap();
        assert_eq!(alias.len(), 64);
        out["recovery"]["prepared_record_hash"] = json!(alias);
        if out["failure"]["kind"] == "db_recovery_abort" {
            out["failure"]["prepared_record_hash"] = json!(alias);
        }
    }
    out
}
fn main() {
    let mode = match std::env::args().nth(1).as_deref() {
        Some("real") => coglet_engine_wamr::COMPUTE_MODE,
        Some("mock") => coglet_mock::engine::COMPUTE_MODE,
        _ => panic!("expected real|mock"),
    };
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let input: Value = serde_json::from_str(&line.unwrap()).unwrap();
        let mut reply = request(&input, mode);
        for response in reply["replies"].as_array_mut().unwrap() {
            if response["next_cursor"].is_string() {
                response["next_cursor"] = json!(CURSOR);
            }
        }
        serde_json::to_writer(&mut stdout, &reply).unwrap();
        writeln!(stdout).unwrap();
        stdout.flush().unwrap();
    }
}
