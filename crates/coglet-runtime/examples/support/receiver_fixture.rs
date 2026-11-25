//! Isolated fixture library for the frozen receiver stdio peer.
use coglet_proto::{
    authority::{Authority, Grant, Scope},
    signing,
    transport::{ALPN, OperationHandler, RequestContext, Role, Session},
    wire,
};
use coglet_runtime::{DeviceConfig, DeviceHandler, TargetPin, digest};
use iroh::{Endpoint, SecretKey, endpoint::Connection, endpoint::presets};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    io::Write,
    path::{Path, PathBuf},
    pin::Pin,
    process::Command,
    sync::Arc,
    time::Duration,
};
use tokio::{io::AsyncWriteExt, task::JoinHandle};

pub const T0: u64 = 1_800_000_000;
const GRANT_GEN: u64 = 0;
const DELEGATION_GEN: u64 = 1;
const DELEGATION_EXPIRES: u64 = T0 + 1_000;
const TICKET_66: &str = "66666666666666666666666666666666";
const TICKET_77: &str = "77777777777777777777777777777777";
const OTHER_OP: &str = "88888888888888888888888888888888";
const WASM_WAT: &str = r#"(module (memory (export "memory") 1 2)
    (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))"#;
const LOOP_WAT: &str = r#"(module (memory (export "memory") 1 2)
    (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
    (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
      local.get 2 i64.const 0 i64.gt_u
      if (loop $busy
        i32.const 0 i32.const 0 i32.const 65536 memory.fill
        br $busy) end i32.const 0))"#;

pub const CASES: &[&str] = &[
    "action-expired",
    "cross-owner-receipt",
    "unknown-receipt",
    "debug-inbox-protected",
    "debug-traversal",
    "read-grant-is-not-write",
    "write-stale-snapshot",
    "read-actor-isolation",
    "read-ended-ticket",
    "read-missing-ticket",
    "read-scoped-allowed",
    "pause-single-step",
    "single-step-requires-pause",
    "status-actor-isolation",
    "status-ended-ticket",
    "status-missing-ticket",
    "status-scoped-allowed",
    "fetch-actor-isolation",
    "fetch-ended-ticket",
    "fetch-missing-ticket",
    "fetch-scoped-allowed",
    "inbox-boundary-queued",
    "inbox-conflicting-id",
    "inbox-durable-dedup",
    "queue-backpressure",
];

type Error = Box<dyn std::error::Error + Send + Sync>;
type Result<T> = std::result::Result<T, Error>;

struct Enroll;
impl OperationHandler for Enroll {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(async move {
            json!({"type":"reply","correlation_id":request["correlation_id"],
                "result":{"selection":{"version":"0.1"},
                    "device":context.peer.to_string(),"hub":context.local.to_string()}})
        })
    }
}

pub struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    coglet: String,
    hub_actor: String,
    operator: String,
    other_actor: String,
    session: Session,
    connection: Connection,
    handler: Option<DeviceHandler>,
    config: DeviceConfig,
    device_session: Session,
    baseline: Value,
    baseline_records: Vec<Value>,
    observations: Vec<(Value, Value)>,
    prepared: Vec<(Value, Value)>,
    quarantine_baseline: Vec<String>,
    serve: JoinHandle<()>,
    _hold: Option<JoinHandle<Value>>,
    held_since: Option<tokio::time::Instant>,
    _device: Endpoint,
    _hub: Endpoint,
    _hub_serve: JoinHandle<()>,
}

pub fn supported(case: &str) -> bool {
    CASES.contains(&case)
}

fn sign(key: &SecretKey, purpose: &str, statement: Value) -> Value {
    let signature = key.sign(&signing::preimage(purpose, &statement).unwrap());
    json!({"signature":hex::encode(signature.to_bytes()),"signer":key.public().to_string(),"statement":statement})
}

fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("/usr/bin/git")
        .current_dir(dir)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

fn validator_path() -> PathBuf {
    // target/<profile>/examples/runtime_receiver_peer -> target/<profile>/validator
    std::env::current_exe()
        .ok()
        .and_then(|exe| Some(exe.parent()?.parent()?.join("coglet-runtime-git-validator")))
        .unwrap_or_else(|| PathBuf::from("target/debug/coglet-runtime-git-validator"))
}

