use std::{
    io::{BufRead, BufReader},
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};
struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[test]
fn serve_starts_excludes_concurrent_identity_writers_and_stops_cleanly() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let config = dir.path().join("config.json");
    std::fs::write(&config, b"{}").unwrap();
    let bin = env!("CARGO_BIN_EXE_coglet-hub");
    let mut server = Server(
        Command::new(bin)
            .arg("serve")
            .arg(&state)
            .arg(&config)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap(),
    );
    let stdout = server.0.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        let result = BufReader::new(stdout).read_line(&mut line);
        let _ = tx.send((result, line));
    });
    let (result, line) = rx
        .recv_timeout(Duration::from_secs(10))
        .expect("server did not publish startup descriptor");
    result.unwrap();
    reader.join().unwrap();
    let info: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(info["hub"].as_str().unwrap().len(), 64);
    assert_eq!(info["invites"], serde_json::json!({}));
    let concurrent = Command::new(bin)
        .arg("identity")
        .arg(&state)
        .output()
        .unwrap();
    assert!(
        !concurrent.status.success(),
        "concurrent process acquired state while server owns it"
    );
    assert!(
        Command::new("kill")
            .args(["-INT", &server.0.id().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = server.0.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        assert!(Instant::now() < deadline, "SIGINT shutdown stalled");
        std::thread::sleep(Duration::from_millis(10));
    }
    let reopened = Command::new(bin)
        .arg("identity")
        .arg(&state)
        .output()
        .unwrap();
    assert!(reopened.status.success());
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&reopened.stdout).unwrap()["hub"],
        info["hub"]
    );
}
