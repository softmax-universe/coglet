use coglet_hub::Kernel;
use coglet_proto::transport::{ALPN, Role, Session};
use iroh::{Endpoint, endpoint::presets};
use serde_json::Value;
use std::{path::Path, process::Command, sync::Arc, time::Duration};

async fn command(args: Vec<String>) -> std::process::Output {
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_coglet"))
            .args(args)
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}
fn s(p: &Path) -> String {
    p.to_str().unwrap().into()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cli_enroll_reference_hub_and_recover_without_reusing_invite() {
    tokio::time::timeout(Duration::from_secs(30), async {
        let dir = tempfile::tempdir().unwrap();
        let key = dir.path().join("device.key");
        let id = command(vec!["identity".into(), s(&key)]).await;
        assert!(id.status.success());
        let id: Value = serde_json::from_slice(&id.stdout).unwrap();
        let device = id["device"].as_str().unwrap().parse().unwrap();
        let endpoint = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind()
            .await
            .unwrap();
        let kernel = Kernel::open(dir.path(), endpoint.id(), Arc::new(|| Some(1))).unwrap();
        kernel.open_window(Duration::from_secs(60)).await.unwrap();
        let invite = kernel
            .invite(device, Duration::from_secs(60), vec![])
            .await
            .unwrap();
        let secret_file = dir.path().join("invite");
        coglet_cli::private_file(&secret_file, invite.as_bytes()).unwrap();
        let address = *endpoint.addr().ip_addrs().next().unwrap();
        let base = vec![
            "enroll".into(),
            "--identity".into(),
            s(&key),
            "--hub".into(),
            endpoint.id().to_string(),
            "--address".into(),
            address.to_string(),
        ];
        let server = tokio::spawn({
            let endpoint = endpoint.clone();
            let kernel = kernel.clone();
            async move {
                let mut tasks = tokio::task::JoinSet::new();
                while let Some(incoming) = endpoint.accept().await {
                    let connection = incoming.await.unwrap();
                    let session = Session::new(connection, endpoint.id(), Role::Hub).unwrap();
                    let kernel = Arc::new(kernel.clone());
                    tasks.spawn(async move {
                        let _ = session.serve(kernel).await;
                    });
                }
            }
        });
        let mut first = base.clone();
        first.extend(["--invite-file".into(), s(&secret_file)]);
        let out = command(first.clone()).await;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let reply: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(reply["device"], device.to_string());
        assert_eq!(reply["hub"], endpoint.id().to_string());
        assert!(!String::from_utf8_lossy(&out.stdout).contains(&invite));
        let recovered = command(base).await;
        assert!(
            recovered.status.success(),
            "{}",
            String::from_utf8_lossy(&recovered.stderr)
        );
        let denied = command(first).await;
        assert!(!denied.status.success(), "consumed invitation was reused");
        assert!(!String::from_utf8_lossy(&denied.stderr).contains(&invite));
        let invalid_secret = "z".repeat(64);
        std::fs::write(&secret_file, &invalid_secret).unwrap();
        let invalid = command(vec![
            "enroll".into(),
            "--identity".into(),
            s(&key),
            "--hub".into(),
            endpoint.id().to_string(),
            "--address".into(),
            address.to_string(),
            "--invite-file".into(),
            s(&secret_file),
        ])
        .await;
        assert!(!invalid.status.success());
        assert!(String::from_utf8_lossy(&invalid.stderr).contains("redacted"));
        assert!(!String::from_utf8_lossy(&invalid.stderr).contains(&invalid_secret));
        let audit = kernel.audit().await.unwrap();
        assert!(!serde_json::to_string(&audit).unwrap().contains(&invite));
        server.abort();
        endpoint.close().await;
    })
    .await
    .expect("reference-hub enrollment test timeout");
}
