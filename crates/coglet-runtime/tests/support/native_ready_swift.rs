//! Actual unchanged Swift SDK health/native-stop oracle over the real C ABI.
//! macOS-only execution, not iOS/acoustic qualification.
use super::*;

#[test]
fn swift_sdk_ready_health_obstruction_and_control() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let scratch = tempfile::tempdir().unwrap();
    let target = scratch.path().join("target");
    let built = Command::new("cargo")
        .current_dir(&root)
        .args([
            "build",
            "--locked",
            "-p",
            "coglet-runtime",
            "--lib",
            "--no-default-features",
            "--features",
            "native-engine,ffi-test-hooks",
        ])
        .env("CARGO_TARGET_DIR", &target)
        .status()
        .unwrap();
    assert!(
        built.success(),
        "isolated embedded diagnostic library build"
    );
    let swift = scratch.path().join("swift");
    assert!(
        Command::new("bash")
            .current_dir(&root)
            .arg("crates/coglet-runtime/tools/ready-output-sdk/build.sh")
            .arg(&swift)
            .status()
            .unwrap()
            .success()
    );
    let wasm = wat::parse_str(
        "(module (memory (export \"memory\") 1 2)
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))",
    )
    .unwrap();
    for mode in ["control", "obstruct"] {
        let dir = scratch.path().join(mode);
        std::fs::create_dir(&dir).unwrap();
        let setup = with_wasm(setup(), &wasm);
        std::fs::write(
            dir.join("binding.json"),
            serde_json::to_vec(&json!({"surface":S,"permitted_coglets":[C]})).unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join("startup.json"), serde_json::to_vec(&json!({"wal":dir.join("state.wal"),"setup":setup,"native_schedule":setup["native_schedule"]})).unwrap()).unwrap();
        let result = Command::new(swift.join("ready-output-sdk"))
            .arg(target.join("debug/libcoglet_runtime.dylib"))
            .arg(&dir)
            .arg(mode)
            .output()
            .unwrap();
        eprintln!(
            "Swift {mode}: {}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(
            result.status.success(),
            "Swift SDK oracle {mode}: {}",
            result.status
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("SDK_READY_HEALTH_EXECUTED"));
        let recovered = RuntimeService::open(&dir.join("state.wal"), &setup, 128).unwrap();
        assert_eq!(recovered.records().len(), 1);
        let record = &recovered.records()[0];
        assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
        assert_eq!(record["status"], "committed");
        assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
        assert_eq!(record["actions"], json!([]));
    }
}
