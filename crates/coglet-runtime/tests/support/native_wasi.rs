use super::native_engine_cases::with_wasm;
use super::*;
use coglet_runtime::{NativeEngine, Runtime, RuntimeService};
use std::{fs, io::Write, path::PathBuf, process::Command, sync::OnceLock};

fn engine(code: &[u8], budgets: &Value) -> NativeEngine {
    NativeEngine::new(code.to_vec(), budgets, 8_388_608, 65_536).unwrap()
}

fn wat_guest(data: &str, body: &str) -> Vec<u8> {
    wat::parse_str(format!(
        r#"(module
(import "wasi_snapshot_preview1" "path_open" (func $path_open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
(import "wasi_snapshot_preview1" "fd_write" (func $fd_write (param i32 i32 i32 i32) (result i32)))
(import "wasi_snapshot_preview1" "fd_close" (func $fd_close (param i32) (result i32)))
(import "wasi_snapshot_preview1" "path_unlink_file" (func $unlink (param i32 i32 i32) (result i32)))
(import "wasi_snapshot_preview1" "fd_filestat_set_size" (func $set_size (param i32 i64) (result i32)))
(import "wasi_snapshot_preview1" "path_readlink" (func $readlink (param i32 i32 i32 i32 i32 i32) (result i32)))
(memory (export "memory") 1 2)
{data}
(func (export "abi_alloc") (param i32) (result i32) i32.const 1024)
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
  {body}))"#
    ))
    .unwrap()
}

fn write_data_guest(abort: bool) -> Vec<u8> {
    wat_guest(
        r#"(data (i32.const 0) "count") (data (i32.const 16) "39")"#,
        &format!(
            r#"i32.const 64 i32.const 16 i32.store
i32.const 68 i32.const 2 i32.store
i32.const 4 i32.const 0 i32.const 0 i32.const 5 i32.const 9
i64.const 64 i64.const 0 i32.const 0 i32.const 80
call $path_open drop
i32.const 80 i32.load i32.const 64 i32.const 1 i32.const 84
call $fd_write drop
i32.const 80 i32.load call $fd_close drop
{}
i32.const 0"#,
            if abort { "unreachable" } else { "" }
        ),
    )
}

fn step_native(setup: &Value, code: &[u8]) -> Value {
    let mut runtime = Runtime::from_fixture(setup).unwrap();
    let mut inputs = setup["inputs"].clone();
    inputs["coglet"] = setup["manifest"]["id"].clone();
    inputs["budgets"] = setup["manifest"]["budgets"].clone();
    inputs["grants"] = json!(["expression.set_expression"]);
    runtime
        .step(&inputs, &mut engine(code, &setup["manifest"]["budgets"]))
        .unwrap()
}

fn hex_file(files: &Value, path: &str) -> Option<Vec<u8>> {
    files.get(path)?.as_str().map(|s| hex::decode(s).unwrap())
}

fn counter_wasm() -> &'static [u8] {
    static WASM: OnceLock<Vec<u8>> = OnceLock::new();
    WASM.get_or_init(|| {
        if let Some(path) = std::env::var_os("COGLET_COUNTER_WASM") {
            return fs::read(path).expect("COGLET_COUNTER_WASM");
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dest = root.join("target/guests/wasm32-wasip1/release/coglet_counter.wasm");
        if !dest.is_file() {
            let status = Command::new("cargo")
                .args([
                    "rustc",
                    "--manifest-path",
                    root.join("coglets/counter/Cargo.toml").to_str().unwrap(),
                    "--target",
                    "wasm32-wasip1",
                    "--release",
                    "--lib",
                    "--locked",
                    "--",
                    "-C",
                    "link-arg=--max-memory=16777216",
                ])
                .env("CARGO_TARGET_DIR", root.join("target/guests"))
                .status()
                .expect("cargo rustc counter");
            assert!(status.success(), "failed to build coglet-counter wasm");
        }
        let qualify = Command::new("cargo")
            .args([
                "run",
                "--quiet",
                "--locked",
                "--manifest-path",
                root.join("Cargo.toml").to_str().unwrap(),
                "-p",
                "coglet-guest-rust",
                "--example",
                "qualify_guest",
                "--",
            ])
            .arg(&dest)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("qualify_guest");
        assert!(qualify.success(), "failed to apply R3 to coglet-counter");
        fs::read(dest).unwrap()
    })
}

