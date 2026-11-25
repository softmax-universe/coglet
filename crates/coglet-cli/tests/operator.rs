//! CLI subprocess -> in-process reference router -> real encrypted iroh ->
//! durable production DeviceHandler. No success-returning fake deployment peer.
#![cfg(target_os = "linux")]
use coglet_hub::{
    Kernel, Registration,
    build::{Delegation, Profile, Target},
    kernel::saga::{Executor, Recipe},
    operator::{
        server,
        service::{GrantPolicy, Provisioning, Service},
        socket::{OperatorSocket, StateLock},
    },
};
use coglet_proto::{
    authority::{Authority, Grant, Scope},
    signing,
    transport::{ALPN, Role, Session},
};
use coglet_runtime::{DeviceConfig, DeviceHandler, TargetPin, digest};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output},
    sync::Arc,
    time::Duration,
};
const NOW: u64 = 1_800_000_000;
const METHODS: [&str; 6] = [
    "hub.git.push",
    "hub.install",
    "hub.start",
    "hub.stop",
    "hub.status",
    "hub.receipt",
];
const WAT: &str = "(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))";
fn git(root: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(root)
        .args(args)
        .env("GIT_AUTHOR_NAME", "fixture")
        .env("GIT_COMMITTER_NAME", "fixture")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .output()
        .unwrap();
    assert!(out.status.success(), "{out:?}");
    String::from_utf8(out.stdout).unwrap().trim().into()
}
async fn cli(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_coglet"))
            .env("TOKIO_WORKER_THREADS", "2")
            .args(args)
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}
fn result(out: Output) -> Value {
    assert!(
        out.status.success(),
        "stderr={} stdout={}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_reference_router_deploy_push_status_stop_and_idempotent_retry() {
    exercise(false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cli_reference_router_push_is_staging_not_install_or_start() {
    exercise(true).await;
}
async fn exercise(push_only: bool) {
    let root = tempfile::tempdir().unwrap();
    let router_root = root.path().join("router");
    std::fs::create_dir(&router_root).unwrap();
    std::fs::set_permissions(&router_root, std::fs::Permissions::from_mode(0o700)).unwrap();
    let key = SecretKey::generate();
    let device_key = SecretKey::generate();
    let coglet_key = SecretKey::generate();
    let target = format!("coglet:{}", coglet_key.public());
    let actor = format!("hub:{}", key.public());
    let router = Endpoint::builder(presets::Minimal)
        .secret_key(key.clone())
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(device_key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let device = endpoint.id();
    let statement = json!({"purpose":"delegation","hub":router.id().to_string(),"device":device.to_string(),"coglet":target,"generation":1,"expires":NOW+10000,"methods":METHODS});
    let sig = coglet_key.sign(&signing::preimage("delegation", &statement).unwrap());
    let proof = json!({"statement":statement,"signer":coglet_key.public().to_string(),"signature":hex::encode(sig.to_bytes())});
    let kernel = Kernel::open(&router_root, router.id(), Arc::new(|| Some(NOW))).unwrap();
    kernel.open_window(Duration::from_secs(60)).await.unwrap();
    let grants: Vec<_> = METHODS
        .iter()
        .map(|m| Grant {
            method: (*m).into(),
            target: Some(target.clone()),
            expires: NOW + 10000,
            generation: 1,
        })
        .collect();
    let invite = kernel
        .invite(device, Duration::from_secs(60), grants.clone())
        .await
        .unwrap();
    kernel
        .register(Registration {
            coglet: target.clone(),
            device: device.to_string(),
            grant_generation: 1,
            delegation: proof.clone(),
            events: vec![],
            actions: vec![],
        })
        .await
        .unwrap();
    let authority = Authority::open(root.path().join("device.db")).unwrap();
    authority.open_window(Duration::from_secs(60)).unwrap();
    let secret = authority
        .issue_invite(router.id(), Duration::from_secs(60), grants.clone())
        .unwrap();
    authority.enroll(router.id(), Some(&secret)).unwrap();
    for grant in &grants {
        authority
            .provision_scope(
                router.id(),
                &Scope {
                    actor: actor.clone(),
                    ticket: None,
                },
                grant,
            )
            .unwrap();
    }
    authority
        .accept_delegation(router.id(), device, &target, &proof, || Some(NOW))
        .unwrap();
    // Explicit real builder validator prerequisite, not a shell success stub.
    let validator = std::env::var_os("COGLET_TEST_VALIDATOR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(env!("CARGO_BIN_EXE_coglet"))
                .with_file_name("coglet-hub-build-validator")
        });
    assert!(
        validator.is_file(),
        "build coglet-hub-build-validator before this test"
    );
    let kill = root.path().join("kill.json");
    std::fs::write(&kill, b"{\"latched\":false}").unwrap();
    let handler=DeviceHandler::new(DeviceConfig{hub:router.id(),device,wal_root:root.path().join("wal"),quarantine:root.path().join("quarantine"),catalog:json!([]),grants:json!({"coglet":target,"operations":[],"resources":[]}),kill_switch:Some(kill),targets:BTreeMap::from([(target.clone(),TargetPin{grant_generation:1,delegation_generation:1,back_channel:None,fetch_snapshots:BTreeMap::new(),native_schedule:Some(json!({"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8388608,"stack_bytes":65536}))})]),adapter:None},authority,&validator,Arc::new(||Some(NOW))).unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let manifest=serde_json::to_vec(&json!({"version":"0.1","id":target,"subscriptions":[],"required_capabilities":[],"exclusive_outputs":[],"budgets":{"compute":10000,"compute_mode":"wamr-2.4.3-fast-cumulative-v1","memory_pages":2,"wall_time_us":100000,"max_events":16,"host_bytes":65536}})).unwrap();
    std::fs::write(source.join("guest.wat"), WAT).unwrap();
    std::fs::write(source.join("manifest.json"), &manifest).unwrap();
    git(&source, &["init"]);
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "fixture"]);
    let commit = git(&source, &["rev-parse", "HEAD"]);
    // SAFETY: geteuid has no pointers.
    let uid = unsafe { libc::geteuid() };
    let quarantine = root.path().join("build-quarantine");
    std::fs::create_dir(&quarantine).unwrap();
    let service = Arc::new(
        Service::new(
            kernel.clone(),
            uid,
            Provisioning {
                uid,
                actor,
                operations: vec![
                    "deploy".into(),
                    "push".into(),
                    "stop".into(),
                    "status".into(),
                ],
                max_window_ms: 0,
                grants: METHODS
                    .iter()
                    .map(|m| GrantPolicy {
                        peer: device.to_string(),
                        method: (*m).into(),
                        target: Some(target.clone()),
                        max_expires_unix_s: NOW + 10000,
                    })
                    .collect(),
            },
        )
        .unwrap()
        .with_executor(Executor {
            key,
            recipes: vec![Recipe {
                target: Target {
                    repository: source,
                    coglet: target.clone(),
                    device: device.to_string(),
                },
                profile: Profile::integer_wat(
                    "fixture",
                    digest(WAT.as_bytes()),
                    digest(&manifest),
                    digest(&wat::parse_str(WAT).unwrap()),
                ),
                delegation: Delegation {
                    id: "fixture".into(),
                    signed: proof,
                },
                generation: 1,
                validator,
                quarantine,
            }],
        }),
    );
    let outgoing = endpoint.connect(router.addr(), ALPN);
    let incoming = async { router.accept().await.unwrap().await.unwrap() };
    let (dc, hc) = tokio::join!(outgoing, incoming);
    let ds = Session::new(dc.unwrap(), device, Role::Device).unwrap();
    let hs = Session::new(hc, router.id(), Role::Hub).unwrap();
    let serving = ds.clone();
    let device_task = tokio::spawn(async move {
        let _ = serving.serve(Arc::new(handler)).await;
    });
    let serving = hs.clone();
    let k = kernel.clone();
    let hub_task = tokio::spawn(async move {
        let _ = serving.serve(Arc::new(k)).await;
    });
    let enrolled=ds.request(json!({"method":"hub.enroll","target":null,"correlation_id":"aa".repeat(16),"timeout_ms":10000,"params":{"invite":invite,"offer":{"versions":["0.1"]}}})).await.unwrap();
    assert_eq!(enrolled["type"], "reply");
    service.attach(hs.clone()).unwrap();
    let driver = tokio::spawn(service.clone().run());
    let lock = StateLock::acquire(&router_root).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let socket_task = tokio::spawn(server::serve(socket, service, 8, std::future::pending()));
    let args = |op: &str, id: &str| -> Vec<String> {
        let mut a = vec![
            op.into(),
            "--operator-socket".into(),
            router_root.join("operator.sock").to_str().unwrap().into(),
            "--operation-id".into(),
            id.into(),
            "--device".into(),
            device.to_string(),
            "--target".into(),
            target.clone(),
        ];
        if op == "deploy" || op == "push" {
            a.extend([
                "--source-commit".into(),
                commit.clone(),
                "--build-profile".into(),
                "fixture".into(),
                "--delegation-id".into(),
                "fixture".into(),
            ]);
        }
        a
    };
    if push_only {
        let push = result(cli(args("push", &"01".repeat(16))).await);
        assert_eq!(push["receipt"]["state"], "applied");
        assert_eq!(push["result"], Value::Null);
        assert_eq!(result(cli(args("push", &"01".repeat(16))).await), push);
        let status = result(cli(args("status", &"03".repeat(16))).await);
        assert_eq!(status["result"]["state"], "stopped", "{status}");
        assert!(status["result"]["live_commit"].is_null(), "{status}");
    } else {
        let deploy = result(cli(args("deploy", &"02".repeat(16))).await);
        assert_eq!(deploy["receipt"]["state"], "applied");
        assert_eq!(
            result(cli(args("deploy", &"02".repeat(16))).await),
            deploy,
            "same ID retains original result"
        );
        let running = result(cli(args("status", &"03".repeat(16))).await);
        assert_eq!(running["result"]["state"], "running");
        let conflict = cli(args("stop", &"02".repeat(16))).await;
        assert!(!conflict.status.success());
        let conflict: Value = serde_json::from_slice(&conflict.stdout).unwrap();
        assert_eq!(conflict["error"]["code"], "conflict");
        result(cli(args("stop", &"04".repeat(16))).await);
        assert_eq!(
            result(cli(args("status", &"05".repeat(16))).await)["result"]["state"],
            "stopped"
        );
        assert_eq!(
            result(cli(args("status", &"03".repeat(16))).await),
            running,
            "status retry is retained observation, not fresh sample"
        );
    }
    socket_task.abort();
    driver.abort();
    device_task.abort();
    hub_task.abort();
    let _ = socket_task.await;
    let _ = driver.await;
    let _ = device_task.await;
    let _ = hub_task.await;
    drop(ds);
    drop(hs);
    endpoint.close().await;
    router.close().await;
}
