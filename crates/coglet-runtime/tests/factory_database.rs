//! Actual Factory guest (wasm32-wasip1, built from coglets/factory) through
//! the durable ABI 0.2 `RuntimeService`, on real WAMR and on the native mock.
//! Real SQLite, real WAL, operator-approved host migration from the actual
//! `0001.sql`, the actual Factory catalog, and offline recorded-result replay.
//! Test fixtures for operator approval / restore verification are NOT a production
//! approval or authentication boundary.
#![cfg(feature = "native-engine")]
use coglet_db::{
    Catalog, CellType, CodeRange, Column, CompatibilityFloor, Database, Grants, Migration, Mode,
    OperatorApproval, Pins, Recovered, RecoveryBundle, RestoreVerifier, Statement, TypedStatement,
};
use coglet_engine::{Engine as SharedEngine, GuestMemory, StepInput};
use coglet_engine_wamr::Wamr;
use coglet_mock::engine::{ClosureGuest, Context, MockEngine, NativeModule};
use coglet_runtime::database::{DatabaseStep, Deployment};
use coglet_runtime::{RuntimeService, cbor, digest};
use coglet_types::Value as V;
use coglet_types_current as coglet_types;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::OnceLock,
};

// Factory source is compiled in-tree as a module (its nested workspace cannot be a
// dev-dependency); the Wasm guest below is built from the very same files.
#[path = "../../../coglets/factory/src/lib.rs"]
#[allow(dead_code)]
mod factory;
mod factory_dispatch_result;
use factory_dispatch_result::dispatch_result_consumed;
#[rustfmt::skip]
#[path = "../../../spikes/cross-engine-hash/host-wasmtime/src/nancanon.rs"]
#[allow(dead_code)]
mod nancanon;

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const HISTORY: &str = "CREATE TABLE IF NOT EXISTS schema_migrations(version BIGINT PRIMARY KEY NOT NULL, applied_at BIGINT NOT NULL, migration_hash TEXT NOT NULL);";
const OUT: u32 = 131072;

/// Compile the actual guest with the frozen restricted WASI inbox reader.
/// Cached for the whole test binary.
fn factory_wasm() -> &'static (Vec<u8>, u32, u32) {
    static WASM: OnceLock<(Vec<u8>, u32, u32)> = OnceLock::new();
    WASM.get_or_init(|| {
        let manifest =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../coglets/factory/Cargo.toml");
        let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/factory-unknown");
        let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
            .args([
                "build",
                "--locked",
                "-j2",
                "--release",
                "--target",
                "wasm32-wasip1",
            ])
            .arg("--manifest-path")
            .arg(&manifest)
            .env("CARGO_TARGET_DIR", &target)
            .env(
                "RUSTFLAGS",
                "-C link-arg=--max-memory=16777216 -C strip=symbols",
            )
            .status()
            .unwrap();
        assert!(status.success(), "factory guest build failed");
        let code = std::fs::read(target.join("wasm32-wasip1/release/coglet_factory.wasm")).unwrap();
        let (mut imports, mut exports, mut mem) = (vec![], vec![], (0u32, 0u32));
        for payload in wasmparser::Parser::new(0).parse_all(&code) {
            match payload.unwrap() {
                wasmparser::Payload::ImportSection(reader) => {
                    for import in reader.into_imports() {
                        let import = import.unwrap();
                        imports.push((import.module.to_owned(), import.name.to_owned()));
                    }
                }
                wasmparser::Payload::ExportSection(reader) => {
                    for e in reader {
                        exports.push(e.unwrap().name.to_owned());
                    }
                }
                wasmparser::Payload::MemorySection(reader) => {
                    for m in reader {
                        let m = m.unwrap();
                        mem = (m.initial as u32, m.maximum.expect("bounded memory") as u32);
                    }
                }
                _ => {}
            }
        }
        // Engine admission validates the restricted WASI signatures and forbids
        // ambient time/random/network.
        assert!(!imports.is_empty());
        for (module, name) in &imports {
            if module == "wasi_snapshot_preview1" {
                continue;
            }
            assert_eq!(module, "coglet_v0", "unexpected import {module}.{name}");
            assert!(
                ["db", "emit", "timer", "llm", "log", "blob"].contains(&name.as_str()),
                "unexpected import {name}"
            );
        }
        assert!(imports.contains(&("coglet_v0".into(), "db".into())));
        for required in ["memory", "abi_alloc", "handle"] {
            assert!(
                exports.iter().any(|e| e == required),
                "missing export {required}"
            );
        }
        let (code, _) = nancanon::canonicalize(&code).unwrap();
        (code, mem.0, mem.1)
    })
}

