// Executed profile tests: production iOS epoch, real WAL and real subprocesses.
// On Linux only elapsed time comes from BOOTTIME, not Apple kernel APIs.
fn process_profile_sample() -> Result<(String, u64)> {
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    let micros = apple_continuous_micros()?;
    #[cfg(not(any(target_os = "macos", target_os = "ios")))]
    let micros = sample()?.1;
    Ok((ios_process_epoch()?, micros))
}
fn process_profile(service: &mut RuntimeService) {
    service.test_clock_sample = Some(std::sync::Arc::new(process_profile_sample));
}
#[test]
fn ios_process_epoch_is_stable_and_entropy_failure_refuses() {
    use std::sync::{
        OnceLock,
        atomic::{AtomicUsize, Ordering},
    };
    let epoch = OnceLock::new();
    let calls = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..16)
            .map(|_| {
                scope.spawn(|| {
                    process_epoch(&epoch, |bytes| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        os_entropy(bytes)
                    })
                    .unwrap()
                })
            })
            .collect();
        let values: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(values.iter().all(|v| v == &values[0]));
        assert!(values[0].starts_with("ios-process:"));
        assert_eq!(values[0].len(), "ios-process:".len() + 64);
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    let failed = OnceLock::new();
    assert_eq!(
        process_epoch(&failed, |_| Err("entropy denied".into())),
        Err("entropy denied".into())
    );
    assert_eq!(
        process_epoch(&failed, |_| panic!("failed epoch must not retry")),
        Err("entropy denied".into())
    );
    assert_eq!(ios_process_epoch().unwrap(), ios_process_epoch().unwrap());
}
#[test]
fn same_process_wal_reopen_preserves_epoch_and_elapsed_time() {
    let (dir, setup) = fixture("same-process");
    let path = dir.join("wal");
    let mut service = RuntimeService::open(&path, &setup, 128).unwrap();
    process_profile(&mut service);
    let first = service.supervised_now().unwrap();
    let anchor = serde_json::to_value(&service.state.clock_anchor).unwrap();
    let runtime = serde_json::to_value(&service.state).unwrap()["runtime"].clone();
    drop(service);
    std::thread::sleep(std::time::Duration::from_millis(3));
    let mut recovered = RuntimeService::open(&path, &setup, 128).unwrap();
    process_profile(&mut recovered);
    assert!(recovered.supervised_now().unwrap() >= first + 3_000);
    assert_eq!(
        serde_json::to_value(&recovered.state.clock_anchor).unwrap(),
        anchor
    );
    assert_eq!(
        serde_json::to_value(&recovered.state).unwrap()["runtime"],
        runtime
    );
    drop(recovered);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn ios_anchored_wal_refuses_another_process_before_guest_admission() {
    const ENV: &str = "COGLET_IOS_PROCESS_CLOCK_CHILD_WAL";
    if let Some(path) = std::env::var_os(ENV) {
        let path = std::path::PathBuf::from(path);
        let setup: Value = serde_json::from_slice(
            &std::fs::read(std::path::Path::new(&path).with_extension("setup")).unwrap(),
        )
        .unwrap();
        let before = std::fs::read(&path).unwrap();
        let mut service = RuntimeService::open(&path, &setup, 128).unwrap();
        process_profile(&mut service);
        let old_epoch = service.state.clock_anchor.as_ref().unwrap().boot.clone();
        assert_ne!(ios_process_epoch().unwrap(), old_epoch);
        // This is the same startup ordering as ffi::coglet_runtime_start:
        // supervised_now must succeed before enable_native/service publication.
        assert!(
            service
                .supervised_now()
                .unwrap_err()
                .contains("continuity unavailable")
        );
        assert!(service.state.dispatches.is_empty());
        assert_eq!(service.state.runtime.step, 0);
        assert_eq!(std::fs::read(&path).unwrap(), before);
        println!("IOS_PROCESS_CLOCK_CHILD_EXECUTED: refused before guest admission");
        return;
    }
    let (dir, setup) = fixture("cross-process");
    let path = dir.join("wal");
    std::fs::write(
        path.with_extension("setup"),
        serde_json::to_vec(&setup).unwrap(),
    )
    .unwrap();
    let mut service = RuntimeService::open(&path, &setup, 128).unwrap();
    process_profile(&mut service);
    service.supervised_now().unwrap();
    drop(service);
    let before = std::fs::read(&path).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "execution::clock::tests::ios_anchored_wal_refuses_another_process_before_guest_admission",
            "--nocapture",
        ])
        .env(ENV, &path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("IOS_PROCESS_CLOCK_CHILD_EXECUTED"),
        "{stdout}"
    );
    assert!(stdout.contains("1 passed"), "{stdout}");
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // A legacy boot-scoped anchor also refuses this process profile unchanged.
    let mut recovered = RuntimeService::open(&path, &setup, 128).unwrap();
    recovered.state.clock_anchor.as_mut().unwrap().boot = "darwin:old-boot".into();
    recovered
        .store
        .commit(&serde_json::to_value(&recovered.state).unwrap(), None)
        .unwrap();
    drop(recovered);
    let before = std::fs::read(&path).unwrap();
    let mut legacy = RuntimeService::open(&path, &setup, 128).unwrap();
    process_profile(&mut legacy);
    assert!(legacy.supervised_now().is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    drop(legacy);
    std::fs::remove_dir_all(dir).unwrap();
}
