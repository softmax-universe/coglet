//! Operator socket -> qualified builder -> streaming Git -> durable runtime.
//! The mock adapter owns a real iroh peer; DeviceHandler owns all effect receipts.
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
use coglet_mock::lifecycle::LifecyclePeer;
use coglet_proto::{
    authority::{Authority, Grant, Scope},
    signing,
    transport::{ALPN, OperationHandler, RequestContext},
};
use coglet_runtime::{DeviceConfig, DeviceHandler, TargetPin};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path, process::Command, sync::Arc, time::Duration};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
fn hash(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}
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
    assert!(out.status.success(), "{:?}", out);
    String::from_utf8(out.stdout).unwrap().trim().into()
}
fn sign(key: &SecretKey, statement: Value) -> Value {
    let sig = key.sign(&signing::preimage("delegation", &statement).unwrap());
    json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(sig.to_bytes())})
}
async fn call(root: &Path, request: &Value) -> Value {
    let mut stream = tokio::net::UnixStream::connect(root.join("operator.sock"))
        .await
        .unwrap();
    let bytes = serde_json::to_vec(request).unwrap();
    stream.write_u32(bytes.len() as u32).await.unwrap();
    stream.write_all(&bytes).await.unwrap();
    let len = stream.read_u32().await.unwrap();
    let mut bytes = vec![0; len as usize];
    stream.read_exact(&mut bytes).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}
