use super::*;
use tempfile::TempDir;

fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["initial_files"]
        .as_object_mut()
        .unwrap()
        .retain(|p, _| !p.starts_with("/data/"));
    setup
}
fn revision(setup: &Value, n: u64) -> CodeRevision {
    let mut code = setup["initial_files"].clone();
    code["/code/revision.txt"] = json!(hex::encode(n.to_string()));
    CodeRevision {
        setup: json!({"manifest": setup["manifest"], "initial_files": code}),
        signed_ref: json!({"statement":{"new":format!("{n:040x}"),"generation":n}}),
        operation_id: format!("{n:032x}"),
        intent: format!("{n:064x}"),
        scope: ReceiptScope {
            actor: "hub:test".into(),
            ticket: None,
            method: "hub.git.push".into(),
        },
    }
}
fn service(root: &TempDir, setup: &Value) -> RuntimeService {
    let mut service = RuntimeService::open(&root.path().join("state.wal"), setup, 8).unwrap();
    service
        .managed_install(&json!({"live_commit":"0".repeat(40)}), &"f".repeat(32))
        .unwrap();
    service
}

#[test]
fn installed_push_preserves_private_data() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    service.managed_start().unwrap();
    service
        .enqueue(setup["inputs"]["events"][0].clone())
        .unwrap();
    let mut script = setup["guest_script"].clone();
    script["writes"]["/data/private"] = json!(hex::encode("keep me"));
    service
        .run_next(&setup["inputs"], &mut MockEngine::new(script))
        .unwrap()
        .unwrap();
    let old_trace = setup["inputs"]["trace_id"].clone();
    let receipt = service
        .apply_code_revision(&revision(&setup, 1), &|| Ok(0))
        .unwrap();
    assert_eq!(receipt["state"], "applied");
    assert_eq!(receipt["operation_id"], format!("{:032x}", 1));
    assert_eq!(
        service.state.runtime.files.files()["/data/private"],
        hex::encode("keep me")
    );
    assert_eq!(
        service.managed_status()["live_commit"],
        format!("{:040x}", 1)
    );
    let old = service.export_segment(&old_trace).unwrap();
    assert_eq!(old["records"].as_array().unwrap().len(), 1);
    assert!(
        !old["initial_files"]
            .as_object()
            .unwrap()
            .contains_key("/code/revision.txt")
    );
    Runtime::import_trace(&old).unwrap();
    let current_id = service.state.runtime.trace_id.clone().unwrap();
    let new = service
        .export_trace(&current_id, service.state.runtime.time)
        .unwrap();
    Runtime::import_trace(&new).unwrap();
    assert_eq!(
        new["initial_files"]["/data/private"],
        hex::encode("keep me")
    );
    drop(service);
    let mut recovered = RuntimeService::open(&root.path().join("state.wal"), &setup, 8).unwrap();
    assert_eq!(
        recovered.managed_status()["live_commit"],
        format!("{:040x}", 1)
    );
    assert_eq!(
        recovered
            .apply_code_revision(&revision(&setup, 1), &|| panic!(
                "confirmed effect must not be retried"
            ))
            .unwrap(),
        receipt
    );
    assert_eq!(recovered.records().len(), 1);
}

#[test]
fn installed_code_multiple_stopped_cuts_keep_empty_segments_and_root_pin() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    for n in 1..=3 {
        service
            .apply_code_revision(&revision(&setup, n), &|| Ok(0))
            .unwrap();
    }
    assert_eq!(service.state.completed_segments.len(), 3);
    assert_eq!(
        service.managed_setup(),
        &service.state.completed_segments[0].initial_setup
    );
    drop(service);
    let service = RuntimeService::open(&root.path().join("state.wal"), &setup, 8).unwrap();
    assert_eq!(service.managed_status()["state"], "stopped");
    assert_eq!(
        service.managed_status()["live_commit"],
        format!("{:040x}", 3)
    );
    for segment in &service.state.completed_segments {
        Runtime::import_trace(&service.export_segment(&segment.trace_id).unwrap()).unwrap();
    }
}

#[test]
fn installed_code_cut_refuses_private_overlay_stale_generation_and_expired_gate() {
    let root = TempDir::new().unwrap();
    let setup = setup();
    let mut service = service(&root, &setup);
    let mut bad = revision(&setup, 1);
    bad.setup["initial_files"]["/data/stolen"] = json!("aa");
    assert!(service.apply_code_revision(&bad, &|| Ok(0)).is_err());
    assert!(
        service
            .apply_code_revision(&revision(&setup, 1), &|| Err("permission_denied".into()))
            .is_err()
    );
    assert_eq!(service.managed_status()["live_commit"], "0".repeat(40));
    service
        .apply_code_revision(&revision(&setup, 2), &|| Ok(0))
        .unwrap();
    assert!(
        service
            .apply_code_revision(&revision(&setup, 1), &|| Ok(0))
            .is_err()
    );
    assert_eq!(
        service.managed_status()["live_commit"],
        format!("{:040x}", 2)
    );
}
