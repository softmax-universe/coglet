use super::*;
use coglet_runtime::{NativeEngine, Runtime, RuntimeService, digest};
#[path = "native_expiry.rs"]
mod expiry;

fn escaped(raw: &[u8]) -> String {
    raw.iter().map(|b| format!("\\{b:02x}")).collect()
}
pub fn with_wasm(mut setup: Value, wasm: &[u8]) -> Value {
    setup["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(wasm));
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
    provenance["rewritten_sha256"] = json!(digest(wasm));
    setup["initial_files"]["/code/build/provenance.cbor"] =
        json!(hex::encode(cbor(&provenance).unwrap()));
    setup["manifest"]["budgets"]["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
    setup["manifest"]["budgets"]["compute"] = json!(100_000);
    setup["manifest"]["budgets"]["memory_pages"] = json!(2);
    setup["manifest"]["budgets"]["wall_time_us"] = json!(500_000);
    setup["manifest"]["budgets"]["host_bytes"] = json!(1_000_000);
    setup["manifest"]["subscriptions"] = json!([]);
    setup["manifest"]["required_capabilities"] = json!(["expression.set_expression"]);
    setup["manifest"]["exclusive_outputs"] = json!(["display.face"]);
    setup["initial_actions"] = json!([]);
    setup["initial_effect_ids"]["actions"] = json!([]);
    setup["initial_timers"] = json!([{"id":"d".repeat(32),"at":0}]);
    setup["initial_effect_ids"]["timers"] = json!(["d".repeat(32)]);
    setup["native_schedule"] = json!({"trace_id":setup["inputs"]["trace_id"],"seed":7,"runtime_bytes":8_388_608,"stack_bytes":65_536});
    setup
}
fn program(abort: bool) -> Vec<u8> {
    program_delay(abort, false)
}
fn program_delay(abort: bool, delay: bool) -> Vec<u8> {
    let actions = setup()["initial_actions"].as_array().unwrap().clone();
    let mut data = String::new();
    let mut calls = String::new();
    for (i, action) in actions.iter().enumerate() {
        let raw = cbor(action).unwrap();
        let ptr = i * 2048;
        data.push_str(&format!("(data (i32.const {ptr}) \"{}\")", escaped(&raw)));
        calls.push_str(&format!(
            "i32.const {ptr} i32.const {} call $emit drop ",
            raw.len()
        ));
    }
    if delay {
        data.push_str("(func (export \"_initialize\") (local $n i32) i32.const 100000000 local.set $n (loop $again local.get $n i32.const 1 i32.sub local.tee $n br_if $again))");
    }
    wat::parse_str(format!("(module (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))
        (memory (export \"memory\") 1 2) {data}
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) {calls} {} i32.const 0))", if abort {"unreachable"} else {""})).unwrap()
}
fn exercise_native(ffi: bool, abort: bool, name: &str) {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut host = Host::new(ffi, name);
    if !ffi {
        let mut ready = host.connect();
        ready.hello();
        drop(ready);
    }
    host.stop();
    // A fresh service, not a reconfiguration of existing immutable code pins.
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    host.setup = with_wasm(setup(), &program(abort));
    std::fs::write(
        host.dir.join("setup.json"),
        serde_json::to_vec(&host.setup).unwrap(),
    )
    .unwrap();
    host.start();
    let mut link = host.connect();
    link.hello();
    // There is a due timer, but no guest may run without the live manifest grants.
    link.send(
        "health",
        json!({"state":"ready","clock":1050,"pending":0,"correlation_id":"9".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["type"], "health");
    link.grants(1);
    let mut sent = vec![];
    if !abort {
        for id in ["f", "4"] {
            let raw = link.recv();
            assert_eq!(decode(&raw)["body"]["action"]["id"], id.repeat(32));
            sent.push(raw);
        }
    }
    link.send(
        "health",
        json!({"state":"ready","clock":1200,"pending":0,"correlation_id":"9".repeat(32)}),
    );
    assert_eq!(decode(&link.recv())["type"], "health");
    drop(link);
    host.stop();
    let recovered = RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    let record = &recovered.records()[0];
    assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
    assert_eq!(record["calls"].as_array().unwrap().len(), 2);
    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(
        record["status"],
        if abort { "aborted" } else { "committed" }
    );
    assert_eq!(
        record["actions"].as_array().unwrap().len(),
        if abort { 0 } else { 2 }
    );
    if abort {
        assert_eq!(record["input_hash"], record["output_hash"]);
    }
    assert_eq!(recovered.queued(), 0);
    drop(recovered);
    if !abort {
        std::thread::sleep(Duration::from_millis(20));
        host.start();
        let mut link = host.connect();
        link.hello();
        for original in sent {
            assert_eq!(link.recv(), original);
        }
        // Reconciliation ran before new grants; no new instance/batch was run.
        drop(link);
        host.stop();
        let recovered =
            RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
        assert_eq!(recovered.records().len(), 1);
    }
}
#[cfg(feature = "echo-smoke")]
#[test]
fn ffi_compiled_echo_timer_wake_starts_executes_and_stops() {
    compiled_echo_timer_wake(true, "ffi-compiled-echo");
}
#[cfg(feature = "echo-smoke")]
#[test]
fn socket_compiled_echo_timer_wake_starts_executes_and_stops() {
    compiled_echo_timer_wake(false, "socket-compiled-echo");
}
#[cfg(feature = "echo-smoke")]
fn compiled_echo_timer_wake(ffi: bool, name: &str) {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = std::env::var_os("COGLET_ECHO_SMOKE_WASM")
        .expect("echo-smoke requires the compiled guest path");
    let wasm = std::fs::read(path).unwrap();
    let mut host = Host::new(ffi, name);
    if !ffi {
        let mut ready = host.connect();
        ready.hello();
        drop(ready);
    }
    host.stop();
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    host.setup = with_wasm(setup(), &wasm);
    host.setup["manifest"]["budgets"]["memory_pages"] = json!(256);
    host.setup["manifest"]["budgets"]["compute"] = json!(100_000_000);
    host.setup["manifest"]["budgets"]["wall_time_us"] = json!(5_000_000);
    if !ffi {
        std::fs::write(
            host.dir.join("setup.json"),
            serde_json::to_vec(&host.setup).unwrap(),
        )
        .unwrap();
    }
    host.start();
    let mut link = host.connect();
    link.hello();
    link.grants(1);
    // Embedded grants pump synchronously. The socket needs an explicit control
    // barrier before terminating the child; release-only M1 qualifies that live
    // exchange without weakening the production inactivity timeout.
    if !ffi {
        link.send(
            "health",
            json!({"state":"ready","clock":1200,"pending":0,"correlation_id":"9".repeat(32)}),
        );
        assert_eq!(decode(&link.recv())["type"], "health");
    }
    drop(link);
    host.stop();
    let recovered = RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    let record = &recovered.records()[0];
    assert_eq!(record["status"], "committed");
    assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    // The production echo ignores runtime.timer. This proves its real handle
    // executes, not test.echo surface roundtrip (reserved catalog operation).
    assert!(record["actions"].as_array().unwrap().is_empty());
}
#[test]
fn ffi_real_wasm_commits_call_order_before_dispatch_and_reconciles() {
    exercise_native(true, false, "ffi-wasm");
}
#[test]
fn socket_real_wasm_commits_call_order_before_dispatch_and_reconciles() {
    exercise_native(false, false, "socket-wasm");
}
#[test]
fn ffi_real_wasm_trap_discards_staged_emits_and_consumes_once() {
    exercise_native(true, true, "ffi-wasm-abort");
}
#[test]
fn socket_real_wasm_trap_discards_staged_emits_and_consumes_once() {
    exercise_native(false, true, "socket-wasm-abort");
}
#[test]
fn real_wamr_journal_reexecutes_two_steps_with_identical_state_hashes() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = program(false);
    let setup = with_wasm(setup(), &code);
    let mut inputs = setup["inputs"].clone();
    inputs["coglet"] = setup["manifest"]["id"].clone();
    inputs["budgets"] = setup["manifest"]["budgets"].clone();
    inputs["grants"] = json!(["expression.set_expression"]);
    let engine = || {
        NativeEngine::new(
            code.clone(),
            &setup["manifest"]["budgets"],
            8_388_608,
            65_536,
        )
        .unwrap()
    };
    let mut live = Runtime::from_fixture(&setup).unwrap();
    let mut expected = Vec::new();
    for step in 0..2 {
        inputs["step"] = json!(step);
        inputs["previous"] = live
            .records()
            .last()
            .map(|r| r["record_hash"].clone())
            .unwrap_or(Value::Null);
        let result = live.step(&inputs, &mut engine()).unwrap();
        assert!(result["journal"]["usage"]["compute"].as_u64().unwrap() > 0);
        expected.push(result["state_hash"].as_str().unwrap().to_owned());
    }
    let replayed = Runtime::from_fixture(&setup)
        .unwrap()
        .replay(live.records(), &mut engine())
        .unwrap();
    assert_eq!(replayed, expected);
}

#[test]
fn native_admission_refuses_mode_memory_wasi_and_preserves_queued_input() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut s = with_wasm(setup(), &program(false));
    let code = program(false);
    let mut budgets = s["manifest"]["budgets"].clone();
    budgets["compute_mode"] = json!("unqualified");
    assert!(NativeEngine::new(code.clone(), &budgets, 8_388_608, 65_536).is_err());
    budgets = s["manifest"]["budgets"].clone();
    budgets["memory_pages"] = json!(1);
    assert!(NativeEngine::new(code.clone(), &budgets, 8_388_608, 65_536).is_err());
    let wasi = wat::parse_str("(module (import \"wasi_snapshot_preview1\" \"args_sizes_get\" (func (param i32 i32) (result i32))) (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))").unwrap();
    assert!(NativeEngine::new(wasi, &s["manifest"]["budgets"], 8_388_608, 65_536).is_ok());
    let clock = wat::parse_str("(module (import \"wasi_snapshot_preview1\" \"clock_time_get\" (func (param i32 i64 i32) (result i32))) (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))").unwrap();
    assert!(NativeEngine::new(clock, &s["manifest"]["budgets"], 8_388_608, 65_536).is_err());
    // A valid engine with lower per-step memory admission cannot consume the batch.
    let dir = std::env::temp_dir().join(format!("native-admit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut service = RuntimeService::open(&dir.join("wal"), &s, 128).unwrap();
    service.queue_due_timers(1).unwrap();
    let mut engine = NativeEngine::new(code, &s["manifest"]["budgets"], 8_388_608, 65_536).unwrap();
    let mut context = s["inputs"].clone();
    context["budgets"] = budgets;
    context["logical_time"] = json!(1);
    assert!(service.run_next(&context, &mut engine).is_err());
    assert_eq!(service.queued(), 1);
    assert!(service.records().is_empty());
    // Metadata supplied by the caller is never trusted on a real-engine path.
    context["budgets"] = s["manifest"]["budgets"].clone();
    context["grants"] = json!(["expression.set_expression"]);
    context["engine"] = json!({"build":"caller-lie","policy":"caller-lie"});
    let result = service.run_next(&context, &mut engine).unwrap().unwrap();
    assert_eq!(
        result["journal"]["engine"]["build"],
        coglet_engine_wamr::BUILD
    );
    assert_ne!(result["journal"]["engine"], context["engine"]);
    drop(service);
    std::fs::remove_dir_all(dir).unwrap();
    // Direct Runtime users also receive the same metadata pin, not only daemon.
    s["inputs"]["budgets"] = s["manifest"]["budgets"].clone();
    let _ = Runtime::from_fixture(&s).unwrap();
}

#[test]
fn reused_admission_proof_keeps_fresh_instances_pin_and_changed_policy_checks() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let code = program(false);
    let setup = with_wasm(setup(), &code);
    let mut inputs = setup["inputs"].clone();
    inputs["coglet"] = setup["manifest"]["id"].clone();
    inputs["budgets"] = setup["manifest"]["budgets"].clone();
    inputs["grants"] = json!(["expression.set_expression"]);
    let budgets = &setup["manifest"]["budgets"];
    let fresh = || NativeEngine::new(code.clone(), budgets, 8_388_608, 65_536).unwrap();
    // One engine (one cached proof) across steps equals a new engine per step.
    let mut shared = fresh();
    let mut live_a = Runtime::from_fixture(&setup).unwrap();
    let mut live_b = Runtime::from_fixture(&setup).unwrap();
    for step in 0..3 {
        inputs["step"] = json!(step);
        inputs["previous"] = live_a
            .records()
            .last()
            .map(|r| r["record_hash"].clone())
            .unwrap_or(Value::Null);
        let a = live_a.step(&inputs, &mut shared).unwrap();
        let mut inputs_b = inputs.clone();
        inputs_b["previous"] = live_b
            .records()
            .last()
            .map(|r| r["record_hash"].clone())
            .unwrap_or(Value::Null);
        let b = live_b.step(&inputs_b, &mut fresh()).unwrap();
        assert_eq!(a["state_hash"], b["state_hash"]);
        assert_eq!(
            a["journal"]["usage"]["compute"],
            b["journal"]["usage"]["compute"]
        );
    }
    // Changed per-step memory policy is rechecked against the cached proof.
    let mut lower = inputs.clone();
    lower["budgets"]["memory_pages"] = json!(1);
    assert!(
        Runtime::from_fixture(&setup)
            .unwrap()
            .step(&lower, &mut shared)
            .is_err()
    );
    // Changed compute mode is refused even though the proof is cached.
    let mut mode = inputs.clone();
    mode["budgets"]["compute_mode"] = json!("unqualified");
    assert!(
        Runtime::from_fixture(&setup)
            .unwrap()
            .step(&mode, &mut shared)
            .is_err()
    );
    // Code pin mismatch: engine proof is for `code`, fixture pins different bytes.
    let other = program(true);
    assert_ne!(other, code);
    let pinned = with_wasm(setup.clone(), &other);
    inputs["step"] = json!(0);
    inputs["previous"] = Value::Null;
    assert!(
        Runtime::from_fixture(&pinned)
            .unwrap()
            .step(&inputs, &mut shared)
            .is_err()
    );
    // A changed runtime byte cap is a new admission and is refused when too small.
    assert!(NativeEngine::new(code.clone(), budgets, 1_024 * 1_024, 65_536).is_ok());
    assert!(NativeEngine::new(code.clone(), budgets, 4, 65_536).is_err());
}
