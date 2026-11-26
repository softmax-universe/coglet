use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn cli(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_coglet"))
        .args(args)
        .output()
        .unwrap()
}
fn ok(args: &[&str]) -> Value {
    let out = cli(args);
    assert!(
        out.status.success(),
        "{:?}: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}
fn manifest() -> Value {
    json!({"version":"1.0","id":format!("coglet:{}",iroh::SecretKey::from_bytes(&[9;32]).public()),
        "subscriptions":[],"required_capabilities":[],"exclusive_outputs":[],
        "budgets":{"compute_mode":"wamr-2.4.3-fast-cumulative-v1","compute":100000,"memory_pages":2,"host_bytes":1000000,"wall_time_us":2000000,"max_events":64}})
}
fn prepare(dir: &Path) -> PathBuf {
    let log = coglet_runtime::cbor(&json!({"level":"info","message":"CLI execution","fields":{}}))
        .unwrap();
    let escaped: String = log.iter().map(|b| format!("\\{b:02x}")).collect();
    let code = wat::parse_str(format!(
        r#"(module
        (import "coglet_v0" "log" (func $log (param i32 i32) (result i32)))
        (memory (export "memory") 1 2) (data (i32.const 0) "{escaped}")
        (func (export "abi_alloc") (param i32) (result i32) i32.const 8192)
        (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
            i32.const 0 i32.const {} call $log drop i32.const 0))"#,
        log.len()
    ))
    .unwrap();
    fs::write(dir.join("input.wasm"), code).unwrap();
    fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest()).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("recipe"), b"precompiled WAT CLI integration test").unwrap();
    fs::write(dir.join("policy"), b"unsigned local sandbox; no dispatch").unwrap();
    dir.join("package")
}
fn build(dir: &Path) -> PathBuf {
    let package = prepare(dir);
    ok(&[
        "build",
        "--wasm",
        s(&dir.join("input.wasm")),
        "--manifest",
        s(&dir.join("manifest.json")),
        "--source-commit",
        &"a".repeat(40),
        "--recipe",
        s(&dir.join("recipe")),
        "--policy",
        s(&dir.join("policy")),
        "--output",
        s(&package),
    ]);
    package
}
#[test]
fn cli_build_run_replay_equal_hash_list_and_inspect() {
    let d = tempfile::tempdir().unwrap();
    let package = build(d.path());
    let runs = d.path().join("runs");
    fs::create_dir(&runs).unwrap();
    let trace = runs.join("first.json");
    let events = d.path().join("events.json");
    let batches: Vec<_> = (0..2).map(|step|json!([{"kind":"runtime.debug_step","source":manifest()["id"],"seq":step,"logical_time":step,"correlation_id":"1".repeat(32),"payload":{}}])).collect();
    fs::write(&events, serde_json::to_vec(&batches).unwrap()).unwrap();
    let daemon = std::env::var_os("COGLET_TEST_DAEMON")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_BIN_EXE_coglet")).with_file_name("cogletd"));
    assert!(
        daemon.is_file(),
        "build cogletd first: cargo build -p coglet-runtime --features native-engine"
    );
    let run = ok(&[
        "run",
        s(&package),
        "--events",
        s(&events),
        "--output",
        s(&trace),
        "--cogletd",
        s(&daemon),
    ]);
    assert_eq!(run["steps"], 2);
    let raw: Value = serde_json::from_slice(&fs::read(&trace).unwrap()).unwrap();
    assert_ne!(
        raw["initial_hash"], raw["final_hash"],
        "real log imports must mutate /logs"
    );
    assert_eq!(raw["records"][0]["status"], "committed");
    assert!(raw["records"][0]["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(raw["records"][0]["calls"][0]["op"], "log");
    let replay = ok(&["replay", s(&trace), "--package", s(&package)]);
    assert_eq!(run["final_hash"], replay["final_hash"]);
    assert_eq!(replay["state_hashes"].as_array().unwrap().len(), 2);
    assert_eq!(ok(&["inspect", s(&trace)])["final_hash"], run["final_hash"]);
    assert_eq!(ok(&["list", s(&runs)]).as_array().unwrap().len(), 1);
    assert!(
        !cli(&[
            "run",
            s(&package),
            "--events",
            s(&events),
            "--output",
            s(&trace)
        ])
        .status
        .success(),
        "must not overwrite private trace"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&trace).unwrap().permissions().mode() & 0o077,
            0
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let fake = d.path().join("wrong-daemon");
        fs::write(
            &fake,
            format!(
                "#!/bin/sh\n/bin/cat >/dev/null\n/bin/cat '{}'\n",
                trace.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
        let setup = coglet_cli::package_setup(&package).unwrap();
        let job = coglet_cli::sandbox_job(
            setup,
            json!(batches),
            json!([]),
            json!({"coglet":manifest()["id"],"operations":[],"resources":[]}),
            raw["trace_id"].as_str().unwrap(),
            999,
        )
        .unwrap();
        let error = coglet_cli::run_daemon(&fake, &job).unwrap_err();
        assert!(
            error.to_string().contains("recorded input mismatch: seed"),
            "{error:#}"
        );
    }
    let mut corrupt = raw.clone();
    corrupt["final_hash"] = json!("0".repeat(64));
    fs::write(
        runs.join("corrupt.json"),
        serde_json::to_vec(&corrupt).unwrap(),
    )
    .unwrap();
    assert!(
        !cli(&["list", s(&runs)]).status.success(),
        "must not hide invalid archives"
    );
    assert!(
        !cli(&[
            "replay",
            s(&runs.join("corrupt.json")),
            "--package",
            s(&package)
        ])
        .status
        .success()
    );
    let original_provenance = fs::read(package.join("provenance.cbor")).unwrap();
    let mut proof: Value = coglet_types::decode("hub-build-record", &original_provenance)
        .unwrap()
        .deserialized()
        .unwrap();
    proof["rewritten_sha256"] = json!("0".repeat(64));
    fs::write(
        package.join("provenance.cbor"),
        coglet_runtime::cbor(&proof).unwrap(),
    )
    .unwrap();
    let error = coglet_cli::package_setup(&package).unwrap_err();
    assert!(error.to_string().contains("provenance code mismatch"));
    fs::write(package.join("provenance.cbor"), original_provenance).unwrap();
    fs::write(package.join("step.wasm"), b"changed program").unwrap();
    assert!(
        !cli(&["replay", s(&trace), "--package", s(&package)])
            .status
            .success()
    );
}
#[test]
fn cli_cargo_build_compiles_real_rust_guest_before_packaging() {
    let d = tempfile::tempdir().unwrap();
    prepare(d.path());
    let mut m = manifest();
    m["budgets"]["memory_pages"] = json!(32);
    fs::write(
        d.path().join("manifest.json"),
        serde_json::to_vec(&m).unwrap(),
    )
    .unwrap();
    let target = d.path().join("target");
    let wasm = target.join("wasm32-wasip1/release/cli_test_guest.wasm");
    let guest = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/guest/Cargo.toml");
    let out = Command::new(env!("CARGO_BIN_EXE_coglet"))
        .args([
            "build",
            "--cargo-manifest",
            s(&guest),
            "--target",
            "wasm32-wasip1",
            "--wasm",
            s(&wasm),
            "--manifest",
            s(&d.path().join("manifest.json")),
            "--source-commit",
            &"a".repeat(40),
            "--recipe",
            s(&d.path().join("recipe")),
            "--policy",
            s(&d.path().join("policy")),
            "--output",
            s(&d.path().join("compiled-package")),
        ])
        .env("CARGO_TARGET_DIR", &target)
        .env_remove("RUSTC_WRAPPER")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(wasm.is_file());
    assert!(d.path().join("compiled-package/step.wasm").is_file());
}

#[test]
fn cli_bad_manifest_reports_actionable_error_before_build() {
    let d = tempfile::tempdir().unwrap();
    let package = prepare(d.path());
    let mut bad = manifest();
    bad["budgets"]["memory_pages"] = json!("unbounded");
    fs::write(
        d.path().join("manifest.json"),
        serde_json::to_vec(&bad).unwrap(),
    )
    .unwrap();
    let result = cli(&[
        "build",
        "--wasm",
        s(&d.path().join("input.wasm")),
        "--manifest",
        s(&d.path().join("manifest.json")),
        "--source-commit",
        &"a".repeat(40),
        "--recipe",
        s(&d.path().join("recipe")),
        "--policy",
        s(&d.path().join("policy")),
        "--output",
        s(&package),
    ]);
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("invalid manifest"));
    assert!(!package.exists());
}
#[test]
fn cli_help_identity_and_deploy_fail_closed() {
    let help = cli(&["--help"]);
    assert!(help.status.success());
    let help = String::from_utf8(help.stdout).unwrap();
    for command in [
        "build", "run", "replay", "inspect", "list", "identity", "enroll", "deploy",
    ] {
        assert!(help.contains(command));
    }
    let d = tempfile::tempdir().unwrap();
    let key = d.path().join("device.key");
    let id = ok(&["identity", s(&key)]);
    assert!(id["device"].as_str().is_some());
    let bytes = fs::read(&key).unwrap();
    assert_eq!(bytes.len(), 32);
    assert!(!cli(&["identity", s(&key)]).status.success());
    assert_eq!(fs::read(&key).unwrap(), bytes);
    let package = build(d.path());
    for command in ["deploy", "push"] {
        let result = cli(&[command, s(&package)]);
        assert!(!result.status.success());
        assert!(String::from_utf8_lossy(&result.stderr).contains("unexpected argument"));
    }
}
#[test]
fn sandbox_missing_grants_unknown_events_and_bad_wasm_are_rejected() {
    let d = tempfile::tempdir().unwrap();
    let package = build(d.path());
    let mut setup = coglet_cli::package_setup(&package).unwrap();
    setup["manifest"]["required_capabilities"] = json!(["test.act"]);
    assert!(
        coglet_cli::sandbox_job(setup, json!([[]]), json!([]), json!({}), &"1".repeat(32), 0)
            .is_err()
    );
    let mut wasm = vec![0, 97, 115, 109, 1, 0, 0, 0];
    wasm.extend([3, 2, 1, 127]); // invalid type index
    fs::write(d.path().join("invalid.wasm"), wasm).unwrap();
    assert!(
        coglet_cli::build(
            &d.path().join("invalid.wasm"),
            &d.path().join("manifest.json"),
            &"a".repeat(40),
            &d.path().join("recipe"),
            &d.path().join("policy"),
            &d.path().join("bad-package")
        )
        .is_err()
    );
}