fn run_service(setup: &Value, code: &[u8], runtime_bytes: u32, stack: u32) -> (PathBuf, Value) {
    let directory = std::env::temp_dir().join(format!(
        "wasi-{}-{}",
        std::process::id(),
        &coglet_runtime::digest(code)[..8]
    ));
    fs::create_dir_all(&directory).unwrap();
    let wal = directory.join("state.wal");
    let mut service = RuntimeService::open(&wal, setup, 128).unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let mut native = NativeEngine::new(
        code.to_vec(),
        &setup["manifest"]["budgets"],
        runtime_bytes,
        stack,
    )
    .unwrap();
    let mut context = setup["inputs"].clone();
    context["budgets"] = setup["manifest"]["budgets"].clone();
    context["grants"] = json!(["expression.set_expression"]);
    context["logical_time"] = json!(1);
    let result = service.run_next(&context, &mut native).unwrap().unwrap();
    drop(service);
    (directory, result)
}

#[test]
fn native_sdk_guest_writes_data_commits_and_reopens() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let wasm = counter_wasm();
    let mut setup = with_wasm(setup(), wasm);
    setup["manifest"]["budgets"]["memory_pages"] = json!(256);
    setup["manifest"]["budgets"]["compute"] = json!(100_000_000);
    setup["manifest"]["budgets"]["wall_time_us"] = json!(5_000_000);
    setup["manifest"]["budgets"]["host_bytes"] = json!(4_000_000);
    let (directory, result) = run_service(&setup, wasm, 32_000_000, 262_144);
    assert_eq!(result["journal"]["status"], "committed");
    assert_ne!(
        result["journal"]["input_hash"],
        result["journal"]["output_hash"]
    );
    let first = hex_file(&result["journal"]["output_files"], "/data/count");
    assert_ne!(first.as_deref(), Some(b"0".as_slice()));
    assert!(first.is_some());
    let recovered = RuntimeService::open(&directory.join("state.wal"), &setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    assert_eq!(
        hex_file(&recovered.records()[0]["output_files"], "/data/count"),
        first
    );
    drop(recovered);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_guest_abort_discards_wasi_writes() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = write_data_guest(true);
    let setup = with_wasm(setup(), &code);
    let result = step_native(&setup, &code);
    assert_eq!(result["journal"]["status"], "aborted");
    assert_eq!(
        result["journal"]["input_hash"],
        result["journal"]["output_hash"]
    );
    assert_eq!(
        hex_file(&result["journal"]["output_files"], "/data/count"),
        hex_file(&result["journal"]["input_files"], "/data/count")
    );
    assert_ne!(
        hex_file(&result["journal"]["output_files"], "/data/count").as_deref(),
        Some(b"39".as_slice())
    );
}

#[test]
fn native_wasi_crash_between_journal_and_commit_recovers_one_state() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = write_data_guest(false);
    let setup = with_wasm(setup(), &code);
    let directory = std::env::temp_dir().join(format!("wasi-crash-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let wal = directory.join("state.wal");
    let mut service = RuntimeService::open(&wal, &setup, 128).unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let after_queued = fs::metadata(&wal).unwrap().len();
    let mut native = engine(&code, &setup["manifest"]["budgets"]);
    let mut context = setup["inputs"].clone();
    context["budgets"] = setup["manifest"]["budgets"].clone();
    context["grants"] = json!(["expression.set_expression"]);
    context["logical_time"] = json!(1);
    let committed = service.run_next(&context, &mut native).unwrap().unwrap();
    assert_eq!(committed["journal"]["status"], "committed");
    let after_files = committed["journal"]["output_files"].clone();
    drop(service);
    let complete = fs::read(&wal).unwrap();
    assert!(complete.len() as u64 > after_queued);
    {
        let mut file = fs::OpenOptions::new().append(true).open(&wal).unwrap();
        file.write_all(&[0, 0, 0, 12, 0xa1, 0x01]).unwrap();
        file.sync_all().unwrap();
    }
    let recovered = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(recovered.records()[0]["output_files"], after_files);
    drop(recovered);
    fs::write(&wal, &complete[..after_queued as usize]).unwrap();
    let previous = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert!(previous.records().is_empty());
    assert!(
        previous
            .records()
            .first()
            .and_then(|r| hex_file(&r["output_files"], "/data/count"))
            .is_none()
    );
    drop(previous);
    fs::write(&wal, &complete).unwrap();
    let restored = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(restored.records()[0]["output_files"], after_files);
    drop(restored);
    fs::remove_dir_all(directory).unwrap();
}

#[test]
fn native_wasi_replay_yields_identical_hashes() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = write_data_guest(false);
    let setup = with_wasm(setup(), &code);
    let mut live = Runtime::from_fixture(&setup).unwrap();
    let mut inputs = setup["inputs"].clone();
    inputs["coglet"] = setup["manifest"]["id"].clone();
    inputs["budgets"] = setup["manifest"]["budgets"].clone();
    inputs["grants"] = json!(["expression.set_expression"]);
    let result = live
        .step(&inputs, &mut engine(&code, &setup["manifest"]["budgets"]))
        .unwrap();
    let expected = vec![result["state_hash"].as_str().unwrap().to_owned()];
    let replayed = Runtime::from_fixture(&setup)
        .unwrap()
        .replay(
            live.records(),
            &mut engine(&code, &setup["manifest"]["budgets"]),
        )
        .unwrap();
    assert_eq!(replayed, expected);
}