fn counter_wasm() -> Result<Vec<u8>> {
    if let Ok(path) = std::env::var("COGLET_COUNTER_WASM") {
        return Ok(std::fs::read(path)?);
    }
    for path in [
        "target/guests/wasm32-wasip1/release/coglet_counter.wasm",
        "coglets/counter/target/wasm32-wasip1/release/coglet_counter.wasm",
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            return Ok(bytes);
        }
    }
    Err("qualified counter wasm missing; build R3 coglet-counter first".into())
}

fn guest_wasm(case: &str) -> Result<(Vec<u8>, u64)> {
    if case == "pause-single-step" {
        return Ok((counter_wasm()?, 10_000_000));
    }
    if matches!(case, "inbox-boundary-queued" | "queue-backpressure") {
        return Ok((wat::parse_str(LOOP_WAT)?, 2_000_000_000));
    }
    Ok((wat::parse_str(WASM_WAT)?, 10_000))
}

fn lifecycle_methods() -> &'static [&'static str] {
    &[
        "hub.git.push",
        "hub.install",
        "hub.start",
        "hub.stop",
        "hub.status",
        "hub.receipt",
        "debug.read",
        "debug.write",
        "hub.inbox",
        "hub.cancel",
        "debug.pause",
        "debug.step",
        "hub.actions",
        "hub.git.fetch",
    ]
}

fn hub_methods(case: &str) -> Vec<&'static str> {
    lifecycle_methods()
        .iter()
        .copied()
        .filter(|method| *method != "debug.write" || case != "read-grant-is-not-write")
        .collect()
}

fn observation_method(case: &str) -> Option<&'static str> {
    if case.starts_with("read-") {
        Some("debug.read")
    } else if case.starts_with("status-") {
        Some("hub.status")
    } else if case.starts_with("fetch-") {
        Some("hub.git.fetch")
    } else {
        None
    }
}

fn write_paths(case: &str) -> Vec<&'static str> {
    if matches!(case, "debug-inbox-protected" | "debug-traversal") {
        vec!["/data/notes/"]
    } else {
        vec!["/data/"]
    }
}

fn fetch_logs_snapshot() -> Result<Value> {
    // Crate-owned harness input (not a spec fixture): compiled in so the peer
    // cannot lose it to an unrelated spec change. Guarded by
    // tests/receiver_harness_inputs.rs.
    let last: Value = serde_json::from_str(include_str!("receiver-fetch-logs-snapshot.json"))?;
    if last["ref"] != "refs/heads/logs" {
        return Err("logs snapshot missing".into());
    }
    Ok(json!({
        "update": last["update"],
        "bundle_hex": last["bundle_hex"],
        "sha256": last["sha256"],
    }))
}