/// The actual Factory catalog data, exactly as `coglets/factory/src/catalog.rs`.
fn catalog() -> Catalog {
    let kind = |s| match s {
        "text" => CellType::Text,
        "int64" => CellType::Integer,
        "nullable_text" => CellType::NullableText,
        "nullable_int64" => CellType::NullableInteger,
        _ => panic!("unknown Factory catalog type"),
    };
    Catalog::approve_typed(
        factory::catalog::STATEMENTS
            .iter()
            .map(|s| {
                let mode = if s.mode == "read" {
                    Mode::Read
                } else {
                    Mode::Write
                };
                let mut statement = Statement::new(s.id, s.sql, mode, s.parameters.len());
                if let Some(n) = s.expected_affected {
                    statement = statement.expect_affected(n);
                }
                TypedStatement {
                    statement,
                    parameters: s.parameters.iter().map(|t| kind(*t)).collect(),
                    columns: s
                        .columns
                        .iter()
                        .map(|(name, t)| Column {
                            name: (*name).into(),
                            kind: kind(*t),
                        })
                        .collect(),
                    tables: std::iter::once(s.table)
                        .chain(s.read_dependencies.iter().copied())
                        .map(str::to_string)
                        .collect(),
                    order: s.order.iter().map(|s| (*s).into()).collect(),
                }
            })
            .collect(),
    )
    .unwrap()
}

struct Approval(String);
impl OperatorApproval for Approval {
    fn verify_migration(
        &mut self,
        _: &Migration,
        digest: &str,
        proof: &[u8],
    ) -> Result<(), String> {
        if digest == self.0 && proof == b"authenticated-operator-migration-approval" {
            Ok(())
        } else {
            Err("operator approval denied".into())
        }
    }
}
struct Replay;
impl RestoreVerifier for Replay {
    fn verify_replay_and_references(
        &mut self,
        owner: &str,
        b: &RecoveryBundle,
    ) -> Result<(), String> {
        if owner == C && b.decisions.is_empty() && b.journal_head == "a".repeat(64) {
            Ok(())
        } else {
            Err("replay/head/reference verification failed".into())
        }
    }
}

