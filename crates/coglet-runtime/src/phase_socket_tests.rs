use super::*;
use crate::phase_evidence::Phase;
use std::sync::{Arc, Mutex};

fn phase_output_gate_case(kind: &str) {
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
    let mut service = crate::RuntimeService::open(&dir.path().join("gate.wal"), &setup, 8).unwrap();
    service.enable_phase_sink();
    let sink = service.phase_sink().unwrap();
    let surface = format!("surface:{}", "a".repeat(64));
    let binding = SurfaceBinding::new(
        surface.clone(),
        vec![setup["manifest"]["id"].as_str().unwrap().into()],
    )
    .unwrap();
    let gate = (kind != "ungated").then(OutputGate::new_closed);
    if kind == "open" || kind == "revoked" {
        gate.as_ref().unwrap().enable().unwrap();
    }
    if kind == "revoked" {
        gate.as_ref().unwrap().revoke().unwrap();
    }
    let hello = crate::cbor(&serde_json::json!({"type":"hello","body":{
        "surface":surface,"offer":{"versions":["0.1"]},"clock":1000,
        "correlation_id":"1".repeat(32)}}))
    .unwrap();
    let (mut peer, mut stream) = UnixStream::pair().unwrap();
    peer.write_all(&(hello.len() as u32).to_be_bytes()).unwrap();
    peer.write_all(&hello).unwrap();
    peer.shutdown(std::net::Shutdown::Write).unwrap();
    // SAFETY: geteuid has no preconditions.
    let uid = unsafe { libc::geteuid() };
    let mut dump = Vec::new();
    let result = serve_with_service_to(
        &mut stream,
        uid,
        binding,
        Some(Arc::new(Mutex::new(service))),
        gate,
        &mut dump,
    );
    let refused = kind == "closed" || kind == "revoked";
    assert_eq!(
        result,
        if refused {
            Err("unavailable: output lease closed".into())
        } else {
            Ok(())
        }
    );
    let snap = sink.snapshot();
    let phases: Vec<_> = snap.entries[..snap.count].iter().map(|e| e.phase).collect();
    let enter = phases.iter().position(|p| *p == Phase::WireEnter).unwrap();
    let returned = phases.iter().position(|p| *p == Phase::WireReturn).unwrap();
    assert!(enter < returned);
    if refused {
        let disconnected = phases
            .iter()
            .position(|p| *p == Phase::DisconnectEnter)
            .unwrap();
        let error = phases
            .iter()
            .position(|p| *p == Phase::ReturnError)
            .unwrap();
        assert!(
            returned < disconnected && disconnected < error,
            "flush must disconnect before returning its error: {phases:?}"
        );
        let text = std::str::from_utf8(&dump).unwrap();
        assert!(text.contains("native-phase-v1"));
        assert!(!dump.is_empty() && dump.len() <= 4096);
    } else {
        assert!(phases.contains(&Phase::ReturnEof));
        assert!(!phases.contains(&Phase::ReturnError));
        assert!(dump.is_empty());
    }
    drop(stream);
    let mut bytes = Vec::new();
    peer.read_to_end(&mut bytes).unwrap();
    if refused {
        assert!(bytes.is_empty(), "refused frame exposed bytes");
    } else {
        assert!(bytes.len() >= 4);
        let n = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert_eq!(bytes.len(), 4 + n);
        let reply: serde_json::Value = coglet_types::decode("surface-message", &bytes[4..])
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(reply["type"], "hello_reply");
    }
}

#[test]
fn phase_output_gate_open_marks_physical_write() {
    phase_output_gate_case("open");
}
#[test]
fn phase_output_gate_closed_marks_refusal_and_disconnect() {
    phase_output_gate_case("closed");
}
#[test]
fn phase_output_gate_revoked_marks_refusal_and_disconnect() {
    phase_output_gate_case("revoked");
}
#[test]
fn phase_output_gate_ungated_marks_physical_write() {
    phase_output_gate_case("ungated");
}

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
    let result = serve_with_service_to(&mut stream, uid, binding, Some(service), None, &mut out);
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
        None,
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
