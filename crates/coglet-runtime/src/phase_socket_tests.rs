use super::*;
use crate::phase_evidence::Phase;
use std::sync::{Arc, Mutex};

#[test]
fn phase_socket_failure_retains_read_and_teardown_order_without_masking() {
    let dir = tempfile::tempdir().unwrap();
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let mut service =
        crate::RuntimeService::open(&dir.path().join("socket.wal"), &setup, 8).unwrap();
    service.enable_phase_sink();
    let sink = service.phase_sink().unwrap();
    let service = Arc::new(Mutex::new(service));
    let binding = SurfaceBinding::new(
        format!("surface:{}", "a".repeat(64)),
        vec![setup["manifest"]["id"].as_str().unwrap().into()],
    )
    .unwrap();
    let (mut peer, mut stream) = UnixStream::pair().unwrap();
    peer.write_all(&0u32.to_be_bytes()).unwrap();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let mut out = Vec::new();
    let result = serve_with_service_to(&mut stream, uid, binding, Some(service), &mut out);
    assert_eq!(result, Err("invalid surface frame length".into()));
    assert!(!out.is_empty(), "missing socket failure phase dump");
    assert!(out.len() <= 4096);
    let snap = sink.snapshot();
    let phases: Vec<_> = snap.entries[..snap.count].iter().map(|e| e.phase).collect();
    assert_eq!(phases.first(), Some(&Phase::SessionStart));
    let read = phases
        .iter()
        .position(|p| *p == Phase::HeaderReturn)
        .unwrap();
    let disconnect = phases
        .iter()
        .position(|p| *p == Phase::DisconnectEnter)
        .unwrap();
    let returned = phases
        .iter()
        .position(|p| *p == Phase::ReturnError)
        .unwrap();
    assert!(read < disconnect && disconnect < returned);
    assert_eq!(phases.last(), Some(&Phase::DisconnectReturn));
    let text = std::str::from_utf8(&out).unwrap();
    assert!(text.contains("native-phase-v1"));

    // A real mid-body EOF returns through `?`; its return observation must
    // survive without changing the original transport error.
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let mut service = crate::RuntimeService::open(&dir.path().join("body.wal"), &setup, 8).unwrap();
    service.enable_phase_sink();
    let sink = service.phase_sink().unwrap();
    let binding = SurfaceBinding::new(
        format!("surface:{}", "a".repeat(64)),
        vec![setup["manifest"]["id"].as_str().unwrap().into()],
    )
    .unwrap();
    let (mut peer, mut stream) = UnixStream::pair().unwrap();
    peer.write_all(&4u32.to_be_bytes()).unwrap();
    peer.write_all(&[0, 0]).unwrap();
    peer.shutdown(std::net::Shutdown::Write).unwrap();
    let result = serve_with_service_to(
        &mut stream,
        uid,
        binding,
        Some(Arc::new(Mutex::new(service))),
        &mut Vec::new(),
    );
    assert_eq!(result, Err("EOF mid-frame".into()));
    let snap = sink.snapshot();
    assert!(
        snap.entries[..snap.count]
            .iter()
            .any(|e| e.phase == Phase::BodyReturn),
        "missing error-return marker"
    );
}