/// Host-owned migration SQL: the actual `0001.sql` minus `schema_migrations`, which
/// the migration runner itself creates and records (guest never executes DDL).
fn migration_sql() -> String {
    let (head, _) = factory::SCHEMA_SQL
        .split_once("CREATE TABLE schema_migrations")
        .unwrap();
    head.trim().to_owned()
}
fn target_schema_hash() -> String {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let db = Database::open(&root.path().canonicalize().unwrap(), C).unwrap();
    let path = db.path().to_owned();
    drop(db);
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(&format!(
            "{}\n{HISTORY}\nPRAGMA user_version=1;",
            migration_sql()
        ))
        .unwrap();
    Database::open(&root.path().canonicalize().unwrap(), C)
        .unwrap()
        .schema_pin()
        .unwrap()
        .1
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
    migration: Migration,
}
const GRANTS: [&str; 3] = ["db.read", "db.write", "factory.work"];
impl Fx {
    fn new(mode: &str) -> Self {
        let (code, _, max_pages) = factory_wasm().clone();
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let cat = catalog();
        let manifest = json!({"version":"1.1","id":C,"subscriptions":[],
            "required_capabilities":["db.read","db.write"],
            "exclusive_outputs":[],"budgets":{"compute_mode":mode,"compute":2_000_000_000u64,
            "memory_pages":max_pages,"host_bytes":4_000_000,"wall_time_us":60_000_000,"max_events":1},
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
        let mut provenance: Value = ::coglet_types::decode("hub-build-record", &raw)
            .unwrap()
            .deserialized()
            .unwrap();
        provenance["rewritten_sha256"] = json!(digest(&code));
        setup["initial_files"]["/code/build/provenance.cbor"] =
            json!(hex::encode(cbor(&provenance).unwrap()));
        let mut db = Database::open(&root.path().canonicalize().unwrap(), C).unwrap();
        let db_path = db.path().to_owned();
        let (from_version, from_schema_hash) = db.schema_pin().unwrap();
        let range = CodeRange {
            code_hash: digest(&code),
            min: 1,
            max: 1,
        };
        let migration = Migration {
            owner: C.into(),
            from_version,
            from_schema_hash,
            to_version: 1,
            to_schema_hash: target_schema_hash(),
            sql: migration_sql(),
            compatibility: CompatibilityFloor { previous: range.clone(), current: range },
            recovery_plan: "Retain verified snapshot; stop dispatch; repair in isolation; never rewind external effects".into(),
        };
        let proof = b"authenticated-operator-migration-approval";
        db.migrate(
            &migration,
            proof,
            &mut Approval(migration.digest().unwrap()),
            &root.path().join("backup"),
            &root.path().join("trial"),
            &RecoveryBundle {
                journal_head: "a".repeat(64),
                catalog_hash: cat.digest().unwrap(),
                decisions: vec![],
                blobs: BTreeMap::new(),
            },
            &mut Replay,
        )
        .unwrap();
        assert_eq!(
            db.schema_pin().unwrap(),
            (1, migration.to_schema_hash.clone())
        );
        drop(db);
        // Host-side fixture rows (guest cannot create relations; catalog has no such statement).
        let observer = rusqlite::Connection::open(&db_path).unwrap();
        observer
            .execute_batch(
                "PRAGMA foreign_keys=ON;
            INSERT INTO objects VALUES('G1','goal','open','{}',1,10,10);
            INSERT INTO objects VALUES('T1','task','ready','{}',1,10,10);
            INSERT INTO objects VALUES('T2','task','ready','{}',1,10,10);
            INSERT INTO relations VALUES('R1','G1','T1','depends','{}',1);
            INSERT INTO relations VALUES('R2','G1','T2','depends','{}',1);",
            )
            .unwrap();
        Self {
            wal: root.path().join("runtime.wal"),
            root,
            db_path,
            setup,
            manifest,
            grants: GRANTS.map(Into::into).to_vec(),
            code,
            cat,
            migration,
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
    fn object(&self, id: &str) -> (String, i64, i64) {
        self.observer()
            .query_row(
                "SELECT status,version,updated_at FROM objects WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap()
    }
    /// `factory.transact` event; `seq` also derives distinct outbox identity.
    fn event(&self, seq: u64, event_id: &str, new_status: &str) -> Value {
        let mut event = self.setup["inputs"]["events"][0].clone();
        event["seq"] = json!(seq);
        event["correlation_id"] = json!(format!("{seq:032x}"));
        event["kind"] = json!("factory.transact");
        event["payload"] = json!({"id":"G1","at":20+seq,"query_type":"task","query_status":"ready",
            "relation_type":"depends","new_status":new_status,"event_id":event_id,
            "intent_id":format!("{:032x}",seq+1),"idempotency_key":format!("{:032x}",seq+0x100),
            "kind":"factory.work","correlation_id":format!("{seq:032x}")});
        event
    }
    fn context(&self) -> Value {
        json!({"trace_id":self.setup["inputs"]["trace_id"],"logical_time":10,"seed":7,
            "budgets":self.manifest["budgets"],"grants":self.grants,"boundary":[]})
    }
    fn step<E: SharedEngine>(
        &self,
        service: &mut RuntimeService,
        db: &mut Database,
        engine: &E,
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
            intent_kinds: self
                .grants
                .iter()
                .filter(|g| g.starts_with("factory."))
                .cloned()
                .collect(),
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
                runtime_bytes: 32 * 1024 * 1024,
                stack_bytes: 65536,
                gate: None,
            },
        )
    }
    fn journal(&self, service: &RuntimeService) -> Vec<Vec<u8>> {
        let mut raw = vec![];
        service.export_journal(&mut raw).unwrap();
        let (mut frames, mut rest) = (vec![], &raw[..]);
        while !rest.is_empty() {
            let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
            frames.push(rest[4..4 + n].to_vec());
            rest = &rest[4 + n..];
        }
        frames
    }
}

fn field<'a>(v: &'a V, key: &str) -> &'a V {
    v.as_map()
        .unwrap()
        .iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
        .unwrap()
}
/// Ordered `(op, statement-or-operation)` of every recorded guest call.
fn recorded_calls(frame: &[u8]) -> Vec<(String, String)> {
    let record = coglet_types::decode("database::db-step-record", frame).unwrap();
    field(&record, "calls")
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            let op = field(c, "op").as_text().unwrap().to_owned();
            let arg = field(c, "argument");
            let name = if op == "db" {
                let key = if field(arg, "operation").as_text() == Some("enqueue") {
                    "operation"
                } else {
                    "statement"
                };
                field(arg, key).as_text().unwrap().to_owned()
            } else {
                String::new()
            };
            (op, name)
        })
        .collect()
}