fn bundle(
    root: &Path,
    hub_key: &SecretKey,
    coglet: &str,
    wasm: &[u8],
    compute: u64,
    case: &str,
) -> Result<(Vec<u8>, Value)> {
    let work = root.join("git-1");
    std::fs::create_dir_all(&work)?;
    git(&work, &["init", "--initial-branch=hub"]);
    let pages = if case == "pause-single-step" { 256 } else { 2 };
    // Held guests loop until this wall bound; the post-run oracle must wait for
    // that real guest to finish, and the harness bounds the last reply.
    let wall_time_us: u64 = match case {
        "inbox-boundary-queued" => 600_000,
        // 256 durable queued inboxes are filled while this real guest runs.
        "queue-backpressure" => 8_000_000,
        _ => 2_000_000,
    };
    let manifest = json!({"version":"0.1","id":coglet,"subscriptions":[],
        "required_capabilities":[],"exclusive_outputs":[],
        "budgets":{"compute":compute,"compute_mode":coglet_engine_wamr::COMPUTE_MODE,
            "memory_pages":pages,"wall_time_us":wall_time_us,"max_events":16,"host_bytes":1_048_576}});
    std::fs::write(work.join("manifest.json"), serde_json::to_vec(&manifest)?)?;
    std::fs::create_dir_all(work.join("bin"))?;
    std::fs::write(work.join("bin/step.wasm"), wasm)?;
    let record = coglet_types::HubBuildRecord {
        version: "0.1".into(),
        source_commit: "ab".repeat(20),
        recipe_sha256: "11".repeat(32),
        policy_sha256: "22".repeat(32),
        transform_sha256: "33".repeat(32),
        original_sha256: hex::encode(Sha256::digest(wasm)),
        rewritten_sha256: hex::encode(Sha256::digest(wasm)),
    }
    .to_cbor()?;
    std::fs::create_dir_all(work.join("build"))?;
    std::fs::write(work.join("build/provenance.cbor"), record)?;
    git(&work, &["add", "."]);
    git(&work, &["commit", "-m", "code"]);
    let tip = String::from_utf8(git(&work, &["rev-parse", "HEAD"]))?
        .trim()
        .to_owned();
    let archive = git(&work, &["archive", "--format=tar", "HEAD"]);
    git(
        &work,
        &["bundle", "create", "transfer.bundle", "refs/heads/hub"],
    );
    let bytes = std::fs::read(work.join("transfer.bundle"))?;
    let signed = sign(
        hub_key,
        "hub-ref",
        json!({"purpose":"hub-ref","hub":hub_key.public().to_string(),"coglet":coglet,
            "ref":"refs/heads/hub","old":null,"new":tip,
            "archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":1}),
    );
    Ok((bytes, signed))
}

fn request(
    coglet: &str,
    method: &str,
    id: &str,
    params: Value,
    actor: &str,
    ticket: Option<&str>,
) -> Value {
    let mut context = json!({"actor":actor});
    if let Some(ticket) = ticket {
        context["ticket"] = json!(ticket);
    }
    json!({"method":method,"target":coglet,"correlation_id":id,"timeout_ms":30_000,
        "params":params,"context":context})
}

fn grant_for(
    authority: &Authority,
    hub: iroh::EndpointId,
    grants: &[Grant],
    methods: &[&str],
    actor: &str,
    ticket: Option<&str>,
) -> Result<()> {
    let scope = Scope {
        actor: actor.into(),
        ticket: ticket.map(str::to_owned),
    };
    for grant in grants
        .iter()
        .filter(|g| methods.contains(&g.method.as_str()))
    {
        authority.provision_scope(hub, &scope, grant)?;
    }
    Ok(())
}

