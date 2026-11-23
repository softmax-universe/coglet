#![cfg(unix)]
mod common;
use coglet_surface_rust::{
    Surface,
    config::AdapterFixture,
    reference::SemanticBackend,
    transport::{SocketConnection, read_frame, write_frame},
};
use common::*;
use serde_json::json;
use std::{
    os::unix::{
        fs::{MetadataExt, PermissionsExt},
        net::UnixListener,
    },
    time::Duration,
};

#[test]
fn real_surface_over_authenticated_fragmented_unix_socket() {
    let dir = std::env::temp_dir().join(format!(
        "ss-{}",
        &coglet_surface_rust::ids::random_id().unwrap()[..12]
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let uid = std::fs::metadata(&socket).unwrap().uid();
    let f = AdapterFixture::parse(
        include_str!("../../../spec/fixtures/surface/adapter-config.json"),
        None,
    )
    .unwrap();
    let backend = SemanticBackend::new(f.speech, f.expression);
    let peer = Surface::open(f.config, &dir.join("state"), backend.clone()).unwrap();
    let runtime = std::thread::spawn(move || {
        use std::io::Write;
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let hello = decode(&read_frame(&mut stream).unwrap().unwrap());
        assert_eq!(hello["type"], "hello");
        let reply = encode(
            &json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hello["body"]["correlation_id"]}}),
        );
        let mut frame = (reply.len() as u32).to_be_bytes().to_vec();
        frame.extend(reply);
        for byte in frame {
            stream.write_all(&[byte]).unwrap();
        }
        assert_eq!(
            decode(&read_frame(&mut stream).unwrap().unwrap())["type"],
            "capabilities"
        );
        assert_eq!(
            decode(&read_frame(&mut stream).unwrap().unwrap())["type"],
            "grants"
        );
        let act = json!({"type":"act","body":{"grant_epoch":1,"action":{"id":hid(42),"coglet":B,"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":1000},"deadline":1_000_000,"correlation_id":hid(42)}}});
        write_frame(&mut stream, &encode(&act)).unwrap();
        assert_eq!(
            decode(&read_frame(&mut stream).unwrap().unwrap())["body"]["accepted"],
            true
        );
        assert_eq!(
            decode(&read_frame(&mut stream).unwrap().unwrap())["body"]["payload"]["status"],
            "succeeded"
        );
        // Malformed CBOR goes through the actual decoder/core, never to the backend.
        write_frame(&mut stream, &[0xff]).unwrap();
        let _ = read_frame(&mut stream);
    });
    let mut link = SocketConnection::connect(&socket, uid, peer, 0).unwrap();
    for now in 1..=1000 {
        if link.poll(now).is_err() {
            break;
        }
    }
    runtime.join().unwrap();
    drop(link);
    assert_eq!(
        backend.state(1001).0,
        "neutral",
        "fatal wire error applies native disconnect"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn real_surface_rejects_wrong_socket_uid_before_hello() {
    let dir = std::env::temp_dir().join(format!(
        "sd-{}",
        &coglet_surface_rust::ids::random_id().unwrap()[..12]
    ));
    std::fs::create_dir(&dir).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    let socket = dir.join("control.sock");
    let listener = UnixListener::bind(&socket).unwrap();
    std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600)).unwrap();
    let uid = std::fs::metadata(&socket).unwrap().uid();
    let f = AdapterFixture::parse(
        include_str!("../../../spec/fixtures/surface/adapter-config.json"),
        None,
    )
    .unwrap();
    let peer = Surface::open(
        f.config,
        &dir.join("state"),
        SemanticBackend::new(f.speech, f.expression),
    )
    .unwrap();
    assert!(SocketConnection::connect(&socket, uid.wrapping_add(1), peer, 0).is_err());
    let (mut stream, _) = listener.accept().unwrap();
    assert!(
        read_frame(&mut stream).unwrap().is_none(),
        "no unauthenticated capability/device bytes"
    );
    drop(listener);
    std::fs::remove_dir_all(dir).unwrap();
}