#[test]
fn native_forbidden_ops_denied_with_permission_errno() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = wat_guest(
        r#"(data (i32.const 0) "bin/step.wasm")
(data (i32.const 32) "../code/x")
(data (i32.const 48) "out")
(data (i32.const 56) "link")"#,
        r#"i32.const 360
i32.const 3 i32.const 0 i32.const 0 i32.const 13 i32.const 9
i64.const 64 i64.const 0 i32.const 0 i32.const 400
call $path_open
i32.store8
i32.const 5 i32.const 0 i32.const 48 i32.const 3 i32.const 1
i64.const 64 i64.const 0 i32.const 1 i32.const 404
call $path_open drop
i32.const 361
i32.const 404 i32.load i64.const 0
call $set_size
i32.store8
i32.const 362
i32.const 5 i32.const 48 i32.const 3
call $unlink
i32.store8
i32.const 363
i32.const 4 i32.const 0 i32.const 32 i32.const 9 i32.const 0
i64.const 2 i64.const 0 i32.const 0 i32.const 416
call $path_open
i32.store8
i32.const 364
i32.const 4 i32.const 56 i32.const 4 i32.const 420 i32.const 8 i32.const 424
call $readlink
i32.store8
i32.const 4 i32.const 0 i32.const 48 i32.const 3 i32.const 9
i64.const 64 i64.const 0 i32.const 0 i32.const 428
call $path_open drop
i32.const 128 i32.const 360 i32.store
i32.const 132 i32.const 5 i32.store
i32.const 428 i32.load i32.const 128 i32.const 1 i32.const 432
call $fd_write drop
i32.const 0"#,
    );
    let setup = with_wasm(setup(), &code);
    let before = setup["initial_files"]["/code/bin/step.wasm"].clone();
    let result = step_native(&setup, &code);
    assert_eq!(result["journal"]["status"], "committed", "{result}");
    assert_eq!(
        result["journal"]["output_files"]["/code/bin/step.wasm"],
        before
    );
    let codes = hex_file(&result["journal"]["output_files"], "/data/out").unwrap();
    assert_eq!(codes, vec![76, 76, 76, 76, 76], "{codes:?}");
}

#[test]
fn native_forbidden_imports_fail_admission_without_instantiation() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let setup = with_wasm(setup(), &write_data_guest(false));
    let budgets = &setup["manifest"]["budgets"];
    for (name, sig) in [
        ("clock_time_get", "(param i32 i64 i32) (result i32)"),
        ("random_get", "(param i32 i32) (result i32)"),
        ("poll_oneoff", "(param i32 i32 i32 i32) (result i32)"),
        ("path_symlink", "(param i32 i32 i32 i32 i32) (result i32)"),
        (
            "path_link",
            "(param i32 i32 i32 i32 i32 i32 i32) (result i32)",
        ),
    ] {
        let bytes = wat::parse_str(format!(
            r#"(module (import "wasi_snapshot_preview1" "{name}" (func {sig}))
(memory (export "memory") 1 2)
(func (export "abi_alloc") (param i32) (result i32) i32.const 0)
(func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#
        ))
        .unwrap();
        assert!(
            NativeEngine::new(bytes, budgets, 8_388_608, 65_536).is_err(),
            "{name} must fail admission"
        );
    }
}
