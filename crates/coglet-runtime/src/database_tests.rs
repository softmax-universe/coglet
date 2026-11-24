use super::*;
use coglet_db::{CellType, Column, Grants, Mode, Statement, TypedStatement};
use coglet_engine_wamr::Wamr;
use coglet_mock::engine::{ClosureGuest, MockEngine, NativeModule};
use serde_json::json;
use std::time::{Duration, Instant};

fn catalog() -> Catalog {
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
            columns: vec![
                Column {
                    name: "id".into(),
                    kind: CellType::Text,
                },
                Column {
                    name: "value".into(),
                    kind: CellType::Text,
                },
            ],
            tables: vec!["objects".into()],
            order: vec!["id".into()],
        },
    ])
    .unwrap()
}
fn request(statement: &str) -> Vec<u8> {
    crate::cbor(&json!({"operation":"statement","statement":statement,"mode":if statement=="put" {"write"} else {"read"},"params":if statement=="put" {json!(["a","value"])} else {json!([])},"max_rows":64,"max_bytes":65536})).unwrap()
}
fn limits(mode: &str) -> Limits {
    Limits {
        compute_mode: mode.into(),
        compute: 1_000_000,
        memory_pages: 3,
        host_bytes: 1_000_000,
        runtime_bytes: 8 * 1024 * 1024,
        stack_bytes: 65536,
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn manifest(mode: &str, cat: &Catalog) -> Value {
    json!({"version":"0.2","id":"coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","subscriptions":[],"required_capabilities":["db.read","db.write"],"exclusive_outputs":[],"budgets":{"compute_mode":mode,"compute":1_000_000,"memory_pages":3,"host_bytes":1_000_000,"wall_time_us":30_000_000,"max_events":1},"database":{"schema_min":1,"schema_max":1,"catalog_hash":cat.digest().unwrap()}})
}
fn pins(db: &Database, cat: &Catalog, code: &[u8]) -> Pins {
    let (schema_version, schema_hash) = db.schema_pin().unwrap();
    Pins {
        code_hash: digest(code),
        schema_version,
        schema_hash,
        catalog_hash: cat.digest().unwrap(),
        grants: Grants {
            read: true,
            write: true,
        },
        intent_kinds: Default::default(),
    }
}
fn ordinary(mode: &str) -> StepHost {
    StepHost::new(
        crate::Snapshot::new(Default::default()).unwrap(),
        Default::default(),
        &json!({"coglet":"coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","grants":["db.read","db.write"],"budgets":{"compute_mode":mode,"host_bytes":1_000_000}}),
    )
}
fn field<'a>(v: &'a V, key: &str) -> &'a V {
    v.as_map()
        .unwrap()
        .iter()
        .find(|(k, _)| k.as_text() == Some(key))
        .map(|(_, v)| v)
        .unwrap()
}
fn wasm(requests: &[Vec<u8>], termination: u32) -> Vec<u8> {
    let mut data = String::new();
    let mut body = String::new();
    for (i, raw) in requests.iter().enumerate() {
        let ptr = 1024 + i * 2048;
        let escaped = raw.iter().map(|b| format!("\\{b:02x}")).collect::<String>();
        data.push_str(&format!("(data (i32.const {ptr}) \"{escaped}\")"));
        body.push_str(&format!(
            "i32.const {ptr} i32.const {} i32.const 65536 i32.const 65540 call $db drop\n",
            raw.len()
        ));
        // Exercise ordinary rejected calls in the same order as DB calls.
        body.push_str("i32.const 0 i32.const 0 call $log drop\n");
    }
    wat::parse_str(format!(r#"(module
      (import "coglet_v0" "db" (func $db (param i32 i32 i32 i32) (result i32)))
      (import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
      (memory (export "memory") 3 3) {data}
      (func (export "abi_alloc") (param i32) (result i32) i32.const 160000)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) {body} i32.const {termination}))"#)).unwrap()
}
fn input() -> StepInput {
    StepInput {
        events: vec![0x81, 0xa0],
        step: 0,
        logical_time: 0,
        seed: 0,
    }
}