fn scenario<E: SharedEngine>(engine: E, fx: Fx) {
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(fx.event(0, "E1", "active")).unwrap();
    let result = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(result["journal"]["version"], "1.1");
    assert_eq!(result["journal"]["status"], "committed");
    db.dispatch_fence().unwrap();
    // Real DB effects of get/query/relations/update/append/enqueue in one commit.
    assert_eq!(fx.object("G1"), ("active".into(), 2, 20));
    assert_eq!((fx.count("events"), fx.count("outbox")), (1, 1));
    let (kind, payload): (String, String) = fx
        .observer()
        .query_row("SELECT kind,payload_cbor_hex FROM outbox", [], |r| {
            Ok((r.get(0)?, r.get(1)?))
        })
        .unwrap();
    assert_eq!(kind, "factory.work");
    let payload = coglet_types::decode_profile(&hex::decode(payload).unwrap()).unwrap();
    // Two peers and two relations were really read, then reported in the payload.
    assert_eq!(
        field(&payload, "peers").as_integer().map(i128::from),
        Some(2)
    );
    assert_eq!(
        field(&payload, "relations").as_integer().map(i128::from),
        Some(2)
    );
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 1);
    let calls: Vec<_> = recorded_calls(&frames[0])
        .into_iter()
        .map(|(o, n)| format!("{o}:{n}"))
        .collect();
    let expected = vec![
        "db:objects_get",
        "db:objects_query",
        "db:relations_get",
        "db:objects_update",
        "db:events_append",
        "db:enqueue",
        "log:",
    ];
    assert_eq!(calls, expected);

    // Failing step: event id collides, error propagates from handle, whole unit rolls back.
    service.enqueue(fx.event(1, "E1", "done")).unwrap();
    let failed = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(failed["journal"]["status"], "aborted");
    assert!(
        failed.to_string().contains("invalid_argument"),
        "duplicate event id must surface the host rejection"
    );
    assert_eq!(
        fx.object("G1"),
        ("active".into(), 2, 20),
        "update rolled back"
    );
    assert_eq!((fx.count("events"), fx.count("outbox")), (1, 1));
    let frames = fx.journal(&service);
    assert_eq!(frames.len(), 2);
    let aborted = coglet_types::decode("database::db-step-record", &frames[1]).unwrap();
    let rejected_call = field(&aborted, "calls").as_array().unwrap().last().unwrap();
    assert_eq!(field(rejected_call, "op").as_text(), Some("db"));
    assert_eq!(
        field(field(rejected_call, "argument"), "statement").as_text(),
        Some("events_append")
    );
    assert_eq!(
        field(field(rejected_call, "response"), "status").as_text(),
        Some("failed")
    );

    // Restart through disk; recovery has nothing to do and runs no guest.
    drop((service, db));
    let mut service = fx.service();
    let mut db = fx.db();
    assert_eq!(fx.journal(&service), frames);
    assert_eq!(
        service.recover_database(&mut db, None).unwrap(),
        Recovered::Nothing
    );
    assert!(fx.step(&mut service, &mut db, &engine).unwrap().is_none());
    // Chain continues after reopen with a fresh unique event.
    service.enqueue(fx.event(2, "E3", "done")).unwrap();
    let third = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(third["journal"]["status"], "committed");
    assert_eq!(fx.object("G1"), ("done".into(), 3, 22));
    let frames = fx.journal(&service);
    let trace_id = fx.setup["inputs"]["trace_id"].clone();
    let archive = service.export_trace(&trace_id, 23).unwrap();
    let imported = coglet_runtime::Runtime::import_trace(&archive).unwrap();
    assert_eq!(imported.export_trace(&trace_id).unwrap(), archive);
    // No guest step at this cutoff: the versioned seal and nonzero observed
    // DB revision must survive WAL reopen and an empty successor segment.
    let change = json!({"actor":format!("hub:{}","3".repeat(64)),"target":C,
        "audit_id":"d".repeat(32),"ticket":null,"method":"debug.write",
        "path":"/data/archive-cutoff","before":null,"after":"31"});
    service.seal_boundary(&trace_id, 23, &[change]).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&fx.wal, &fx.setup, 8).unwrap();
    let sealed = service.export_trace(&trace_id, 23).unwrap();
    assert_eq!(sealed["trailing_boundary"]["version"], "1.1");
    assert_eq!(
        coglet_runtime::Runtime::import_trace(&sealed)
            .unwrap()
            .export_trace(&trace_id)
            .unwrap(),
        sealed
    );
    let next_id = json!("e".repeat(32));
    service.start_segment(&next_id).unwrap();
    drop(service);
    let mut service = RuntimeService::open(&fx.wal, &fx.setup, 8).unwrap();
    let empty = service.export_trace(&next_id, 23).unwrap();
    assert_eq!(empty["initial_db_revision"], third["journal"]["db_after"]);
    assert_eq!(empty["final_db_revision"], third["journal"]["db_after"]);
    assert!(empty["records"].as_array().unwrap().is_empty());
    assert_eq!(service.export_segment(&trace_id).unwrap(), sealed);
    drop((service, db));

    // Offline replay from recorded results alone: no database file exists.
    std::fs::remove_file(&fx.db_path).unwrap();
    let refs: Vec<&[u8]> = frames.iter().map(Vec::as_slice).collect();
    coglet_runtime::Runtime::from_fixture(&fx.setup)
        .unwrap()
        .replay_database(&refs, &engine, 32 * 1024 * 1024, 65536)
        .unwrap();
    assert!(!fx.db_path.exists(), "replay must not touch a database");
    assert_eq!(fx.migration.to_version, 1);
}

