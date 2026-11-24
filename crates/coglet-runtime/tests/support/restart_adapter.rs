//! Actual adapter frames trigger the installed daemon's native Wasm scheduler.
use super::*;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

pub(super) fn catalog() -> Value {
    json!([{"name":"speech.control","events":["speech.start"],"reads":[],"actions":[],"exclusive":["audio.mic","audio.speaker"],"schemas":{"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start"}}])
}

async fn send(link: &mut UnixStream, kind: &str, body: Value) {
    let raw = coglet_runtime::cbor(&json!({"type":kind,"body":body})).unwrap();
    link.write_u32(raw.len() as u32).await.unwrap();
    link.write_all(&raw).await.unwrap();
}

pub(super) async fn events(
    path: &Path,
    surface: &str,
    coglet: &str,
    epoch: u64,
    first_seq: u64,
) -> UnixStream {
    let mut link = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match UnixStream::connect(path).await {
                Ok(link) => break link,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    tokio::task::yield_now().await
                }
                Err(e) => panic!("adapter connect failed: {e}"),
            }
        }
    })
    .await
    .unwrap_or_else(|e| {
        panic!(
            "adapter listener readiness at {}: {e}; daemon log: {}",
            path.display(),
            std::fs::read_to_string(
                path.parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .join("child/child.log")
            )
            .unwrap_or_default()
        )
    });
    send(&mut link, "hello", json!({"surface":surface,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"a".repeat(32)})).await;
    let n = link.read_u32().await.unwrap();
    assert!(n <= 1_048_576);
    let mut bytes = vec![0; n as usize];
    link.read_exact(&mut bytes).await.unwrap();
    let hello: Value = coglet_types::decode("surface-message", &bytes)
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(hello["type"], "hello_reply", "{hello}");
    send(
        &mut link,
        "capabilities",
        json!({"revision":1,"clock":1100,"entries":catalog(),"correlation_id":"b".repeat(32)}),
    )
    .await;
    send(&mut link, "grants", json!({"epoch":epoch,"entries":[{"coglet":coglet,"operations":["speech.start"],"resources":["audio.mic","audio.speaker"],"expires":100_000_000}],"correlation_id":"c".repeat(32)})).await;
    for seq in first_seq..first_seq + 2 {
        send(&mut link, "event", json!({"source":surface,"seq":seq,"logical_time":2000+seq,"kind":"speech.start","payload":{"utterance":correlation(seq),"position_ms":0},"correlation_id":correlation(seq)})).await;
    }
    link
}

pub(super) async fn step_at_least(env: &mut Env, minimum: u64) -> u64 {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let status = env.simple("hub.status").await;
            assert_eq!(status["result"]["state"], "running", "{status}");
            let step = status["result"]["step"].as_u64().unwrap();
            if step >= minimum {
                return step;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("real guest must commit after adapter event")
}
