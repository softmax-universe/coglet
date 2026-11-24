#![cfg(any(target_os = "linux", target_os = "macos"))]
use coglet_runtime::{
    SurfaceBinding, cbor,
    socket::{SurfaceListener, serve},
};
use serde_json::json;
use std::{
    io::{Read, Write},
    os::unix::{fs::PermissionsExt, net::UnixStream},
    time::Duration,
};
fn binding() -> SurfaceBinding {
    SurfaceBinding::new(format!("surface:{}", "a".repeat(64)), vec![]).unwrap()
}
fn frame() -> Vec<u8> {
    let bytes = cbor(&json!({"type":"hello","body":{"surface":binding().surface(),"clock":0,"offer":{"versions":["0.1"]},"correlation_id":"1".repeat(32)}})).unwrap();
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    frame
}
#[test]
fn fragmented_header_payload_and_multiple_frames_have_identical_body() {
    let (mut client, server) = UnixStream::pair().unwrap();
    let thread = std::thread::spawn(move || serve(server, unsafe { libc::geteuid() }, binding()));
    for byte in frame() {
        client.write_all(&[byte]).unwrap();
    }
    let mut header = [0; 4];
    client.read_exact(&mut header).unwrap();
    let mut raw = vec![0; u32::from_be_bytes(header) as usize];
    client.read_exact(&mut raw).unwrap();
    let expected = cbor(&json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":"1".repeat(32)}})).unwrap();
    assert_eq!(raw, expected);
    // Two complete messages in one write: first reply then terminal close.
    let mut frames = frame();
    frames.extend(frame());
    client.write_all(&frames).unwrap();
    drop(client);
    assert!(thread.join().unwrap().is_err());
}
#[test]
fn oversize_zero_partial_header_partial_body_and_uid_mismatch_close() {
    let full = frame();
    for bytes in [
        vec![0, 0, 0, 0],
        (1_048_577u32).to_be_bytes().to_vec(),
        vec![0, 0],
        full[..full.len() - 1].to_vec(),
    ] {
        let (mut client, server) = UnixStream::pair().unwrap();
        let thread =
            std::thread::spawn(move || serve(server, unsafe { libc::geteuid() }, binding()));
        client.write_all(&bytes).unwrap();
        client.shutdown(std::net::Shutdown::Write).unwrap();
        let mut reply = Vec::new();
        client.read_to_end(&mut reply).unwrap();
        assert!(reply.is_empty());
        assert!(thread.join().unwrap().is_err());
    }
    let (mut client, server) = UnixStream::pair().unwrap();
    let wrong_uid = unsafe { libc::geteuid() }.wrapping_add(1);
    let thread = std::thread::spawn(move || serve(server, wrong_uid, binding()));
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut byte = [0];
    assert_eq!(client.read(&mut byte).unwrap(), 0);
    assert!(thread.join().unwrap().is_err());
}
#[test]
fn idle_socket_emits_heartbeats_without_incoming_frames_and_disconnects() {
    let (mut client, server) = UnixStream::pair().unwrap();
    let thread = std::thread::spawn(move || serve(server, unsafe { libc::geteuid() }, binding()));
    client
        .set_read_timeout(Some(Duration::from_secs(4)))
        .unwrap();
    client.write_all(&frame()).unwrap();
    for expected in ["hello_reply", "health", "health"] {
        let mut header = [0; 4];
        client.read_exact(&mut header).unwrap();
        let mut raw = vec![0; u32::from_be_bytes(header) as usize];
        client.read_exact(&mut raw).unwrap();
        let message: serde_json::Value = coglet_types::decode_profile(&raw)
            .unwrap()
            .deserialized()
            .unwrap();
        assert_eq!(message["type"], expected);
    }
    let mut byte = [0];
    assert_eq!(client.read(&mut byte).unwrap(), 0);
    assert!(thread.join().unwrap().is_err());
}
#[test]
fn listener_modes_existing_socket_and_insecure_parent_fail_closed() {
    let base = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(
                std::env::var_os("HOME").expect("test requires HOME or TMPDIR"),
            )
            .join(".cache")
        });
    std::fs::create_dir_all(&base).unwrap();
    let dir = base.join(format!("runtime-socket-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = dir.join("s");
    assert!(SurfaceListener::bind(&path, unsafe { libc::geteuid() }, binding()).is_err());
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let listener = SurfaceListener::bind(&path, unsafe { libc::geteuid() }, binding()).unwrap();
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert!(SurfaceListener::bind(&path, unsafe { libc::geteuid() }, binding()).is_err());
    drop(listener);
    assert!(!path.exists());
    std::fs::remove_dir(dir).unwrap();
}