#[test]
fn real_wamr_runs_actual_factory_guest_end_to_end() {
    let engine = Wamr::interpreter().allow_db();
    scenario(engine, Fx::new(coglet_engine_wamr::COMPUTE_MODE));
}

/// Mock path: drives the actual Factory repository API (`transact_repository`)
/// through the mock's real `coglet_v0.db` import into the real `DatabaseHost`.
struct MockDb<'a, 'b> {
    cx: &'a mut Context<'b>,
    fatal: Option<coglet_engine::Error>,
}
impl coglet_guest_rust::Host for MockDb<'_, '_> {
    fn call(&mut self, operation: coglet_guest_rust::Operation, bytes: &[u8]) -> i32 {
        use coglet_guest_rust::Operation;
        let name = match operation {
            Operation::Timer => "timer",
            Operation::Emit => "emit",
            Operation::Llm => "llm",
            Operation::Log => "log",
        };
        let result = self
            .cx
            .memory
            .write(4096, bytes)
            .and_then(|()| self.cx.call("coglet_v0", name, &[4096, bytes.len() as u64]));
        match result {
            Ok(rc) => rc as i32,
            Err(error) => {
                self.fatal = Some(error);
                -1
            }
        }
    }
}
impl factory::DbHost for MockDb<'_, '_> {
    fn db(&mut self, request: &[u8], output: &mut [u8]) -> i32 {
        let run = (|| {
            self.cx.memory.write(4096, request)?;
            let rc = self.cx.call(
                "coglet_v0",
                "db",
                &[4096, request.len() as u64, OUT as u64, output.len() as u64],
            )?;
            use coglet_engine::GuestMemory;
            output.copy_from_slice(&self.cx.memory.read(OUT, output.len() as u32)?);
            Ok::<_, coglet_engine::Error>(rc as i32)
        })();
        match run {
            Ok(rc) => rc,
            Err(e) => {
                self.fatal = Some(e);
                -1
            }
        }
    }
}
fn mock() -> (MockEngine, Fx) {
    let (code, ..) = factory_wasm().clone();
    let mut engine = MockEngine::default();
    engine.allow_db();
    engine
        .register(
            code,
            NativeModule::new(4, 4, move || {
                Box::new(ClosureGuest(
                    move |_, input: &StepInput, cx: &mut Context<'_>| {
                        let events: Vec<coglet_guest_rust::Event> =
                            coglet_types::decode_profile(&input.events)
                                .unwrap()
                                .deserialized()
                                .unwrap();
                        let mut host = MockDb { cx, fatal: None };
                        let outcome = factory::wake(
                            &mut coglet_guest_rust::Context::new(
                                &mut host,
                                input.step,
                                input.logical_time,
                                input.seed,
                            ),
                            &events,
                        );
                        if let Some(fatal) = host.fatal.take() {
                            return Err(fatal);
                        }
                        Ok(if outcome.is_ok() { 0 } else { 1 })
                    },
                ))
            })
            .unwrap(),
        )
        .unwrap();
    (engine, Fx::new(coglet_mock::engine::COMPUTE_MODE))
}
#[test]
fn mock_runs_factory_repository_against_real_database_host() {
    let (engine, fx) = mock();
    scenario(engine, fx);
}