struct CrashBoundary {
    handler: DeviceHandler,
    method: Option<&'static str>,
    armed: std::sync::atomic::AtomicBool,
    busy: std::sync::atomic::AtomicBool,
    not_ready: std::sync::atomic::AtomicBool,
    reached: tokio::sync::Notify,
    release: tokio::sync::Notify,
    requests: std::sync::Mutex<Vec<Value>>,
}
impl CrashBoundary {
    async fn after(&self, request: &Value) {
        let selected = self.method == request["method"].as_str()
            || (self.method == Some("running")
                && request["method"] == "hub.status"
                && self
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r["method"] == "hub.start"));
        if selected && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.reached.notify_one();
            self.release.notified().await;
        }
    }
    fn record(&self, request: &Value) {
        self.requests.lock().unwrap().push(request.clone());
    }
}
impl OperationHandler for CrashBoundary {
    fn handle<'a>(
        &'a self,
        c: RequestContext,
        r: Value,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            self.record(&r);
            // Deterministic independent-stream negotiation race: a real peer
            // can refuse before selection without ever reaching its handler.
            if r["method"] == "hub.status"
                && self
                    .not_ready
                    .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return json!({"type":"error","correlation_id":r["correlation_id"],"error":{"code":"unauthenticated","message":"protocol not selected","retryable":false,"details":{}}});
            }
            // Fault injection is denial before admission, never a success receipt.
            if r["method"] == "hub.install"
                && self.busy.swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                return json!({"type":"error","correlation_id":r["correlation_id"],"error":{"code":"resource_exhausted","message":"fixture busy","retryable":true,"details":{}}});
            }
            let result = self.handler.handle(c, r.clone()).await;
            self.after(&r).await;
            result
        })
    }
    fn handle_git<'a>(
        &'a self,
        c: RequestContext,
        r: Value,
        stream: coglet_proto::transport::GitStream<'a>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Value, coglet_proto::transport::TransportError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            self.record(&r);
            let result = self.handler.handle_git(c, r.clone(), stream).await;
            self.after(&r).await;
            result
        })
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_real_iroh_deploy_running_stop_status() {
    exercise(None, false).await;
}
/// Explicit binary acceptance: build cogletd with native-engine first and supply
/// COGLET_TEST_COGLETD. Never substitute the in-process handler for this proof.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires an explicitly built native cogletd; run with COGLET_TEST_COGLETD"]
async fn operator_cogletd_deploy_running_stop_status() {
    exercise(None, true).await;
}
struct Daemon(std::process::Child);
impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn operator_saga_crash_each_stage() {
    // Status is the post-build/pre-push barrier: the artifact and every child
    // intent are durable, but no mutation has been sent yet.
    for method in [
        "hub.status",
        "hub.git.push",
        "hub.install",
        "hub.start",
        "running",
    ] {
        exercise(Some(method), false).await;
    }
}
async fn exercise(crash: Option<&'static str>, daemon: bool) {
    let root = tempfile::tempdir().unwrap();
    let router_root = root.path().join("router");
    std::fs::create_dir(&router_root).unwrap();
    use std::os::unix::fs::PermissionsExt;
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
    let endpoint = LifecyclePeer::bind(device_key.clone()).await.unwrap();
    let device = endpoint.id();
    let proof = sign(
        &coglet_key,
        json!({"purpose":"delegation","hub":router.id().to_string(),"device":device.to_string(),"coglet":target,"generation":1,"expires":NOW+10000,"methods":METHODS}),
    );
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
    let real_validator = std::path::PathBuf::from(env!("CARGO_BIN_EXE_coglet-hub-build-validator"));
    let building = root.path().join("building");
    let resume_build = root.path().join("resume-build");
    let validator = if crash.is_none() {
        let wrapper = root.path().join("validator");
        std::fs::write(&wrapper, format!("#!/bin/sh\ntouch '{}'\nwhile [ ! -f '{}' ]; do sleep 0.01; done\nexec '{}' \"$@\"\n", building.display(), resume_build.display(), real_validator.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        wrapper
    } else {
        real_validator
    };
    let kill = root.path().join("kill.json");
    std::fs::write(&kill, b"{\"latched\":false}").unwrap();
    let handler=DeviceHandler::new(DeviceConfig {hub:router.id(),device,wal_root:root.path().join("wal"),quarantine:root.path().join("device-quarantine"),catalog:json!([]),grants:json!({"coglet":target,"operations":[],"resources":[]}),kill_switch:Some(kill),
  targets:BTreeMap::from([(target.clone(),TargetPin{grant_generation:1,delegation_generation:1,native_schedule:Some(json!({"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8388608,"stack_bytes":65536})),back_channel:Default::default(),fetch_snapshots:Default::default()})]),adapter:None},authority.clone(),&validator,Arc::new(||Some(NOW))).unwrap();
    let boundary = Arc::new(CrashBoundary {
        handler: handler.clone(),
        method: crash,
        armed: std::sync::atomic::AtomicBool::new(true),
        busy: std::sync::atomic::AtomicBool::new(crash.is_none()),
        not_ready: std::sync::atomic::AtomicBool::new(crash.is_none()),
        reached: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        requests: std::sync::Mutex::new(vec![]),
    });
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let manifest = json!({"version":"1.0","id":target,"subscriptions":[],"required_capabilities":[],"exclusive_outputs":[],"budgets":{"compute":10000,"compute_mode":"wamr-2.4.3-fast-cumulative-v1","memory_pages":2,"wall_time_us":100000,"max_events":16,"host_bytes":65536}});
    let manifest = serde_json::to_vec(&manifest).unwrap();
    std::fs::write(source.join("guest.wat"), WAT).unwrap();
    std::fs::write(source.join("manifest.json"), &manifest).unwrap();
    git(&source, &["init"]);
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "fixture"]);
    let commit = git(&source, &["rev-parse", "HEAD"]);
    let quarantine = root.path().join("build-quarantine");
    std::fs::create_dir(&quarantine).unwrap();
    // SAFETY: geteuid has no arguments or mutable memory.
    let uid = unsafe { libc::geteuid() };
    let p = Provisioning {
        uid,
        actor,
        operations: vec![
            "grant.revoke".into(),
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
    };
    let executor = Executor {
        key,
        recipes: vec![Recipe {
            target: Target {
                repository: source,
                coglet: target.clone(),
                device: device.to_string(),
            },
            profile: Profile::integer_wat(
                "fixture",
                hash(WAT.as_bytes()),
                hash(&manifest),
                hash(&wat::parse_str(WAT).unwrap()),
            ),
            delegation: Delegation {
                id: "fixture".into(),
                signed: proof,
            },
            generation: 1,
            validator,
            quarantine,
        }],
    };
    let service = Arc::new(
        Service::new(kernel.clone(), uid, p.clone())
            .unwrap()
            .with_executor(executor.clone()),
    );
    let mut child = None;
    let mut daemon_token = None;
    let (fixture, mut session, mut server) = if daemon {
        // Unlike LifecyclePeer's explicit attach, use the SAME connection handler
        // as production startup, and let the actual daemon enroll itself.
        endpoint.close().await;
        let identity = root.path().join("device.key");
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&identity)
            .unwrap()
            .write_all(&device_key.to_bytes())
            .unwrap();
        let config = root.path().join("device.json");
        std::fs::write(&config, serde_json::to_vec(&json!({
            "hub":router.id().to_string(), "device":device.to_string(),
            "identity_path":identity,"authority_path":root.path().join("device.db"),
            "validator_path":executor.recipes[0].validator,
            "wal_root":root.path().join("wal"),"quarantine":root.path().join("device-quarantine"),
            "catalog":[],"grants":{"coglet":target,"operations":[],"resources":[]},
            "kill_switch":root.path().join("kill.json"),"invite":invite,
            "hub_addr":router.addr().ip_addrs().next().unwrap(),
            "targets":{(target.clone()):{"grant_generation":1,"delegation_generation":1,
            "native_schedule":{"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8388608,"stack_bytes":65536}}}
        })).unwrap()).unwrap();
        let log = root.path().join("daemon.log");
        child = Some(Daemon(
            Command::new(
                std::env::var_os("COGLET_TEST_COGLETD").expect("explicit native cogletd path"),
            )
            .args(["--router-service"])
            .arg(&config)
            .env("TOKIO_WORKER_THREADS", "2")
            .stdout(std::process::Stdio::null())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap(),
        ));
        let connection = tokio::time::timeout(Duration::from_secs(20), async {
            router.accept().await.unwrap().await.unwrap()
        })
        .await
        .expect("actual cogletd connects");
        let session = coglet_proto::transport::Session::new(
            connection,
            router.id(),
            coglet_proto::transport::Role::Hub,
        )
        .unwrap();
        let token = kernel.transport_connected(device).await.unwrap();
        daemon_token = Some(token);
        let handler = service.connection(session.clone(), token);
        let incoming = session.clone();
        let server = tokio::spawn(async move {
            let _ = incoming.serve(handler).await;
        });
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let db = rusqlite::Connection::open(router_root.join("kernel.db")).unwrap();
                let enrolled: bool = db
                    .query_row(
                        "SELECT EXISTS(SELECT 1 FROM proto_peers WHERE peer=?1)",
                        [device.to_string()],
                        |r| r.get(0),
                    )
                    .unwrap();
                if enrolled {
                    break;
                }
                if let Some(status) = child.as_mut().unwrap().0.try_wait().unwrap() {
                    panic!(
                        "cogletd exited {status}: {}",
                        std::fs::read_to_string(&log).unwrap()
                    );
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real cogletd enrollment committed");
        (None, session, server)
    } else {
        let (fixture, device_session, session) =
            LifecyclePeer::connect(endpoint, &router, boundary.clone())
                .await
                .unwrap();
        let kernel_serve = kernel.clone();
        let incoming = session.clone();
        let server = tokio::spawn(async move {
            let _ = incoming.serve(Arc::new(kernel_serve)).await;
        });
        let enrolled=device_session.request(json!({"method":"hub.enroll","target":null,"correlation_id":"aa".repeat(16),"timeout_ms":10000,"params":{"invite":invite,"offer":{"versions":["0.1"]}}})).await.unwrap();
        assert_eq!(enrolled["type"], "reply", "{enrolled}");
        service.attach(session.clone()).unwrap();
        (Some(fixture), session, server)
    };
    let mut driver = tokio::spawn(service.clone().run());
    let lock = StateLock::acquire(&router_root).unwrap();
    let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
    let mut socket_task = tokio::spawn(server::serve(
        socket,
        service.clone(),
        128,
        std::future::pending(),
    ));
    let deploy = json!({"version":"local-1","operation_id":"01".repeat(16),"op":"deploy","timeout_ms":30000,"params":{"device":device.to_string(),"target":target,"source_commit":commit,"build_profile":"fixture","delegation_id":"fixture"}});
    if crash.is_none() {
        let mut first = deploy.clone();
        first["timeout_ms"] = json!(1);
        assert_eq!(
            call(&router_root, &first).await["receipt"]["state"],
            "queued"
        );
        tokio::time::timeout(Duration::from_secs(20), async {
            while !building.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("real builder validation reached");
        tokio::time::timeout(Duration::from_secs(2), kernel.status())
            .await
            .expect("builder must not hold global State")
            .unwrap();
        std::fs::write(&resume_build, b"resume").unwrap();
    }
    if crash.is_some() {
        let mut first = deploy.clone();
        first["timeout_ms"] = json!(1);
        assert_eq!(
            call(&router_root, &first).await["receipt"]["state"],
            "queued"
        );
        tokio::time::timeout(Duration::from_secs(30), boundary.reached.notified())
            .await
            .expect("real runtime boundary reached");
        let db = rusqlite::Connection::open(router_root.join("kernel.db")).unwrap();
        let before: String = db
            .query_row(
                "SELECT progress FROM operator_sagas WHERE id=?1",
                ["01".repeat(16)],
                |r| r.get(0),
            )
            .unwrap();
        let before: Value = serde_json::from_str(&before).unwrap();
        // Admission remains independent of the blocked operation executor.
        let concurrent = json!({"version":"local-1","operation_id":"08".repeat(16),"op":"status","timeout_ms":1,"params":{"device":device.to_string(),"target":target}});
        let admitted =
            tokio::time::timeout(Duration::from_secs(2), call(&router_root, &concurrent))
                .await
                .expect("blocked executor must not block durable admission");
        assert_eq!(admitted["receipt"]["state"], "queued");
        driver.abort();
        let _ = driver.await;
        socket_task.abort();
        let _ = socket_task.await;
        boundary.release.notify_one();
        let recovered = Kernel::open(&router_root, router.id(), Arc::new(|| Some(NOW))).unwrap();
        let service = Arc::new(
            Service::new(recovered, uid, p.clone())
                .unwrap()
                .with_executor(executor.clone()),
        );
        service.attach(session.clone()).unwrap();
        driver = tokio::spawn(service.clone().run());
        let socket = OperatorSocket::bind_locked(&lock).await.unwrap();
        socket_task = tokio::spawn(server::serve(socket, service, 128, std::future::pending()));
        let response = call(&router_root, &deploy).await;
        assert_eq!(response["receipt"]["state"], "applied", "{response}");
        let after: String = db
            .query_row(
                "SELECT progress FROM operator_sagas WHERE id=?1",
                ["01".repeat(16)],
                |r| r.get(0),
            )
            .unwrap();
        let after: Value = serde_json::from_str(&after).unwrap();
        assert_eq!(
            before["children"], after["children"],
            "restart must retain exact child intents, IDs and actor"
        );
        let requests = boundary.requests.lock().unwrap();
        assert!(
            requests.iter().any(|r| r["method"] == "hub.receipt"),
            "restart must actually poll peer receipt"
        );
        for method in ["hub.git.push", "hub.install", "hub.start"] {
            let ids: std::collections::BTreeSet<_> = requests
                .iter()
                .filter(|r| r["method"] == method)
                .map(|r| r["correlation_id"].as_str().unwrap())
                .collect();
            assert_eq!(ids.len(), 1, "original mutation ID only: {method}");
        }
    }
    let response = call(&router_root, &deploy).await;
    assert_eq!(response["receipt"]["state"], "applied", "{response}");
    let progress: Value = {
        let db = rusqlite::Connection::open(router_root.join("kernel.db")).unwrap();
        let text: String = db
            .query_row(
                "SELECT progress FROM operator_sagas WHERE id=?1",
                ["01".repeat(16)],
                |r| r.get(0),
            )
            .unwrap();
        serde_json::from_str(&text).unwrap()
    };
    assert_eq!(
        progress["outcomes"].as_array().unwrap().last().unwrap()["observation"]["state"],
        "running",
        "deploy itself must observe running"
    );
    let repeated = call(&router_root, &deploy).await;
    assert_eq!(response, repeated);
    let status = json!({"version":"local-1","operation_id":"02".repeat(16),"op":"status","timeout_ms":30000,"params":{"device":device.to_string(),"target":target}});
    let running = call(&router_root, &status).await;
    assert_eq!(running["receipt"]["state"], "applied", "{running}");
    assert_eq!(running["result"]["state"], "running", "{running}");
    let stop = json!({"version":"local-1","operation_id":"03".repeat(16),"op":"stop","timeout_ms":30000,"params":{"device":device.to_string(),"target":target}});
    let stopped = call(&router_root, &stop).await;
    assert_eq!(stopped["receipt"]["state"], "applied", "{stopped}");
    if daemon {
        // Actual process death, not an in-process handler reconstruction. Resume
        // enrollment without an invite and preserve both device WAL and local IDs.
        drop(child.take());
        let token = daemon_token.unwrap();
        service.detach(device, token);
        kernel.transport_disconnected(device, token).await.unwrap();
        server.abort();
        let _ = (&mut server).await;
        let config_path = root.path().join("device.json");
        let mut config: Value =
            serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
        config["invite"] = Value::Null;
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        child = Some(Daemon(
            Command::new(std::env::var_os("COGLET_TEST_COGLETD").unwrap())
                .arg("--router-service")
                .arg(&config_path)
                .env("TOKIO_WORKER_THREADS", "2")
                .stdout(std::process::Stdio::null())
                .stderr(std::fs::File::create(root.path().join("daemon-restart.log")).unwrap())
                .spawn()
                .unwrap(),
        ));
        let connection = tokio::time::timeout(Duration::from_secs(20), async {
            router.accept().await.unwrap().await.unwrap()
        })
        .await
        .expect("restarted cogletd connects");
        session = coglet_proto::transport::Session::new(
            connection,
            router.id(),
            coglet_proto::transport::Role::Hub,
        )
        .unwrap();
        let token = kernel.transport_connected(device).await.unwrap();
        let connection_handler = service.connection(session.clone(), token);
        let incoming = session.clone();
        server = tokio::spawn(async move {
            let _ = incoming.serve(connection_handler).await;
        });
        let mut fresh = status.clone();
        fresh["operation_id"] = json!("20".repeat(16));
        let observation = call(&router_root, &fresh).await;
        assert_eq!(observation["receipt"]["state"], "applied", "{observation}");
        assert_eq!(observation["result"]["state"], "stopped", "{observation}");
        assert_eq!(
            call(&router_root, &deploy).await,
            response,
            "restart must not redeploy a retained operation"
        );
        assert_eq!(
            call(&router_root, &stop).await,
            stopped,
            "restart retains local stop receipt"
        );
    }
    assert_eq!(
        call(&router_root, &status).await,
        running,
        "status retry must retain original running observation"
    );
    if crash.is_none() {
        let db = rusqlite::Connection::open(router_root.join("kernel.db")).unwrap();
        let progress: String = db
            .query_row(
                "SELECT progress FROM operator_sagas WHERE id=?1",
                ["01".repeat(16)],
                |r| r.get(0),
            )
            .unwrap();
        let progress: Value = serde_json::from_str(&progress).unwrap();
        let original = &progress["children"][2]; // install after preflight + push
        let other = format!("hub:{}", SecretKey::generate().public());
        for grant in &grants {
            authority
                .provision_scope(
                    router.id(),
                    &Scope {
                        actor: other.clone(),
                        ticket: None,
                    },
                    grant,
                )
                .unwrap();
        }
        let mut receipt = json!({"method":"hub.receipt","target":target,"timeout_ms":10000,"correlation_id":"41".repeat(16),"context":{"actor":p.actor},"params":{"operation_id":original["correlation_id"]}});
        let exact = session.request(receipt.clone()).await.unwrap();
        assert_eq!(exact["result"]["state"], "applied", "{exact}");
        receipt["context"]["actor"] = json!(other);
        receipt["correlation_id"] = json!("42".repeat(16));
        let hidden = session.request(receipt).await.unwrap();
        assert_eq!(hidden["error"]["code"], "not_found", "{hidden}");
        let mut changed = deploy.clone();
        changed["params"]["source_commit"] = json!("ff".repeat(20));
        assert_eq!(
            call(&router_root, &changed).await["error"]["code"],
            "conflict"
        );
        // The profile still admits the same exact source/module pins, but a different
        // source commit produces a distinct signed publication. Existing install must
        // conflict rather than silently replacing code or resetting data.
        let source = &executor.recipes[0].target.repository;
        git(
            source,
            &[
                "commit",
                "--allow-empty",
                "-m",
                "different pinned source identity",
            ],
        );
        let mut incompatible = deploy.clone();
        incompatible["operation_id"] = json!("04".repeat(16));
        incompatible["params"]["source_commit"] = json!(git(source, &["rev-parse", "HEAD"]));
        let conflict = call(&router_root, &incompatible).await;
        assert_eq!(
            conflict["receipt"]["error"]["code"], "conflict",
            "{conflict}"
        );
        assert_eq!(call(&router_root, &incompatible).await, conflict);
        let revoke = json!({"version":"local-1","operation_id":"05".repeat(16),"op":"grant.revoke","timeout_ms":30000,"params":{"peer":device.to_string(),"method":"hub.start","target":target,"generation":2}});
        let revoked = call(&router_root, &revoke).await;
        assert_eq!(revoked["receipt"]["state"], "applied", "{revoked}");
        let mut missing = deploy.clone();
        missing["operation_id"] = json!("06".repeat(16));
        let denied = call(&router_root, &missing).await;
        assert_eq!(
            denied["receipt"]["error"]["code"], "permission_denied",
            "{denied}"
        );
        assert_eq!(call(&router_root, &missing).await, denied);
        kernel.revoke_peer(device).await.unwrap();
        missing["operation_id"] = json!("07".repeat(16));
        let unenrolled = call(&router_root, &missing).await;
        assert_eq!(
            unenrolled["receipt"]["error"]["code"], "unauthenticated",
            "{unenrolled}"
        );
    }
    driver.abort();
    let _ = driver.await;
    socket_task.abort();
    let _ = socket_task.await;
    server.abort();
    let _ = server.await;
    if let Some(fixture) = fixture {
        fixture.close().await;
    }
    drop(child);
    router.close().await;
}
