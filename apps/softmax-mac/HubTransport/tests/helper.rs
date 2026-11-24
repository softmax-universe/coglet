use coglet_mock::hub::MockHub;
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

async fn event(lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>) -> Value {
    let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    serde_json::from_str(&line).unwrap()
}
#[tokio::test]
async fn private_pipe_helper_enroll_drop_reconnect_and_eof() {
    let hub = MockHub::start().await.unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_softmax-hub-link"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    input
        .write_all(format!("{}\n", json!({"key":"5a".repeat(32)})).as_bytes())
        .await
        .unwrap();
    let ready = event(&mut lines).await;
    assert_eq!(ready["state"], "ready");
    let id = ready["device"].as_str().unwrap().parse().unwrap();
    let invite = hub
        .invite(id, Duration::from_secs(60), vec![])
        .await
        .unwrap();
    let mut config = json!({"hub":hub.id().to_string(),"addresses":hub.addr().ip_addrs().map(|a| a.to_string()).collect::<Vec<_>>(),"invite":invite});
    input
        .write_all(format!("{config}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(event(&mut lines).await["state"], "connected");
    input.write_all(b"{}\n").await.unwrap();
    assert_eq!(event(&mut lines).await["state"], "offline");
    config["invite"] = Value::Null;
    input
        .write_all(format!("{config}\n").as_bytes())
        .await
        .unwrap();
    assert_eq!(event(&mut lines).await["state"], "connected");
    drop(input);
    assert!(
        tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    hub.shutdown().await;
}