impl Fixture {
    pub async fn boot(case: &str) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_owned();
        let coglet_key = SecretKey::from_bytes(&[1; 32]);
        let hub_key = SecretKey::from_bytes(&[2; 32]);
        let device_key = SecretKey::from_bytes(&[3; 32]);
        let coglet = format!("coglet:{}", coglet_key.public());
        let hub_actor = format!("hub:{}", hub_key.public());
        let operator = format!("hub:{}", SecretKey::from_bytes(&[9; 32]).public());
        let other_actor = format!("hub:{}", SecretKey::from_bytes(&[4; 32]).public());
        let hub = Endpoint::builder(presets::Minimal)
            .secret_key(hub_key.clone())
            .alpns(vec![ALPN.to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
            .bind()
            .await?;
        let device = Endpoint::builder(presets::Minimal)
            .secret_key(device_key.clone())
            .bind()
            .await?;
        let authority_path = root.join("authority.sqlite");
        let delegation = {
            let authority = Authority::open(&authority_path)?;
            authority.open_window(Duration::from_secs(30))?;
            let grants: Vec<Grant> = lifecycle_methods()
                .iter()
                .map(|method| Grant {
                    method: (*method).into(),
                    target: Some(coglet.clone()),
                    expires: T0 + 100_000,
                    generation: GRANT_GEN,
                })
                .collect();
            let invite =
                authority.issue_invite(hub.id(), Duration::from_secs(30), grants.clone())?;
            authority.enroll(hub.id(), Some(&invite))?;
            grant_for(
                &authority,
                hub.id(),
                &grants,
                &hub_methods(case),
                &hub_actor,
                None,
            )?;
            grant_for(
                &authority,
                hub.id(),
                &grants,
                &["debug.write", "debug.read", "hub.receipt", "hub.status"],
                &operator,
                None,
            )?;
            grant_for(
                &authority,
                hub.id(),
                &grants,
                &["debug.write", "hub.receipt", "hub.status"],
                &other_actor,
                None,
            )?;
            if let Some(method) = observation_method(case) {
                if case.ends_with("actor-isolation") {
                    grant_for(
                        &authority,
                        hub.id(),
                        &grants,
                        &[method],
                        &hub_actor,
                        Some(TICKET_77),
                    )?;
                } else if case.ends_with("ended-ticket") || case.ends_with("missing-ticket") {
                    grant_for(
                        &authority,
                        hub.id(),
                        &grants,
                        &[method],
                        &coglet,
                        Some(TICKET_77),
                    )?;
                    if case.ends_with("ended-ticket") {
                        authority.end_ticket(&Scope {
                            actor: coglet.clone(),
                            ticket: Some(TICKET_77.into()),
                        })?;
                    }
                } else if case.ends_with("scoped-allowed") {
                    grant_for(
                        &authority,
                        hub.id(),
                        &grants,
                        &[method],
                        &coglet,
                        Some(TICKET_66),
                    )?;
                }
            }
            let delegation = sign(
                &coglet_key,
                "delegation",
                json!({"purpose":"delegation","coglet":coglet,"hub":hub.id().to_string(),
                    "device":device.id().to_string(),"generation":DELEGATION_GEN,
                    "expires":DELEGATION_EXPIRES,"methods":lifecycle_methods()}),
            );
            authority
                .accept_delegation(hub.id(), device.id(), &coglet, &delegation, || Some(T0))?;
            delegation
        };
        let runtime_bytes = if case == "pause-single-step" {
            33_554_432u32
        } else {
            8_388_608u32
        };
        let pin = json!({
            "grant_generation": GRANT_GEN,
            "delegation_generation": DELEGATION_GEN,
            "native_schedule": {"trace_id":"11".repeat(16),"seed":1u64,"runtime_bytes":runtime_bytes,"stack_bytes":65_536u32},
            "back_channel": {"read_paths":["/data/","@step/"],"write_paths":write_paths(case)},
            "fetch_snapshots": {"refs/heads/logs": fetch_logs_snapshot()?},
        });
        let mut targets = BTreeMap::new();
        targets.insert(coglet.clone(), serde_json::from_value::<TargetPin>(pin)?);
        std::fs::write(root.join("kill.json"), b"{\"latched\":false}")?;
        let config = DeviceConfig {
            hub: hub.id(),
            device: device.id(),
            wal_root: root.join("wal"),
            quarantine: root.join("quarantine"),
            catalog: json!([]),
            grants: json!({"coglet":coglet,"operations":[],"resources":[]}),
            kill_switch: Some(root.join("kill.json")),
            targets,
            adapter: None,
        };
        let handler = DeviceHandler::new(
            config.clone(),
            Authority::open(&authority_path)?,
            validator_path(),
            Arc::new(|| Some(T0)),
        )?;
        let hub_accept = hub.clone();
        let incoming = tokio::spawn(async move {
            hub_accept
                .accept()
                .await
                .ok_or("no incoming")?
                .await
                .map_err(|e| e.to_string())
        });
        let device_conn = device.connect(hub.addr(), ALPN).await?;
        let hub_conn = incoming.await??;
        let device_session = Session::new(device_conn, device.id(), Role::Device)?;
        let hub_session = Session::new(hub_conn.clone(), hub.id(), Role::Hub)?;
        let hub_serve = tokio::spawn(async move {
            let _ = hub_session.clone().serve(Arc::new(Enroll)).await;
        });
        let hub_session = Session::new(hub_conn.clone(), hub.id(), Role::Hub)?;
        tokio::task::yield_now().await;
        let enrolled = device_session
            .request(
                json!({"method":"hub.enroll","target":null,"timeout_ms":5000,
                "correlation_id":"ab".repeat(16),
                "params":{"offer":{"versions":["0.1"]},"invite":"11".repeat(32)}}),
            )
            .await
            .map_err(|e| format!("enroll transport: {e}"))?;
        if enrolled["type"] != "reply" {
            return Err(format!("enroll failed: {enrolled}").into());
        }
        hub_serve.abort();
        let serve = tokio::spawn({
            let session = device_session.clone();
            let handler = handler.clone();
            async move {
                let _ = session.serve(Arc::new(handler)).await;
            }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mut fixture = Self {
            _dir: dir,
            root,
            coglet: coglet.clone(),
            hub_actor: hub_actor.clone(),
            operator: operator.clone(),
            other_actor: other_actor.clone(),
            session: hub_session,
            connection: hub_conn,
            handler: Some(handler),
            config,
            device_session,
            baseline: Value::Null,
            baseline_records: Vec::new(),
            observations: Vec::new(),
            prepared: Vec::new(),
            quarantine_baseline: Vec::new(),
            serve,
            _hold: None,
            held_since: None,
            _device: device,
            _hub: hub,
            _hub_serve: hub_serve,
        };
        fixture.install_guest(case, &hub_key, delegation).await?;
        Ok(fixture)
    }

    async fn install_guest(
        &mut self,
        case: &str,
        hub_key: &SecretKey,
        delegation: Value,
    ) -> Result<()> {
        let (wasm, compute) = guest_wasm(case)?;
        let (bytes, signed) = bundle(&self.root, hub_key, &self.coglet, &wasm, compute, case)?;
        let push = request(
            &self.coglet,
            "hub.git.push",
            "00000000000000000000000000000001",
            json!({"update":signed,"size":bytes.len(),"sha256":hex::encode(Sha256::digest(&bytes))}),
            &self.hub_actor,
            None,
        );
        let pushed = self
            .session
            .push_git(push, &mut bytes.as_slice())
            .await
            .map_err(|e| format!("push transport: {e}"))?;
        if pushed["type"] != "reply" {
            return Err(format!("push failed: {pushed}").into());
        }
        let install = request(
            &self.coglet,
            "hub.install",
            "00000000000000000000000000000002",
            json!({"initial":signed,"delegation":delegation}),
            &self.hub_actor,
            None,
        );
        let installed = self.session.request(install).await?;
        if installed["type"] != "reply" {
            return Err(format!("install failed: {installed}").into());
        }
        // Every default has a genuinely committed native step zero, not an
        // installed-but-never-run tree. No scheduler is pumped implicitly.
        for (n, method) in [
            (0x20u32, "debug.pause"),
            (0x21, "debug.step"),
            (0x23, "hub.stop"),
        ] {
            let reply = self
                .session
                .request(request(
                    &self.coglet,
                    method,
                    &format!("{n:032x}"),
                    json!({}),
                    &self.hub_actor,
                    None,
                ))
                .await?;
            if reply["type"] != "reply" {
                return Err(format!("step-zero {method} failed: {reply}").into());
            }
            if method == "debug.step" && reply["result"]["receipt"]["state"] != "applied" {
                return Err(format!("step-zero guest failed: {reply}").into());
            }
        }
        if matches!(case, "read-grant-is-not-write" | "read-scoped-allowed") {
            self.seed_file().await?;
        }
        if case == "cross-owner-receipt" {
            self.seed_other_receipt().await?;
        }
        if matches!(case, "pause-single-step" | "single-step-requires-pause") {
            let started = self
                .session
                .request(request(
                    &self.coglet,
                    "hub.start",
                    "00000000000000000000000000000003",
                    json!({}),
                    &self.hub_actor,
                    None,
                ))
                .await?;
            if started["type"] != "reply" {
                return Err(format!("start failed: {started}").into());
            }
        }
        // Sample the ORIGINAL retired WAL, then reopen the same receiver. This
        // establishes immutable before/after oracles without a production seam.
        let store = self.retire_and_open().await?;
        self.baseline = store.state().ok_or("missing baseline state")?.clone();
        self.baseline_records = store.records().to_vec();
        if self.baseline_records.len() != 1 || self.baseline_records[0]["step"] != 0 {
            return Err("baseline lacks actual committed native step zero".into());
        }
        if inbox_bytes(&self.baseline).is_some()
            || self.baseline["router"]["queue"] != json!([])
            || self.baseline["receiver"]["pending"] != json!([])
            || self.baseline_records[0]["status"] != "committed"
        {
            return Err("baseline has an inbox file".into());
        }
        drop(store);
        self.quarantine_baseline = self.quarantine_entries()?;
        self.reopen_receiver()?;
        if matches!(case, "inbox-boundary-queued" | "queue-backpressure") {
            self.hold_guest().await?;
        }
        if case == "queue-backpressure" {
            // The held debug.step already occupies one of the 256 pending slots.
            self.fill_pending(255).await?;
            // Hold readiness until the last reply can still meet its frozen
            // 2s deadline while the ACTUAL bounded guest completes within the
            // separate 9s teardown bound. This changes setup, not case timers.
            tokio::time::sleep_until(
                self.held_since.ok_or("no held start")? + Duration::from_millis(2500),
            )
            .await;
            let status = request(
                &self.coglet,
                "hub.status",
                &format!("{:032x}", 0x5000),
                json!({}),
                &self.hub_actor,
                None,
            );
            let reply = self.session.request(status.clone()).await?;
            if reply["result"]["pending"] != 256
                || self._hold.as_ref().is_none_or(JoinHandle::is_finished)
            {
                return Err(format!("pressure not live/full: {reply}").into());
            }
            self.prepared.push((status, reply));
        }
        Ok(())
    }

    async fn tree_hash(&self) -> Result<String> {
        let reply = self
            .session
            .request(request(
                &self.coglet,
                "debug.read",
                "00000000000000000000000000000024",
                json!({"path":"@step/0","offset":0,"limit":64}),
                &self.hub_actor,
                None,
            ))
            .await?;
        reply["result"]["snapshot_hash"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| format!("committed snapshot read failed: {reply}").into())
    }

    async fn seed_file(&self) -> Result<()> {
        let hash = self.tree_hash().await?;
        let write = json!({"method":"debug.write","target":self.coglet,
            "correlation_id":"00000000000000000000000000000004","timeout_ms":30000,
            "params":{"path":"/data/test.txt","text":"original","expected_hash":hash,"actor":self.operator}});
        let reply = self.session.request(write).await?;
        if reply["result"]["state"] != "applied" {
            return Err(format!("seed write failed: {reply}").into());
        }
        Ok(())
    }

    async fn seed_other_receipt(&self) -> Result<()> {
        let hash = self.tree_hash().await?;
        let write = json!({"method":"debug.write","target":self.coglet,
            "correlation_id":OTHER_OP,"timeout_ms":30000,
            "params":{"path":"/data/notes/other","text":"owned","expected_hash":hash,"actor":self.other_actor}});
        let reply = self.session.request(write).await?;
        if reply["result"]["state"] != "applied" {
            return Err(format!("cross-owner seed failed: {reply}").into());
        }
        Ok(())
    }

    async fn hold_guest(&mut self) -> Result<()> {
        let pause = request(
            &self.coglet,
            "debug.pause",
            "00000000000000000000000000000005",
            json!({}),
            &self.hub_actor,
            None,
        );
        let paused = self.session.request(pause).await?;
        if paused["result"]["state"] != "applied" {
            return Err(format!("pause failed: {paused}").into());
        }
        let step = request(
            &self.coglet,
            "debug.step",
            "00000000000000000000000000000006",
            json!({}),
            &self.hub_actor,
            None,
        );
        let session = self.session.clone();
        let hold = tokio::spawn(async move {
            session.request(step).await.unwrap_or_else(
                |e| json!({"type":"error","error":{"code":"unavailable","message":e.to_string()}}),
            )
        });
        let shared = self
            .handler
            .as_ref()
            .unwrap()
            .share_service(&self.coglet)
            .ok_or("no service")?;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if shared
                    .try_lock()
                    .is_ok_and(|locked| locked.has_unresolved_attempt())
                {
                    break Ok(());
                }
                if hold.is_finished() {
                    break Err("guest finished before in-progress fence");
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        self._hold = Some(hold);
        self.held_since = Some(tokio::time::Instant::now());
        Ok(())
    }

    async fn fill_pending(&mut self, count: usize) -> Result<()> {
        let handler = self.handler.as_ref().ok_or("no handler")?.clone();
        let peer = self._hub.id();
        let local = self._device.id();
        for i in 0..count {
            let id = format!("{:032x}", 0x1000 + i);
            let message_id = format!("{:032x}", 0x2000 + i);
            let inbox = json!({"method":"hub.inbox","target":self.coglet,"correlation_id":id,"timeout_ms":30000,
                "params":{"message":{"id":message_id,"sender":self.hub_actor,
                    "event":{"source":self.hub_actor,"kind":"hub.note","seq":i as u64,"logical_time":0,
                    "payload":{"n":i},"correlation_id":message_id}},"actor":self.hub_actor}});
            let context = RequestContext {
                peer,
                local,
                deadline: tokio::time::Instant::now() + Duration::from_secs(30),
            };
            let reply = handler.handle(context, inbox.clone()).await;
            if i % 32 == 0 && std::env::var_os("RECEIVER_FIXTURE_TRACE").is_some() {
                eprintln!(
                    "fixture: pending fill {i} at {:?}",
                    std::time::SystemTime::now()
                );
            }
            if reply["result"]["state"] != "queued" {
                return Err(format!("pending fill {i} failed: {reply}").into());
            }
            self.prepared.push((inbox, reply));
        }
        Ok(())
    }

    pub async fn forward(&mut self, request: Value) -> Result<Value> {
        if request["method"] == "hub.git.fetch" {
            let (mut send, mut recv) = self.connection.open_bi().await?;
            wire::write_frame(&mut send, &request).await?;
            let frame = wire::read_frame(&mut recv).await?;
            if frame["type"] == "transfer" {
                let _ = send.reset(1u32.into());
                let _ = recv.stop(1u32.into());
            } else {
                let _ = send.finish();
            }
            self.observations.push((request, frame.clone()));
            return Ok(frame);
        }
        let reply = self.session.request(request.clone()).await?;
        self.observations.push((request, reply.clone()));
        Ok(reply)
    }

    fn quarantine_entries(&self) -> Result<Vec<String>> {
        let mut entries = std::fs::read_dir(&self.config.quarantine)?
            .map(|e| e.map(|e| e.file_name().to_string_lossy().into_owned()))
            .collect::<std::io::Result<Vec<_>>>()?;
        entries.sort();
        Ok(entries)
    }

    async fn retire_and_open(&mut self) -> Result<coglet_runtime::DurableStore> {
        self.serve.abort();
        let _ = (&mut self.serve).await;
        drop(self.handler.take());
        let wal = self
            .root
            .join("wal")
            .join(digest(self.coglet.as_bytes()))
            .join("state.wal");
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        loop {
            match coglet_runtime::DurableStore::open(&wal) {
                Ok(store) => return Ok(store),
                Err(error)
                    if error.starts_with("WAL already owned:")
                        && tokio::time::Instant::now() < deadline =>
                {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(error) => return Err(error.into()),
            }
        }
    }

    fn reopen_receiver(&mut self) -> Result<()> {
        let handler = DeviceHandler::new(
            self.config.clone(),
            Authority::open(self.root.join("authority.sqlite"))?,
            validator_path(),
            Arc::new(|| Some(T0)),
        )?;
        self.serve = tokio::spawn({
            let handler = handler.clone();
            let session = self.device_session.clone();
            async move {
                let _ = session.serve(Arc::new(handler)).await;
            }
        });
        self.handler = Some(handler);
        Ok(())
    }

    pub async fn oracle(&mut self, case: &str, last: &Value) -> Result<()> {
        if let Some(hold) = self._hold.take() {
            // Await REAL guest completion, never abort a guest to manufacture
            // a recovered snapshot. Its returned failure is a recorded outcome.
            let reply = tokio::time::timeout(Duration::from_secs(7), hold).await??;
            if reply["type"] != "reply" || reply["result"]["receipt"]["state"] == "queued" {
                return Err(format!("held guest did not finish: {reply}").into());
            }
        }
        let store = self.retire_and_open().await?;
        let state = store.state().ok_or("missing wal state")?;
        if case.starts_with("fetch-") {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while self.quarantine_entries()? != self.quarantine_baseline {
                if tokio::time::Instant::now() >= deadline {
                    return Err("fetch reset retained a quarantine lease".into());
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        if let Ok(path) = std::env::var("COGLET_RECEIVER_RESULT") {
            std::fs::write(
                format!("{path}.oracle.json"),
                serde_json::to_vec_pretty(&json!({
                    "case":case,"baseline":self.baseline,"baseline_records":self.baseline_records,
                    "state":state,"records":store.records(),"observations":self.observations,"prepared":self.prepared,
                }))?,
            )?;
        }
        match case {
            "action-expired" => {
                if !state["dispatches"]
                    .as_object()
                    .is_some_and(|d| d.is_empty())
                    || !state["dispatch_order"]
                        .as_array()
                        .is_some_and(|d| d.is_empty())
                    || state["receiver"]["rejected_actions"]
                        .as_object()
                        .map(|o| o.len())
                        != Some(1)
                    || store.records() != self.baseline_records
                {
                    return Err("action-expired wal oracle failed".into());
                }
            }
            "inbox-durable-dedup" | "inbox-conflicting-id" => inbox_one_line(state)?,
            "inbox-boundary-queued" => {
                if inbox_bytes(state).is_some() {
                    return Err("queued cancel left an inbox append".into());
                }
                if last["result"]["state"] != "cancelled" {
                    return Err("inbox-boundary-queued missing cancelled receipt".into());
                }
            }
            "queue-backpressure" if inbox_contains(state, "ffffffffffffffffffffffffffffffff") => {
                return Err("backpressure admitted the overflowing inbox".into());
            }
            _ => {}
        }
        Ok(())
    }
}

fn inbox_bytes(state: &Value) -> Option<Vec<u8>> {
    let hex = state
        .pointer("/staged_boundary/runtime/files/~1data~1inbox.jsonl")
        .or_else(|| state.pointer("/runtime/files/~1data~1inbox.jsonl"))
        .and_then(|v| v.as_str())?;
    hex::decode(hex).ok()
}

fn inbox_one_line(state: &Value) -> Result<()> {
    let raw = inbox_bytes(state).ok_or("missing inbox.jsonl")?;
    let text = std::str::from_utf8(&raw)?;
    let mut lines = text.lines().filter(|line| !line.is_empty());
    let line = lines.next().ok_or("empty inbox")?;
    if lines.next().is_some() {
        return Err("inbox jsonl is not exactly one record".into());
    }
    let message: Value = serde_json::from_str(line)?;
    if message["index"] != 0
        || message["id"] != "ffffffffffffffffffffffffffffffff"
        || message["event"]["payload"]["text"] != "hello\nworld"
    {
        return Err("inbox oracle content mismatch".into());
    }
    if raw.contains(&b'\n') && line.as_bytes().contains(&b'\n') {
        return Err("raw newline split the event".into());
    }
    Ok(())
}

fn inbox_contains(state: &Value, id: &str) -> bool {
    inbox_bytes(state)
        .and_then(|raw| String::from_utf8(raw).ok())
        .is_some_and(|text| text.contains(id))
}

pub fn ready() -> Result<()> {
    eprintln!("COGLET_CONFORMANCE_READY");
    std::io::stderr().flush()?;
    Ok(())
}

pub async fn serve_stdio(mut fixture: Fixture, requests: usize, case: &str) -> Result<()> {
    ready()?;
    let mut input = tokio::io::stdin();
    let mut output = tokio::io::stdout();
    for index in 0..requests {
        let request = wire::read_frame(&mut input).await?;
        let started = std::time::Instant::now();
        let reply = fixture.forward(request.clone()).await?;
        if std::env::var_os("RECEIVER_FIXTURE_TRACE").is_some() {
            eprintln!(
                "fixture: {} -> {} in {:?}",
                request["method"],
                reply["type"],
                started.elapsed()
            );
        }
        wire::write_frame(&mut output, &reply).await?;
        output.flush().await?;
        if index + 1 == requests {
            fixture.oracle(case, &reply).await?;
        }
    }
    Ok(())
}