fn exercise<E: Engine>(engine: &E, code: &[u8], requests: &[Vec<u8>], mode: &str, fail: bool) {
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(
        root.path(),
        "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();
    // Disposable test fixture only. No runtime migration API or approval bypass.
    let observer = rusqlite::Connection::open(db.path()).unwrap();
    observer.execute_batch("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
    let cat = catalog();
    let p = pins(&db, &cat, code);
    let (mut unit, instance) = admit(
        engine,
        code,
        Deployment {
            manifest: &manifest(mode, &cat),
            grants: &["db.read".into(), "db.write".into()],
            pins: p,
        },
        &mut db,
        &cat,
        limits(mode),
    )
    .unwrap();
    let mut ordinary = ordinary(mode);
    let mut host = DatabaseHost::new(&mut unit, &mut ordinary);
    let result = host.execute(instance, input());
    assert_eq!(result.result.is_err(), fail, "{result:?}");
    assert_eq!(host.calls().len(), requests.len() * 2);
    for (i, raw) in requests.iter().enumerate() {
        assert_eq!(field(&host.calls()[2 * i], "op").as_text(), Some("db"));
        assert_eq!(field(&host.calls()[2 * i + 1], "op").as_text(), Some("log"));
        if coglet_types::decode("database::db-request", raw).is_err() {
            assert_eq!(
                field(&host.calls()[2 * i], "argument_hex").as_text(),
                Some(hex::encode(raw).as_str())
            );
        }
    }
    if !fail {
        let response = field(&host.calls()[2], "response");
        assert_eq!(field(response, "rows").as_array().unwrap().len(), 1); // sees provisional put
    }
    // Keeping the Unit alive must not retain a write lock after an abort.
    drop(host);
    if fail {
        observer
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
            .unwrap();
    }
    drop(unit); // no complete decision supplied: nothing can commit
    assert_eq!(
        observer
            .query_row("SELECT count(*) FROM objects", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[test]
fn real_wasm_db_imports_share_runtime_call_order_and_rollback() {
    for (requests, termination, fail) in [
        (vec![request("put"), request("scan")], 0, false),
        (vec![request("put"), request("put")], 0, true),
        (vec![request("put"), vec![0xff]], 0, true),
        (vec![request("put"), request("scan")], 7, true),
    ] {
        let code = wasm(&requests, termination);
        exercise(
            &Wamr::interpreter().allow_db(),
            &code,
            &requests,
            coglet_engine_wamr::COMPUTE_MODE,
            fail,
        );
    }
}
#[test]
fn mock_db_imports_share_runtime_call_order_and_rollback() {
    for (requests, termination, fail) in [
        (vec![request("put"), request("scan")], 0, false),
        (vec![request("put"), request("put")], 0, true),
        (vec![request("put"), vec![0xff]], 0, true),
        (vec![request("put"), request("scan")], 7, true),
    ] {
        let code = wasm(&requests, termination);
        let calls = requests.clone();
        let mut engine = MockEngine::default();
        engine.allow_db();
        engine
            .register(
                code.clone(),
                NativeModule::new(3, 3, move || {
                    let calls = calls.clone();
                    Box::new(ClosureGuest(
                        move |_, _: &StepInput, cx: &mut coglet_mock::engine::Context<'_>| {
                            for (i, raw) in calls.iter().enumerate() {
                                let ptr = 1024 + i as u32 * 2048;
                                cx.memory.write(ptr, raw)?;
                                cx.call(
                                    "coglet_v0",
                                    "db",
                                    &[ptr as u64, raw.len() as u64, 65536, 65540],
                                )?;
                                cx.call("coglet_v0", "log", &[0, 0])?;
                            }
                            Ok(termination)
                        },
                    ))
                })
                .unwrap(),
            )
            .unwrap();
        exercise(
            &engine,
            &code,
            &requests,
            coglet_mock::engine::COMPUTE_MODE,
            fail,
        );
    }
}

#[test]
fn db_deployment_pins_refuse_before_instantiation() {
    struct Count(std::cell::Cell<usize>);
    impl Engine for Count {
        type Module = <Wamr as Engine>::Module;
        type Instance = <Wamr as Engine>::Instance;
        fn metadata(&self) -> coglet_engine::Metadata {
            Wamr::interpreter().metadata()
        }
        fn load(&self, bytes: &[u8], limits: &Limits) -> coglet_engine::Result<Self::Module> {
            Wamr::interpreter().allow_db().load(bytes, limits)
        }
        fn instantiate(
            &self,
            module: &Self::Module,
            limits: Limits,
        ) -> coglet_engine::Result<Self::Instance> {
            self.0.set(self.0.get() + 1);
            Wamr::interpreter().allow_db().instantiate(module, limits)
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut db = Database::open(
        root.path(),
        "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    )
    .unwrap();
    rusqlite::Connection::open(db.path()).unwrap().execute_batch("CREATE TABLE objects(id TEXT PRIMARY KEY NOT NULL,value TEXT NOT NULL); PRAGMA user_version=1;").unwrap();
    let cat = catalog();
    let code = wasm(&[request("scan")], 0);
    let mode = coglet_engine_wamr::COMPUTE_MODE;
    let engine = Count(std::cell::Cell::new(0));
    for change in 0..9 {
        let mut manifest = manifest(mode, &cat);
        let mut pins = pins(&db, &cat, &code);
        let mut grants = vec!["db.read".into(), "db.write".into()];
        match change {
            0 => manifest["version"] = json!("0.1"),
            1 => {
                manifest["id"] =
                    json!("coglet:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")
            }
            2 => manifest["database"]["catalog_hash"] = json!("b".repeat(64)),
            3 => pins.code_hash = "b".repeat(64),
            4 => pins.schema_hash = "b".repeat(64),
            5 => manifest["database"]["schema_min"] = json!(2),
            6 => grants.clear(),
            7 => manifest["budgets"]["host_bytes"] = json!(1),
            _ => pins
                .intent_kinds
                .insert("worker.run".into())
                .then_some(())
                .unwrap(),
        }
        assert!(
            admit(
                &engine,
                &code,
                Deployment {
                    manifest: &manifest,
                    grants: &grants,
                    pins
                },
                &mut db,
                &cat,
                limits(mode)
            )
            .is_err()
        );
        assert_eq!(
            engine.0.get(),
            0,
            "bad deployment reached instantiate: {change}"
        );
        assert_eq!(db.counters().unwrap(), (0, 0));
    }
}