/// Actual guest executive: task.failed stages an LLM request and the failed
/// status survives dropping the runtime. The reply arrives as runtime.inbox,
/// which this database harness cannot yet inject; dispatch consumption is
/// proved on the host dispatcher tests.
#[test]
fn wasm_task_failed_stages_llm_and_survives_reopen() {
    let engine = Wamr::interpreter().allow_db();
    let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
    fx.grants.push("llm.complete".into());
    fx.grants.push("factory.dispatch".into());
    let policy = serde_json::to_string(&factory::executive::Policy {
        model: "mock".into(),
        remaining_budget: 9,
        available_workers: 2,
        max_priority: 100,
    })
    .unwrap();
    rusqlite::Connection::open(&fx.db_path)
        .unwrap()
        .execute(
            "INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
            [policy],
        )
        .unwrap();
    let mut event = fx.event(0, "Efail", "active");
    event["kind"] = json!("task.failed");
    event["payload"] = json!({"task_id":"T1","trace_artifact":"sha256:trace"});
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(event).unwrap();
    let result = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(result["journal"]["status"], "committed");
    assert!(
        !result["journal"]["llm_requests"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    drop(service);
    drop(db);
    let _ = engine;
    assert_eq!(fx.object("T1").0, "failed");
}

/// Router authentication is supplied by the embedding fixture, as for the
/// existing migration approval fixture; this tests the actual boundary, not a
/// second direct llm.reply enqueue.
fn reply_scenario(stale: bool, duplicate: bool, dispatch: bool) {
    let engine = Wamr::interpreter().allow_db();
    let mut fx = Fx::new(coglet_engine_wamr::COMPUTE_MODE);
    fx.grants.push("llm.complete".into());
    fx.grants.push("factory.dispatch".into());
    fx.observer()
        .execute(
            "INSERT INTO objects VALUES('factory-policy','policy','active',?1,1,10,10)",
            [serde_json::to_string(&factory::executive::Policy {
                model: "mock".into(),
                remaining_budget: 9,
                available_workers: 2,
                max_priority: 100,
            })
            .unwrap()],
        )
        .unwrap();
    let mut event = fx.event(0, "Efail", "active");
    event["kind"] = json!("task.failed");
    event["payload"] = json!({"task_id":"T1","trace_artifact":"sha256:trace"});
    let mut db = fx.db();
    let mut service = fx.service();
    service.enqueue(event).unwrap();
    let first = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(first["journal"]["status"], "committed");
    let request = &first["journal"]["llm_requests"][0];
    let prompt: Value = serde_json::from_str(request["input"]["prompt"].as_str().unwrap()).unwrap();
    let proposal = json!({"state_version":prompt["state_version"],"operations":[
        {"op":"INVESTIGATE","task_id":"11111111111111111111111111111111",
        "hypothesis_id":"22222222222222222222222222222222","description":"inspect trace"},
        {"op":"DISPATCH","task_id":"11111111111111111111111111111111"}]});
    use coglet_llm::{Admission, Completion, Gateway, Provider, Route};
    use coglet_mock::llm::{MockLlm, Script};
    let routes = [(
        "mock".into(),
        Route {
            provider: "scripted".into(),
            ceiling: 2,
        },
    )]
    .into();
    let gateway_path = fx.root.path().join("gateway.sqlite");
    let mut gateway = Gateway::open(gateway_path.to_str().unwrap(), routes).unwrap();
    gateway.set_budget(C, 10).unwrap();
    gateway
        .enqueue_at(
            Admission {
                coglet: C,
                llm_complete_granted: true,
            },
            request,
            1,
        )
        .unwrap();
    drop(gateway); // reopen the committed pending gateway call before dispatch
    let mut gateway = Gateway::open(
        gateway_path.to_str().unwrap(),
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
    let mut mock = MockLlm::new([Script {
        provider: "scripted".into(),
        request: request.clone(),
        result: Ok(Completion {
            cost: 1,
            reply: json!({"request_id":request["id"],"status":"succeeded","output":{"text":proposal.to_string()}}),
        }),
    }]);
    gateway
        .dispatch_next_checked(|_| Ok(()), |call| mock.complete(call))
        .unwrap();
    assert_eq!(mock.calls.len(), 1);
    drop(gateway); // replay a durable reply without a second provider call
    let gateway = Gateway::open(
        gateway_path.to_str().unwrap(),
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
    let payload = coglet_types::decode(
        "abi::llm-reply",
        &gateway.replay(C, request["id"].as_str().unwrap()).unwrap(),
    )
    .unwrap();
    if stale {
        fx.observer()
            .execute("UPDATE objects SET version=version+1 WHERE id='T1'", [])
            .unwrap();
    }
    let mut reply = fx.event(1, "reply", "active");
    reply["kind"] = json!("llm.reply");
    reply["correlation_id"] = request["correlation_id"].clone();
    reply["payload"] = payload.deserialized().unwrap();
    let message = json!({"index":0,"id":"33333333333333333333333333333333","sender":reply["source"],"event":reply});
    let after = hex::encode(format!("{}\n", message));
    drop((service, db));
    let mut service = fx.service();
    service.seal_boundary(&fx.setup["inputs"]["trace_id"], 10, &[json!({
        "actor":format!("hub:{}","a".repeat(64)),"audit_id":"44444444444444444444444444444444",
        "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl","before":null,"after":after
    })]).unwrap();
    let next = json!("66666666666666666666666666666666");
    service.start_segment(&next).unwrap();
    fx.setup["inputs"]["trace_id"] = next;
    drop(service);
    let mut service = fx.service();
    let mut db = fx.db();
    let replay_start = service
        .export_trace(&fx.setup["inputs"]["trace_id"], 10)
        .unwrap();
    let result = fx.step(&mut service, &mut db, &engine).unwrap().unwrap();
    assert_eq!(
        result["journal"]["status"], "committed",
        "{:?}",
        result["journal"]["failure"]
    );
    if stale {
        assert_eq!(fx.count("outbox"), 0);
        assert_eq!(
            fx.observer()
                .query_row(
                    "SELECT count(*) FROM objects WHERE type='hypothesis'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            fx.observer()
                .query_row(
                    "SELECT count(*) FROM events WHERE event_type='decision.stale'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert_eq!(
            result["journal"]["llm_requests"].as_array().unwrap().len(),
            1
        );
    } else {
        assert_eq!(fx.object("22222222222222222222222222222222").0, "open");
        assert_eq!(
            fx.object("11111111111111111111111111111111").0,
            "dispatched"
        );
        assert_eq!(fx.count("outbox"), 1);
        let (kind, payload): (String, String) = fx
            .observer()
            .query_row("SELECT kind,payload_cbor_hex FROM outbox", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(kind, "factory.dispatch");
        assert_eq!(
            field(
                &coglet_types::decode_profile(&hex::decode(payload).unwrap()).unwrap(),
                "task_id"
            )
            .as_text(),
            Some("11111111111111111111111111111111")
        );
    }
    if dispatch {
        drop((service, db));
        dispatch_result_consumed(&mut fx, &engine, &message, &after);
        return;
    }
    if duplicate {
        let before = fx.count("events");
        // A fresh runtime event identity naming the same immutable line cannot
        // apply the decision a second time, including after service/DB reopen.
        drop((service, db));
        service = fx.service();
        db = fx.db();
        let mut repeated = message.clone();
        repeated["index"] = json!(1);
        repeated["id"] = json!("77777777777777777777777777777777");
        let appended = format!("{}\n{}\n", message, repeated);
        let error = service.seal_boundary(&fx.setup["inputs"]["trace_id"], 10, &[json!({
            "actor":format!("hub:{}","a".repeat(64)),"audit_id":"88888888888888888888888888888888",
            "ticket":null,"method":"hub.inbox","target":C,"path":"/data/inbox.jsonl",
            "before":after,"after":hex::encode(appended)
        })]).unwrap_err();
        assert!(error.contains("pending request"), "{error}");
        assert_eq!(fx.count("events"), before);
        assert_eq!(fx.count("outbox"), 1);
    }
    let frames = fx.journal(&service);
    drop((service, db));
    // Offline guest replay from the successor segment's initial snapshot;
    // DB state and provider are not available to this execution.
    std::fs::remove_file(&fx.db_path).unwrap();
    let mut runtime = coglet_runtime::Runtime::import_trace(&replay_start).unwrap();
    let refs: Vec<&[u8]> = frames
        .iter()
        .filter(|frame| {
            let record = coglet_types::decode_profile(frame).unwrap();
            field(&record, "trace_id").as_text() == fx.setup["inputs"]["trace_id"].as_str()
        })
        .map(Vec::as_slice)
        .collect();
    assert_eq!(refs.len(), 1);
    runtime
        .replay_database(&refs, &engine, 32 * 1024 * 1024, 65536)
        .unwrap();
    assert!(!fx.db_path.exists());
}

#[test]
fn factory_wasm_llm_reply_produces_hypothesis() {
    reply_scenario(false, false, false);
}
#[test]
fn factory_wasm_stale_llm_reply_refused() {
    reply_scenario(true, false, false);
}
#[test]
fn factory_wasm_reply_survives_reopen() {
    reply_scenario(false, true, false);
}
#[test]
fn factory_wasm_reply_to_outbox_dispatch_intent() {
    reply_scenario(false, false, false);
}

#[test]
fn factory_wasm_outbox_dispatch_result_consumed() {
    reply_scenario(false, false, true);
}
