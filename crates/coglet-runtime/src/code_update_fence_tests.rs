use super::*;

#[cfg(feature = "native-engine")]
fn pin_wasm(setup: &mut Value, wasm: &[u8]) {
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
}

#[cfg(feature = "native-engine")]
pub(super) fn data_writer_setup(mut setup: Value) -> Value {
    let wasm = wat::parse_str(r#"(module
      (import "wasi_snapshot_preview1" "path_open" (func $open (param i32 i32 i32 i32 i32 i64 i64 i32 i32) (result i32)))
      (import "wasi_snapshot_preview1" "fd_write" (func $write (param i32 i32 i32 i32) (result i32)))
      (memory (export "memory") 1 2)
      (data (i32.const 0) "private") (data (i32.const 16) "keep me")
      (func (export "abi_alloc") (param i32) (result i32) i32.const 1024)
      (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
        i32.const 64 i32.const 16 i32.store
        i32.const 68 i32.const 7 i32.store
        i32.const 4 i32.const 0 i32.const 0 i32.const 7 i32.const 9
        i64.const 64 i64.const 0 i32.const 0 i32.const 80 call $open drop
        i32.const 80 i32.load i32.const 64 i32.const 1 i32.const 84 call $write drop
        i32.const 0))"#).unwrap();
    pin_wasm(&mut setup, &wasm);
    setup["manifest"]["budgets"]["compute_mode"] = json!(coglet_engine_wamr::COMPUTE_MODE);
    setup["manifest"]["budgets"]["compute"] = json!(100_000);
    setup["manifest"]["budgets"]["wall_time_us"] = json!(2_000_000);
    setup["inputs"]["budgets"] = setup["manifest"]["budgets"].clone();
    setup
}

#[test]
#[cfg(feature = "native-engine")]
fn installed_code_cut_waits_for_detached_reservation_before_unresolved_fence() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let _run = service.prepare_detached(&setup["inputs"]).unwrap().unwrap();
    assert!(service.state.runtime.unresolved_attempt.is_none());
    let before = serde_json::to_value(&service.state).unwrap();
    let error = service
        .complete_code_revision(&revision(&setup, 1), &|| {
            panic!("no code cut during detached engine custody")
        })
        .unwrap_err();
    assert!(
        error.starts_with("unavailable: detached runtime custody"),
        "{error}"
    );
    assert_eq!(serde_json::to_value(&service.state).unwrap(), before);
    assert!(service.state.completed_segments.is_empty());
    service.detached_execution = false;
    let applied = service
        .complete_code_revision(&revision(&setup, 1), &|| Ok(0))
        .unwrap();
    assert_eq!(applied["state"], "applied");
    assert_eq!(applied["operation_id"], format!("{:032x}", 1));
}

#[test]
fn installed_code_pending_fences_next_step_across_reopen() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    service.fence_code(true).unwrap();
    assert!(service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone())
        )
        .unwrap()
        .is_none());
    assert!(service.records().is_empty());
    drop(service);
    let mut service = RuntimeService::open(&root.path().join("state.wal"), &setup, 8).unwrap();
    assert!(service.code_pending());
    assert!(service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone())
        )
        .unwrap()
        .is_none());
    service.fence_code(false).unwrap();
    assert!(service
        .run_next(
            &setup["inputs"],
            &mut MockEngine::new(setup["guest_script"].clone())
        )
        .unwrap()
        .is_some());
}

#[test]
fn installed_code_empty_stage_preserves_source_reservations_across_reopen() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    let mut next = service.state.clone();
    let mut reserved = setup["inputs"]["events"][0].clone();
    reserved["source"] = next.runtime.manifest["id"].clone();
    reserved["seq"] = json!(0);
    next.router.enqueue(reserved).unwrap();
    next.runtime.next_source_seq = 1;
    next.staged_boundary = Some(StagedBoundary {
        changes: Vec::new(),
        runtime: next.runtime.clone(),
    });
    service.commit_managed(next).unwrap();
    let receipt = service
        .complete_code_revision(&revision(&setup, 1), &|| Ok(0))
        .unwrap();
    assert_eq!(receipt["state"], "applied");
    assert!(service.records().is_empty());
    assert!(service.state.staged_boundary.is_none());
    assert_eq!(service.state.runtime.next_source_seq, 1);
    drop(service);
    let recovered = RuntimeService::open(&root.path().join("state.wal"), &setup, 8).unwrap();
    assert_eq!(recovered.state.runtime.next_source_seq, 1);
    assert_eq!(
        recovered.managed_status()["live_commit"],
        format!("{:040x}", 1)
    );
}

