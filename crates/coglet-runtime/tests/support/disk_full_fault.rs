use super::*;
struct FillDisk {
    marker: std::path::PathBuf,
    script: Value,
    calls: usize,
}
impl Engine for FillDisk {
    fn execute(
        &mut self,
        input: &Value,
        host: &mut StepHost,
    ) -> coglet_runtime::Result<EngineReport> {
        self.calls += 1;
        let report = MockEngine::new(self.script.clone()).execute(input, host)?;
        std::fs::write(&self.marker, b"inject").unwrap();
        Ok(report)
    }
}
#[test]
fn disk_full_child() {
    let Some(path) = std::env::var_os("COGLET_FAULT_DISK_WAL") else {
        return;
    };
    let marker = std::path::PathBuf::from(std::env::var_os("COGLET_FAULT_DISK_MARKER").unwrap());
    let send = setup();
    let path = Path::new(&path);
    let mut service = RuntimeService::open(path, &send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    let mut engine = FillDisk {
        marker: marker.clone(),
        script: send["guest_script"].clone(),
        calls: 0,
    };
    let error = service.run_next(&send["inputs"], &mut engine).unwrap_err();
    assert!(
        error.contains("No space left on device"),
        "hook did not inject ENOSPC: {error}"
    );
    assert!(service.records().is_empty());
    assert!(service.run_next(&send["inputs"], &mut engine).is_err());
    assert_eq!(engine.calls, 1);
    std::fs::remove_file(marker).unwrap();
    drop(service);
    let mut service = RuntimeService::open(path, &send, 4).unwrap();
    assert!(
        service
            .run_next(&send["inputs"], &mut MustNotExecute)
            .is_err()
    );
    assert!(service.records().is_empty());
    assert_eq!(service.queued(), 1);
    drop(service);
    let store = DurableStore::open(path).unwrap();
    assert_eq!(
        store.state().unwrap()["runtime"]["files"],
        send["initial_files"]
    );
    assert_eq!(
        store.state().unwrap()["runtime"]["pending_actions"],
        json!({})
    );
    assert_eq!(store.state().unwrap()["dispatches"], json!({}));
    println!("ENOSPC injected after guest return; torn result discarded; attempt remains fenced");
}
#[test]
fn runtime_disk_full_during_result_commit_never_dispatches_or_reexecutes() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("enospc.so");
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/enospc.c");
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
            .arg(source)
            .arg("-o")
            .arg(&library)
            .status()
            .unwrap()
            .success()
    );
    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "disk_full_fault::disk_full_child", "--nocapture"])
        .env("LD_PRELOAD", library)
        .env("COGLET_FAULT_DISK_WAL", dir.path().join("runtime.wal"))
        .env("COGLET_FAULT_DISK_MARKER", dir.path().join("armed"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("ENOSPC injected after guest return"),
        "child test did not run: {stdout}"
    );
}
