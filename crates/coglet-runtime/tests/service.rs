use coglet_runtime::{MockEngine, RuntimeService};
use serde_json::Value;
use std::fs;

#[test]
fn restart_preserves_arrival_order_and_consumes_durable_attempt_only_once() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let send = &cases[1]["steps"][0]["send"];
    let directory = std::env::temp_dir().join(format!("coglet-service-{}", std::process::id()));
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join("runtime.wal");
    let mut service = RuntimeService::open(&path, send, 4).unwrap();
    service
        .enqueue(send["inputs"]["events"][0].clone())
        .unwrap();
    assert_eq!(service.queued(), 1);
    drop(service);
    let mut service = RuntimeService::open(&path, send, 4).unwrap();
    assert_eq!(service.queued(), 1);
    let recorded = service
        .run_next(
            &send["inputs"],
            &mut MockEngine::new(send["guest_script"].clone()),
        )
        .unwrap()
        .unwrap();
    assert_eq!(recorded["journal"]["step"], 0);
    assert_eq!(service.queued(), 0);
    drop(service);
    let mut service = RuntimeService::open(&path, send, 4).unwrap();
    assert_eq!(service.queued(), 0);
    assert!(
        service
            .run_next(
                &send["inputs"],
                &mut MockEngine::new(send["guest_script"].clone())
            )
            .unwrap()
            .is_none()
    );
    assert_eq!(service.records().len(), 1);
    assert_eq!(service.records()[0], recorded["journal"]);
    drop(service);
    fs::remove_dir_all(directory).unwrap();
}