#[test]
fn installed_code_receipt_pressure_is_no_effect_and_preserves_stop_slot() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    let mut next = service.state.clone();
    let managed = next.managed.as_mut().unwrap();
    let update = revision(&setup, 1);
    for n in 100..100 + OPERATION_LEDGER - 1 {
        let id = format!("{n:032x}");
        managed.operations.insert(
            id.clone(),
            OperationReceipt {
                intent: format!("{n:064x}"),
                scope: Some(update.scope.clone()),
                receipt: json!({"operation_id":id,"state":"failed","step":null,
                "error":{"code":"conflict","message":"test retained refusal","retryable":false}}),
            },
        );
    }
    service.commit_managed(next).unwrap();
    let before = serde_json::to_value(&service.state).unwrap();
    let error = service
        .complete_code_revision(&update, &|| panic!("pressure must refuse before effect"))
        .unwrap_err();
    assert!(error.starts_with("resource_exhausted: "), "{error}");
    assert_eq!(serde_json::to_value(&service.state).unwrap(), before);
    assert_eq!(
        service.state.managed.as_ref().unwrap().operations.len(),
        OPERATION_LEDGER - 1
    );
    drop(service);
    RuntimeService::open(&root.path().join("state.wal"), &setup, 8).unwrap();
}

#[cfg(feature = "native-engine")]
#[test]
fn installed_code_replaces_real_native_engine_and_terminalizes_unqualified_module() {
    let root = TempDir::new().unwrap();
    let mut setup = setup();
    let wasm = wat::parse_str("(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))").unwrap();
    setup["manifest"]["budgets"]["compute_mode"] = json!(
        coglet_engine::Engine::metadata(&coglet_engine_wamr::Wamr::interpreter()).compute_mode
    );
    setup["manifest"]["budgets"]["wall_time_us"] = json!(2_000_000);
    pin_wasm(&mut setup, &wasm);
    let mut service = service(&root, &setup);
    service
        .enable_native(native_engine::NativeSchedule {
            trace_id: setup["inputs"]["trace_id"].as_str().unwrap().into(),
            seed: 0,
            runtime_bytes: 8_388_608,
            stack_bytes: 65_536,
        })
        .unwrap();
    service.managed_start().unwrap();
    let mut bad = revision(&setup, 1);
    // Transfer validation accepts core Wasm; native execution admits no ambient
    // imports. This deterministic qualification failure must be terminal.
    pin_wasm(
        &mut bad.setup,
        &wat::parse_str("(module (import \"foreign\" \"effect\" (func)))").unwrap(),
    );
    let failed = service.complete_code_revision(&bad, &|| Ok(0)).unwrap();
    assert_eq!(failed["state"], "failed");
    assert_eq!(failed["error"]["code"], "invalid_argument");
    let replacement = wat::parse_str("(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) nop i32.const 0))").unwrap();
    let mut update = revision(&setup, 2);
    pin_wasm(&mut update.setup, &replacement);
    service.apply_code_revision(&update, &|| Ok(0)).unwrap();
    let (config, mut engine) = service.native.take().unwrap();
    let mut inputs = setup["inputs"].clone();
    inputs["trace_id"] = json!(config.trace_id);
    inputs["budgets"] = setup["manifest"]["budgets"].clone();
    service.enqueue(inputs["events"][0].clone()).unwrap();
    let record = service.run_next(&inputs, &mut engine).unwrap().unwrap();
    assert_eq!(record["journal"]["code_hash"], digest(&replacement));
    assert!(record["journal"]["usage"]["compute"].as_u64().unwrap() > 0);
}
