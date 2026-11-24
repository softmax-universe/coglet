//! Real-iroh router -> device lifecycle tests against the actual `DeviceHandler`.
//!
//! The router side is `coglet-mock`'s `MockHub` (real iroh endpoint + shared hub
//! kernel); the device side is the production `DeviceHandler` served through a
//! real `Session`. Git bundles are real, signed with the router key, validated by
//! the actual `coglet-git-validator` helper. Authority (enrollment grants, exact
//! scopes, the coglet-signed delegation) is pre-provisioned locally by the
//! operator; no remote delegation acceptance and no test-only flags are used.
//!
//! `hub.start` needs the native engine. Without `--features native-engine` the
//! tests prove start fails closed (no mock fallback); with it they prove
//! start -> running -> stop. A separate native test explicitly invokes the installed
//! guest through the handler's shared RuntimeService (not an adapter socket).
#![cfg(target_os = "linux")]

use coglet_mock::hub::MockHub;
use coglet_proto::{
    authority::{Authority, Grant, Scope},
    signing,
    transport::{ALPN, Role, Session},
};
use coglet_runtime::{DeviceConfig, DeviceHandler, TargetPin};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::task::JoinHandle;

const T0: u64 = 1_800_000_000;
const DELEGATION_EXPIRES: u64 = T0 + 1_000;
const GRANT_GEN: u64 = 7;
const TICKET: &str = "abababababababababababababababab";
const OTHER_TICKET: &str = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
const NARROW_TICKET: &str = "efefefefefefefefefefefefefefefef";
const DELEGATION_GEN: u64 = 9;
const METHODS: [&str; 6] = [
    "hub.git.push",
    "hub.install",
    "hub.start",
    "hub.stop",
    "hub.status",
    "hub.receipt",
];
const WASM_WAT: &str = "(module (memory (export \"memory\") 1 2) \
    (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) \
    (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))";

#[cfg(feature = "native-engine")]
const NATIVE: bool = true;
#[cfg(not(feature = "native-engine"))]
const NATIVE: bool = false;

type Clock = Arc<AtomicU64>;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Use the runtime package's installed shared validator, built by Cargo.
fn validator_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_coglet-runtime-git-validator"))
}

fn key(seed: u8, role: u8) -> SecretKey {
    let mut bytes = [seed; 32];
    bytes[31] = role;
    SecretKey::from_bytes(&bytes)
}

/// One-shot action run inside the handler's N-th trusted-clock sample.
type Trap = Arc<Mutex<Option<(u64, Box<dyn FnOnce() + Send>)>>>;

fn clock_fn(
    clock: &Clock,
    reads: &Clock,
    trap: &Trap,
) -> Arc<dyn Fn() -> Option<u64> + Send + Sync> {
    let (clock, reads, trap) = (clock.clone(), reads.clone(), trap.clone());
    Arc::new(move || {
        let n = reads.fetch_add(1, Ordering::SeqCst) + 1;
        let action = {
            let mut armed = trap.lock().unwrap();
            if armed.as_ref().is_some_and(|(at, _)| *at == n) {
                armed.take().map(|(_, action)| action)
            } else {
                None
            }
        };
        if let Some(action) = action {
            action();
        }
        Some(clock.load(Ordering::SeqCst))
    })
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

fn sign(key: &SecretKey, purpose: &str, statement: Value) -> Value {
    let signature = key.sign(&signing::preimage(purpose, &statement).unwrap());
    json!({"signature":hex::encode(signature.to_bytes()),"signer":key.public().to_string(),"statement":statement})
}

fn correlation(n: u64) -> String {
    format!("{n:032x}")
}

/// Scenario knobs. Defaults are the happy path.
#[derive(Clone)]
struct Options {
    delegation_methods: Vec<&'static str>,
    pin_grant_generation: u64,
    pin_delegation_generation: u64,
    native_schedule: bool,
    /// Provision extra identities (other ticket, no ticket, other actor, narrow
    /// ticket) for hub.receipt cross-identity tests.
    extra_scopes: bool,
}
impl Default for Options {
    fn default() -> Self {
        Self {
            delegation_methods: vec![
                "hub.install",
                "hub.start",
                "hub.stop",
                "hub.status",
                "hub.receipt",
            ],
            pin_grant_generation: GRANT_GEN,
            pin_delegation_generation: DELEGATION_GEN,
            native_schedule: NATIVE,
            extra_scopes: false,
        }
    }
}

impl Options {
    /// Default scenario plus the extra identities hub.receipt tests need.
    fn receipts() -> Self {
        Self {
            extra_scopes: true,
            ..Self::default()
        }
    }
}

struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    clock: Clock,
    /// Number of trusted-clock samples taken by the handler (deterministic
    /// progress marker for a request blocked after its first authorization).
    reads: Clock,
    trap: Trap,
    hub: MockHub,
    hub_key: SecretKey,
    coglet_key: SecretKey,
    coglet: String,
    actor: String,
    other_actor: String,
    ticket: String,
    endpoint: Endpoint,
    device_session: Session,
    session: Session,
    handler: Option<DeviceHandler>,
    serve: JoinHandle<()>,
    config: DeviceConfig,
    seq: u64,
    delegation_methods: Vec<&'static str>,
}

impl Env {
    /// `seed` gives each concurrent test distinct router/device/coglet identities;
    /// session scheduler budgets are shared per authenticated (local, peer).
    async fn new(seed: u8, options: Options) -> Result<Self> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().to_owned();
        let clock: Clock = Arc::new(AtomicU64::new(T0));
        let reads: Clock = Arc::new(AtomicU64::new(0));
        let trap: Trap = Arc::new(Mutex::new(None));
        let hub_key = key(seed, 1);
        let device_key = key(seed, 2);
        let coglet_key = key(seed, 3);
        let coglet = format!("coglet:{}", coglet_key.public());
        let actor = format!("hub:{}", hub_key.public());
        let ticket = TICKET.to_owned();
        let other_actor = format!("hub:{}", key(seed, 9).public());
        let hub = MockHub::with_clock(hub_key.clone(), Arc::new(|| Some(T0))).await?;
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(device_key.clone())
            .bind()
            .await?;

        // Operator provisioning of the device's own durable Authority.
        let authority_path = root.join("authority.sqlite");
        {
            let authority = Authority::open(&authority_path)?;
            authority.open_window(Duration::from_secs(30))?;
            let grants: Vec<Grant> = METHODS
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
            let scope = Scope {
                actor: actor.clone(),
                ticket: Some(ticket.clone()),
            };
            for grant in &grants {
                authority.provision_scope(hub.id(), &scope, grant)?;
            }
            // Further provisioned identities for cross-identity receipt tests:
            // same actor/other ticket, same actor without a ticket, another actor,
            // and a ticket that may poll receipts but holds no lifecycle grant.
            let other_actor = format!("hub:{}", key(seed, 9).public());
            for (who, ticket) in [
                (&actor, Some(OTHER_TICKET)),
                (&actor, None),
                (&other_actor, Some(TICKET)),
            ]
            .into_iter()
            .filter(|_| options.extra_scopes)
            {
                let scope = Scope {
                    actor: who.clone(),
                    ticket: ticket.map(str::to_owned),
                };
                for grant in &grants {
                    authority.provision_scope(hub.id(), &scope, grant)?;
                }
            }
            let narrow = Scope {
                actor: actor.clone(),
                ticket: Some(NARROW_TICKET.to_owned()),
            };
            for grant in grants.iter().filter(|g| {
                options.extra_scopes && (g.method == "hub.receipt" || g.method == "hub.status")
            }) {
                authority.provision_scope(hub.id(), &narrow, grant)?;
            }
            let delegation = Self::delegation_for(
                &coglet_key,
                &coglet,
                hub.id().to_string(),
                endpoint.id().to_string(),
                DELEGATION_GEN,
                DELEGATION_EXPIRES,
                &options.delegation_methods,
            );
            authority
                .accept_delegation(hub.id(), endpoint.id(), &coglet, &delegation, || Some(T0))?;
        }

        let mut targets = BTreeMap::new();
        let native_schedule = options.native_schedule.then(|| {
            json!({"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8_388_608,"stack_bytes":65_536})
        });
        targets.insert(
            coglet.clone(),
            serde_json::from_value::<TargetPin>(json!({
                "grant_generation": options.pin_grant_generation,
                "delegation_generation": options.pin_delegation_generation,
                "native_schedule": native_schedule,
            }))?,
        );
        std::fs::write(root.join("kill.json"), b"{\"latched\":false}")?;
        let config = DeviceConfig {
            hub: hub.id(),
            device: endpoint.id(),
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
            clock_fn(&clock, &reads, &trap),
        )?;
        let (device_session, session, serve) = attach(&hub, &endpoint, &handler).await?;
        Ok(Self {
            _dir: dir,
            root,
            clock,
            reads,
            trap,
            hub,
            hub_key,
            coglet_key,
            coglet,
            actor,
            other_actor,
            ticket,
            endpoint,
            device_session,
            session,
            handler: Some(handler),
            serve,
            config,
            seq: 0,
            delegation_methods: options.delegation_methods,
        })
    }

    fn delegation_for(
        coglet_key: &SecretKey,
        coglet: &str,
        hub: String,
        device: String,
        generation: u64,
        expires: u64,
        methods: &[&str],
    ) -> Value {
        sign(
            coglet_key,
            "delegation",
            json!({"purpose":"delegation","coglet":coglet,"hub":hub,"device":device,
                "generation":generation,"expires":expires,"methods":methods}),
        )
    }

    /// The operator-provisioned delegation (the one stored by `Authority`).
    fn delegation(&self) -> Value {
        Self::delegation_for(
            &self.coglet_key,
            &self.coglet,
            self.hub.id().to_string(),
            self.endpoint.id().to_string(),
            DELEGATION_GEN,
            DELEGATION_EXPIRES,
            &self.delegation_methods,
        )
    }

    fn next(&mut self) -> String {
        self.seq += 1;
        correlation(self.seq)
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        let id = self.next();
        self.request_with(
            method,
            id,
            params,
            &self.actor.clone(),
            Some(self.ticket.clone()),
        )
    }

    fn request_with(
        &self,
        method: &str,
        id: String,
        params: Value,
        actor: &str,
        ticket: Option<String>,
    ) -> Value {
        let mut context = json!({"actor":actor});
        if let Some(ticket) = ticket {
            context["ticket"] = json!(ticket);
        }
        json!({"method":method,"target":self.coglet,"correlation_id":id,"timeout_ms":30_000,
            "params":params,"context":context})
    }

    /// Real signed Git bundle containing manifest.json, bin/step.wasm and
    /// build/provenance.cbor. `old` is the previous signed tip for generation > 1.
    fn bundle(&self, generation: u64, old: Option<&str>, extra: &[(&str, &[u8])]) -> Bundle {
        self.bundle_wasm(generation, old, extra, WASM_WAT)
    }

    fn bundle_wasm(
        &self,
        generation: u64,
        old: Option<&str>,
        extra: &[(&str, &[u8])],
        wasm_wat: &str,
    ) -> Bundle {
        let work = self.root.join(format!("git-{generation}"));
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "--initial-branch=hub"]);
        let wasm = wat::parse_str(wasm_wat).unwrap();
        let mode = native_compute_mode();
        let manifest = json!({"version":"0.1","id":self.coglet,"subscriptions":[],
            "required_capabilities":[],"exclusive_outputs":[],
            "budgets":{"compute":10000,"compute_mode":mode,"memory_pages":2,
                "wall_time_us":2_000_000,"max_events":16,"host_bytes":65536}});
        std::fs::write(
            work.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        std::fs::create_dir_all(work.join("bin")).unwrap();
        std::fs::write(work.join("bin/step.wasm"), &wasm).unwrap();
        let record = coglet_types::HubBuildRecord {
            version: "0.1".into(),
            source_commit: "ab".repeat(20),
            recipe_sha256: "11".repeat(32),
            policy_sha256: "22".repeat(32),
            transform_sha256: "33".repeat(32),
            original_sha256: hex::encode(Sha256::digest(&wasm)),
            rewritten_sha256: hex::encode(Sha256::digest(&wasm)),
        }
        .to_cbor()
        .unwrap();
        std::fs::create_dir_all(work.join("build")).unwrap();
        std::fs::write(work.join("build/provenance.cbor"), record).unwrap();
        for (path, bytes) in extra {
            std::fs::write(work.join(path), bytes).unwrap();
        }
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "code"]);
        let tip = String::from_utf8(git(&work, &["rev-parse", "HEAD"]))
            .unwrap()
            .trim()
            .to_owned();
        let archive = git(&work, &["archive", "--format=tar", "HEAD"]);
        git(
            &work,
            &["bundle", "create", "transfer.bundle", "refs/heads/hub"],
        );
        let bytes = std::fs::read(work.join("transfer.bundle")).unwrap();
        let signed = sign(
            &self.hub_key,
            "hub-ref",
            json!({"purpose":"hub-ref","hub":self.hub.id().to_string(),"coglet":self.coglet,
                "ref":"refs/heads/hub","old":old,"new":tip,
                "archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":generation}),
        );
        Bundle { bytes, signed, tip }
    }

    async fn push(&mut self, bundle: &Bundle) -> Value {
        let request = self.request(
            "hub.git.push",
            json!({"update":bundle.signed,"size":bundle.bytes.len(),
                "sha256":hex::encode(Sha256::digest(&bundle.bytes))}),
        );
        self.session
            .push_git(request, &mut bundle.bytes.as_slice())
            .await
            .expect("push reply")
    }

    async fn install(&mut self, initial: &Value, delegation: &Value) -> Value {
        let request = self.request(
            "hub.install",
            json!({"initial":initial,"delegation":delegation}),
        );
        self.call(request).await
    }
    async fn install_request(&mut self, id: String, initial: &Value) -> Value {
        let request = self.request_with(
            "hub.install",
            id,
            json!({"initial":initial,"delegation":self.delegation()}),
            &self.actor.clone(),
            Some(self.ticket.clone()),
        );
        self.call(request).await
    }
    async fn simple(&mut self, method: &str) -> Value {
        let request = self.request(method, json!({}));
        self.call(request).await
    }
    async fn call(&self, request: Value) -> Value {
        self.session
            .request(request)
            .await
            .expect("session request completes with a terminal frame")
    }

    /// Replace the handler/authority as a process restart would: same WAL root,
    /// same Authority file, same device identity and connection.
    async fn restart_handler(&mut self) {
        let retired = self.handler.as_ref().and_then(|handler| {
            handler
                .share_service(&self.coglet)
                .map(|service| Arc::downgrade(&service))
        });
        self.serve.abort();
        let _ = (&mut self.serve).await;
        drop(self.handler.take());
        if let Some(retired) = retired {
            tokio::time::timeout(Duration::from_secs(5), async {
                while retired.upgrade().is_some() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("old handler releases the coordinator");
        }
        // Aborting Session::serve aborts stream tasks, but they may still hold
        // the old handler until their async cancellation is driven. A new
        // handler has an independent coordinator map; do not race its first
        // request against the old coordinator's OS flock destruction.
        let wal = self
            .config
            .wal_root
            .join(coglet_runtime::digest(self.coglet.as_bytes()))
            .join("state.wal");
        if wal.exists() {
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match coglet_runtime::DurableStore::open(&wal) {
                        Ok(store) => {
                            drop(store);
                            break;
                        }
                        Err(error) if error.starts_with("WAL already owned:") => {
                            tokio::task::yield_now().await;
                        }
                        Err(error) => panic!("restart WAL recovery failed: {error}"),
                    }
                }
            })
            .await
            .expect("old stream tasks release the WAL lock");
        }
        let handler = DeviceHandler::new(
            self.config.clone(),
            Authority::open(self.root.join("authority.sqlite")).unwrap(),
            validator_path(),
            clock_fn(&self.clock, &self.reads, &self.trap),
        )
        .unwrap();
        self.serve = tokio::spawn(serve(self.device_session.clone(), handler.clone()));
        self.handler = Some(handler);
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        self.serve.abort();
    }
}

struct Bundle {
    bytes: Vec<u8>,
    signed: Value,
    tip: String,
}

#[cfg(feature = "native-engine")]
fn native_compute_mode() -> String {
    coglet_engine_wamr::COMPUTE_MODE.into()
}
#[cfg(not(feature = "native-engine"))]
fn native_compute_mode() -> String {
    "fixture.interpreter".into()
}

async fn serve(session: Session, handler: DeviceHandler) {
    let _ = session.serve(Arc::new(handler)).await;
}

/// Device enrolls with the (mock) router over real iroh, then serves the handler.
/// Returns (device-side session, router-side session, serve task).
async fn attach(
    hub: &MockHub,
    endpoint: &Endpoint,
    handler: &DeviceHandler,
) -> Result<(Session, Session, JoinHandle<()>)> {
    let invite = hub
        .invite(
            endpoint.id(),
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: u64::MAX,
                generation: 0,
            }],
        )
        .await?;
    let connection = endpoint.connect(hub.addr(), ALPN).await?;
    let device = Session::new(connection, endpoint.id(), Role::Device)?;
    device
        .request(
            json!({"method":"hub.enroll","target":null,"timeout_ms":5000,
            "correlation_id":"ab".repeat(16),
            "params":{"offer":{"versions":["0.1"]},"invite":invite}}),
        )
        .await?;
    let job = tokio::spawn(serve(device.clone(), handler.clone()));
    for _ in 0..200 {
        if let Some(session) = hub.session(endpoint.id()).await {
            return Ok((device, session, job));
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    Err("router never registered the device session".into())
}

// ---- reply helpers -------------------------------------------------------

fn error_code(reply: &Value) -> String {
    assert_eq!(reply["type"], "error", "expected error frame: {reply}");
    reply["error"]["code"].as_str().unwrap().to_owned()
}
fn applied(reply: &Value) -> &Value {
    assert_eq!(reply["type"], "reply", "expected success: {reply}");
    assert_eq!(reply["result"]["state"], "applied", "{reply}");
    assert_eq!(reply["result"]["error"], Value::Null);
    &reply["result"]
}
fn denied(reply: &Value) {
    assert_eq!(error_code(reply), "permission_denied", "{reply}");
}
fn state(reply: &Value) -> (String, Value) {
    assert_eq!(reply["type"], "reply", "status failed: {reply}");
    (
        reply["result"]["state"].as_str().unwrap().to_owned(),
        reply["result"]["live_commit"].clone(),
    )
}

/// push -> install, returning (bundle, install receipt).
async fn pushed_and_installed(env: &mut Env) -> (Bundle, Value) {
    let bundle = env.bundle(1, None, &[]);
    let pushed = env.push(&bundle).await;
    assert_eq!(pushed["result"]["state"], "queued", "{pushed}");
    let delegation = env.delegation();
    let receipt = env.install(&bundle.signed, &delegation).await;
    applied(&receipt);
    (bundle, receipt)
}

macro_rules! timed {
    ($body:expr) => {
        tokio::time::timeout(Duration::from_secs(120), $body)
            .await
            .expect("test exceeded its bounded deadline")
    };
}

#[path = "support/installed_push_cases.rs"]
mod installed_push_cases;

// ---- tests ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_install_stopped_then_stop_and_repeat_receipt() {
    timed!(async {
        let mut env = Env::new(0x21, Options::default()).await.unwrap();
        // Not yet installed: status is the stopped default with no live commit.
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), Value::Null)
        );
        let (bundle, receipt) = pushed_and_installed(&mut env).await;
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        // Identical install (same correlation id) returns the original receipt;
        // a new correlation id for the same installed identity does too.
        let same_id = receipt["correlation_id"].as_str().unwrap().to_owned();
        assert_eq!(env.install_request(same_id, &bundle.signed).await, receipt);
        let original_id = receipt["result"]["operation_id"].clone();
        let again = env.install_request(correlation(1001), &bundle.signed).await;
        assert_eq!(again["result"]["operation_id"], original_id, "{again}");
        assert_eq!(again["result"], receipt["result"]);
        // Install did not start it, and stopping a stopped coglet is idempotent.
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        applied(&env.simple("hub.stop").await);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        // A known-but-unimplemented method is denied (if its request is schema-valid).
        let fetch = env.request("hub.cancel", json!({"id":"cd".repeat(16)}));
        if let Ok(frame) = env.session.request(fetch).await {
            denied(&frame);
        }
    });
}

#[cfg(not(feature = "native-engine"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_fails_closed_without_native_engine_and_stays_stopped() {
    timed!(async {
        // No schedule pin, and a schedule pin without the feature: both refuse.
        for (seed, schedule) in [(0x22, false), (0x23, true)] {
            let options = Options {
                native_schedule: schedule,
                ..Options::default()
            };
            let mut env = Env::new(seed, options).await.unwrap();
            let (bundle, _) = pushed_and_installed(&mut env).await;
            let started = env.simple("hub.start").await;
            assert_eq!(error_code(&started), "unavailable", "{started}");
            // No mock fallback: the coglet never became running.
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(bundle.tip))
            );
        }
    });
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_start_running_stop_with_native_engine() {
    timed!(async {
        let mut env = Env::new(0x24, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        let started = env.simple("hub.start").await;
        applied(&started);
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("running".into(), json!(bundle.tip))
        );
        // Repeating start is idempotent.
        applied(&env.simple("hub.start").await);
        assert_eq!(state(&env.simple("hub.status").await).0, "running");
        applied(&env.simple("hub.stop").await);
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        // Durable stop survives a handler/authority restart.
        env.restart_handler().await;
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        applied(&env.simple("hub.start").await);
        assert_eq!(state(&env.simple("hub.status").await).0, "running");
        // A running coglet restarts into its durable running state, and stop works.
        env.restart_handler().await;
        assert_eq!(state(&env.simple("hub.status").await).0, "running");
        applied(&env.simple("hub.stop").await);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wal_restart_preserves_installed_and_stopped_state() {
    timed!(async {
        let mut env = Env::new(0x25, Options::default()).await.unwrap();
        let (bundle, receipt) = pushed_and_installed(&mut env).await;
        applied(&env.simple("hub.stop").await);
        for _ in 0..2 {
            env.restart_handler().await;
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(bundle.tip))
            );
        }
        // The durable install receipt is replayed exactly after restart, and
        // the accepted ref still permits only the installed identity.
        assert_eq!(
            env.install_request(correlation(2000), &bundle.signed).await["result"],
            receipt["result"]
        );
        // An install of a different (unaccepted) commit is refused post-restart.
        let other = env.bundle(2, Some(&bundle.tip), &[("note.txt", b"x")]);
        denied(&env.install(&other.signed, &env.delegation()).await);
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_rejects_wrong_expired_and_wrong_generation_signed_delegations() {
    timed!(async {
        let mut env = Env::new(0x26, Options::default()).await.unwrap();
        let bundle = env.bundle(1, None, &[]);
        assert_eq!(env.push(&bundle).await["result"]["state"], "queued");
        let methods = Options::default().delegation_methods;
        let hub = env.hub.id().to_string();
        let device = env.endpoint.id().to_string();
        let other_node = key(0x66, 9).public().to_string();
        let other_hub = key(0x67, 9).public().to_string();
        let forged = |hub: String,
                      device: String,
                      generation: u64,
                      expires: u64,
                      signer: &SecretKey,
                      coglet: &str| {
            Env::delegation_for(signer, coglet, hub, device, generation, expires, &methods)
        };
        let impostor = key(0x68, 9);
        let cases = [
            (
                "wrong node",
                forged(
                    hub.clone(),
                    other_node,
                    DELEGATION_GEN,
                    DELEGATION_EXPIRES,
                    &env.coglet_key,
                    &env.coglet,
                ),
            ),
            (
                "wrong hub",
                forged(
                    other_hub,
                    device.clone(),
                    DELEGATION_GEN,
                    DELEGATION_EXPIRES,
                    &env.coglet_key,
                    &env.coglet,
                ),
            ),
            (
                "expired",
                forged(
                    hub.clone(),
                    device.clone(),
                    DELEGATION_GEN,
                    T0,
                    &env.coglet_key,
                    &env.coglet,
                ),
            ),
            (
                "generation behind",
                forged(
                    hub.clone(),
                    device.clone(),
                    DELEGATION_GEN - 1,
                    DELEGATION_EXPIRES,
                    &env.coglet_key,
                    &env.coglet,
                ),
            ),
            (
                "generation ahead",
                forged(
                    hub.clone(),
                    device.clone(),
                    DELEGATION_GEN + 1,
                    DELEGATION_EXPIRES,
                    &env.coglet_key,
                    &env.coglet,
                ),
            ),
            (
                "wrong signer",
                forged(
                    hub.clone(),
                    device.clone(),
                    DELEGATION_GEN,
                    DELEGATION_EXPIRES,
                    &impostor,
                    &env.coglet,
                ),
            ),
        ];
        for (name, delegation) in cases {
            let reply = env.install(&bundle.signed, &delegation).await;
            assert_eq!(error_code(&reply), "permission_denied", "{name}: {reply}");
            // Nothing was installed by a rejected delegation.
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), Value::Null),
                "{name}"
            );
        }
        // Tampering with an otherwise valid signed delegation is also rejected.
        let mut tampered = env.delegation();
        tampered["statement"]["expires"] = json!(DELEGATION_EXPIRES + 1);
        denied(&env.install(&bundle.signed, &tampered).await);
        assert_eq!(state(&env.simple("hub.status").await).1, Value::Null);
        // The exact provisioned delegation still installs.
        let good = env.delegation();
        applied(&env.install(&bundle.signed, &good).await);
        assert_eq!(state(&env.simple("hub.status").await).1, json!(bundle.tip));
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stored_delegation_expiry_and_clock_loss_deny_at_the_exact_second() {
    timed!(async {
        let mut env = Env::new(0x27, Options::default()).await.unwrap();
        let (bundle, receipt) = pushed_and_installed(&mut env).await;
        env.clock.store(DELEGATION_EXPIRES - 1, Ordering::SeqCst);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        applied(&env.simple("hub.stop").await);
        // now == expires is already expired for every lifecycle method.
        env.clock.store(DELEGATION_EXPIRES, Ordering::SeqCst);
        for method in ["hub.status", "hub.start", "hub.stop"] {
            denied(&env.simple(method).await);
        }
        denied(&env.install_request(correlation(3000), &bundle.signed).await);
        env.clock
            .store(DELEGATION_EXPIRES + 10_000, Ordering::SeqCst);
        denied(&env.simple("hub.status").await);
        // Expiry is evaluated from the injected trusted clock, not cached: rewinding
        // it (a test-only fixture clock) restores access, and nothing was mutated.
        env.clock.store(DELEGATION_EXPIRES - 1, Ordering::SeqCst);
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        assert_eq!(
            env.install_request(correlation(3001), &bundle.signed).await["result"],
            receipt["result"]
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn enrollment_grant_expiry_is_independent_of_delegation_expiry() {
    timed!(async {
        let mut env = Env::new(0x28, Options::default()).await.unwrap();
        let _ = pushed_and_installed(&mut env).await;
        // Grants and scopes expire after the delegation, so only the clock moving
        // past the (later) grant expiry could matter; delegation already denies.
        env.clock.store(T0 + 100_000, Ordering::SeqCst);
        denied(&env.simple("hub.status").await);
        // Pushes use the same enrollment authority and are denied at that time too.
        let bundle = env.bundle(2, None, &[("late.txt", b"x")]);
        let request = env.request(
            "hub.git.push",
            json!({"update":bundle.signed,"size":bundle.bytes.len(),
                "sha256":hex::encode(Sha256::digest(&bundle.bytes))}),
        );
        if let Ok(frame) = env
            .session
            .push_git(request, &mut bundle.bytes.as_slice())
            .await
        {
            denied(&frame);
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_exact_method_and_wrong_generation_pins_are_denied() {
    timed!(async {
        // Delegation only grants install and status: start/stop are outside it even
        // though enrollment grants and scopes exist for them.
        let options = Options {
            delegation_methods: vec!["hub.install", "hub.status"],
            ..Options::default()
        };
        let mut env = Env::new(0x29, options).await.unwrap();
        let bundle = env.bundle(1, None, &[]);
        assert_eq!(env.push(&bundle).await["result"]["state"], "queued");
        let provisioned = Env::delegation_for(
            &env.coglet_key,
            &env.coglet,
            env.hub.id().to_string(),
            env.endpoint.id().to_string(),
            DELEGATION_GEN,
            DELEGATION_EXPIRES,
            &["hub.install", "hub.status"],
        );
        applied(&env.install(&bundle.signed, &provisioned).await);
        denied(&env.simple("hub.start").await);
        denied(&env.simple("hub.stop").await);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
    });

    for (seed, options, name) in [
        (
            0x2a,
            Options {
                pin_delegation_generation: DELEGATION_GEN + 1,
                ..Options::default()
            },
            "delegation pin",
        ),
        (
            0x2b,
            Options {
                pin_grant_generation: GRANT_GEN + 1,
                ..Options::default()
            },
            "grant pin",
        ),
    ] {
        timed!(async {
            let mut env = Env::new(seed, options).await.unwrap();
            let bundle = env.bundle(1, None, &[]);
            // Push uses the grant pin: a wrong grant generation cannot even push.
            let request = env.request(
                "hub.git.push",
                json!({"update":bundle.signed,"size":bundle.bytes.len(),
                    "sha256":hex::encode(Sha256::digest(&bundle.bytes))}),
            );
            let pushed = env
                .session
                .push_git(request, &mut bundle.bytes.as_slice())
                .await;
            if name == "grant pin" {
                assert!(
                    pushed.map(|f| f["type"] == "error").unwrap_or(true),
                    "{name}"
                );
                return;
            }
            assert_eq!(pushed.unwrap()["result"]["state"], "queued");
            let delegation = env.delegation();
            denied(&env.install(&bundle.signed, &delegation).await);
            denied(&env.simple("hub.status").await);
            denied(&env.simple("hub.start").await);
        });
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unauthorized_peer_context_ticket_and_target_are_denied() {
    timed!(async {
        let mut env = Env::new(0x2c, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        let wrong_actor = format!("hub:{}", key(0x6a, 1).public());
        let other_ticket = "cd".repeat(16);
        let contexts = [
            ("wrong actor", wrong_actor.clone(), Some(env.ticket.clone())),
            ("wrong ticket", env.actor.clone(), Some(other_ticket)),
            ("missing ticket", env.actor.clone(), None),
        ];
        for (name, actor, ticket) in contexts {
            for method in ["hub.status", "hub.start", "hub.stop"] {
                let request =
                    env.request_with(method, correlation(4000), json!({}), &actor, ticket.clone());
                denied(&env.call(request).await);
                let _ = name;
            }
            let request = env.request_with(
                "hub.install",
                correlation(4001),
                json!({"initial":bundle.signed,"delegation":env.delegation()}),
                &actor,
                ticket.clone(),
            );
            let reply = env.call(request).await;
            assert_eq!(error_code(&reply), "permission_denied", "{name}");
        }
        // Unpinned target.
        let mut request = env.request("hub.status", json!({}));
        request["target"] = json!(format!("coglet:{}", key(0x6b, 3).public()));
        denied(&env.call(request).await);
        // The authorised context still works, so every denial was specific.
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");

        // A different router identity is not the pinned hub, even with a valid
        // iroh session and the correct-looking context.
        let rogue = MockHub::with_clock(key(0x6c, 1), Arc::new(|| Some(T0)))
            .await
            .unwrap();
        let (_device_session, rogue_session, job) =
            attach(&rogue, &env.endpoint, env.handler.as_ref().unwrap())
                .await
                .unwrap();
        let reply = rogue_session
            .request(env.request_with(
                "hub.status",
                correlation(4002),
                json!({}),
                &env.actor,
                Some(env.ticket.clone()),
            ))
            .await
            .unwrap();
        denied(&reply);
        let reply = rogue_session
            .request(env.request_with(
                "hub.stop",
                correlation(4003),
                json!({}),
                &env.actor,
                Some(env.ticket.clone()),
            ))
            .await
            .unwrap();
        denied(&reply);
        let rogue_bundle = env.bundle(2, Some(&bundle.tip), &[("rogue.txt", b"x")]);
        let push = env.request(
            "hub.git.push",
            json!({"update":rogue_bundle.signed,"size":rogue_bundle.bytes.len(),
                "sha256":hex::encode(Sha256::digest(&rogue_bundle.bytes))}),
        );
        if let Ok(frame) = rogue_session
            .push_git(push, &mut rogue_bundle.bytes.as_slice())
            .await
        {
            denied(&frame);
        }
        job.abort();
        rogue.shutdown().await;
        // Nothing changed for the real router.
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn install_without_accepted_push_is_refused() {
    timed!(async {
        let mut env = Env::new(0x2d, Options::default()).await.unwrap();
        // Perfectly signed ref and delegation, but no push was ever accepted.
        let never_pushed = env.bundle(1, None, &[]);
        let delegation = env.delegation();
        denied(&env.install(&never_pushed.signed, &delegation).await);
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), Value::Null)
        );
        denied(&env.simple("hub.start").await);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_refuses_when_accepted_ref_advances_past_installed_commit() {
    timed!(async {
        let mut env = Env::new(0x2e, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        // Control: the installed commit is readable through status.
        assert_eq!(state(&env.simple("hub.status").await).1, json!(bundle.tip));
        if NATIVE {
            applied(&env.simple("hub.start").await);
            assert_eq!(state(&env.simple("hub.status").await).0, "running");
        }
        // The router later pushes a newer accepted commit; start must not
        // resurrect either the old or the new commit. Status is independent of the
        // newest pushed bytes and still reports the original installed identity.
        let next = env.bundle(2, Some(&bundle.tip), &[("next.txt", b"n")]);
        assert_eq!(env.push(&next).await["result"]["state"], "failed");
        denied(&env.simple("hub.start").await);
        assert_eq!(
            state(&env.simple("hub.status").await),
            (
                if NATIVE { "running" } else { "stopped" }.into(),
                json!(bundle.tip)
            )
        );
        let conflict = env.install_request(correlation(5000), &next.signed).await;
        assert_eq!(error_code(&conflict), "conflict", "{conflict}");
        // Stop is a safety operation: an accepted replacement must not prevent it.
        applied(&env.simple("hub.stop").await);
        let shared = env
            .handler
            .as_ref()
            .unwrap()
            .share_service(&env.coglet)
            .unwrap();
        assert_eq!(shared.lock().unwrap().managed_status()["state"], "stopped");
        // This must also work with no in-memory service after reopening the WAL.
        drop(shared);
        env.restart_handler().await;
        applied(&env.simple("hub.stop").await);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_refuses_tampered_retained_bundle_bytes() {
    timed!(async {
        let mut env = Env::new(0x2f, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        assert_eq!(state(&env.simple("hub.status").await).1, json!(bundle.tip));
        // Corrupt the durable retained bytes in the device's own Authority DB (a
        // local-disk attacker, not a remote capability).
        let sql = "UPDATE proto_ref_audit SET bundle = CAST(substr(bundle,1,32) || X'FF' || substr(bundle,34) AS BLOB);";
        let out = Command::new("sqlite3")
            .arg(env.root.join("authority.sqlite"))
            .arg(".timeout 5000")
            .arg(sql)
            .output()
            .expect("sqlite3 CLI is required to tamper retained bytes");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let started = env.simple("hub.start").await;
        assert_eq!(started["type"], "error", "{started}");
        assert_ne!(started["error"]["code"], "applied");
        // Status reads only the original private WAL setup, so tampered retained
        // bytes do not hide the installed identity.
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        assert_eq!(env.simple("hub.start").await["type"], "error");
        // After a restart the corrupt bytes still cannot be revalidated.
        env.restart_handler().await;
        assert_eq!(env.simple("hub.start").await["type"], "error");
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_refuses_after_authority_loses_the_accepted_ref() {
    timed!(async {
        let mut env = Env::new(0x30, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        applied(&env.simple("hub.stop").await);
        // The WAL says installed, but the Authority DB no longer has an accepted
        // ref (e.g. replaced/restored): start must not trust the WAL alone.
        let out = Command::new("sqlite3")
            .arg(env.root.join("authority.sqlite"))
            .arg(".timeout 5000")
            .arg("DELETE FROM proto_accepted_refs;")
            .output()
            .expect("sqlite3 CLI is required");
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        denied(&env.simple("hub.start").await);
        // Status needs exact current local authority, not the accepted ref.
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
    });
}

/// Take the real service mutex exactly when the request samples the clock for
/// its in-callback gate (after runtime_boundary's two samples, i.e. after the
/// full initial authority check and while the SQL guard is held). The handler
/// then blocks on the service mutex; advance the trusted clock to `expiry` and
/// release. Returns the reply.
async fn expire_while_service_locked(env: &mut Env, request: Value, expiry: u64) -> Value {
    let shared = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .expect("installed service");
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = Arc::new(Mutex::new(None));
    let holder_slot = holder.clone();
    let target = env.reads.load(Ordering::SeqCst) + 3;
    *env.trap.lock().unwrap() = Some((
        target,
        Box::new(move || {
            let (locked_tx, locked_rx) = std::sync::mpsc::channel();
            *holder_slot.lock().unwrap() = Some(std::thread::spawn(move || {
                let guard = shared.lock().unwrap();
                locked_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                drop(guard);
            }));
            locked_rx.recv().unwrap();
        }),
    ));
    let session = env.session.clone();
    let call = tokio::spawn(async move { session.request(request).await });
    for _ in 0..4000 {
        if holder.lock().unwrap().is_some() && env.reads.load(Ordering::SeqCst) >= target {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(
        holder.lock().unwrap().is_some() && env.reads.load(Ordering::SeqCst) >= target,
        "request never reached the in-callback gate"
    );
    // Give the authorized request time to reach its service-mutex wait; the
    // outcome does not depend on this (any later acquire is rechecked).
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!call.is_finished(), "request must be blocked on the mutex");
    env.clock.store(expiry, Ordering::SeqCst);
    release_tx.send(()).unwrap();
    holder.lock().unwrap().take().unwrap().join().unwrap();
    let reply = call.await.unwrap().expect("terminal frame");
    env.clock.store(T0, Ordering::SeqCst);
    reply
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn delegation_expiring_while_waiting_for_service_mutex_denies_every_method() {
    timed!(async {
        let mut env = Env::new(0x32, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        for method in ["hub.start", "hub.status", "hub.install", "hub.stop"] {
            // A running coglet makes a wrongly applied stop observable.
            if NATIVE && method == "hub.stop" {
                applied(&env.simple("hub.start").await);
            }
            let shared = env
                .handler
                .as_ref()
                .unwrap()
                .share_service(&env.coglet)
                .unwrap();
            let before = shared.lock().unwrap().managed_status();
            let request = if method == "hub.install" {
                let delegation = env.delegation();
                env.request(
                    method,
                    json!({"initial":bundle.signed,"delegation":delegation}),
                )
            } else {
                env.request(method, json!({}))
            };
            // Exactly the delegation expiry: grants/scopes are still valid, so
            // only the signed delegation window can deny.
            let reply = expire_while_service_locked(&mut env, request, DELEGATION_EXPIRES).await;
            denied(&reply);
            assert_eq!(shared.lock().unwrap().managed_status(), before, "{method}");
        }
        assert_eq!(
            state(&env.simple("hub.status").await),
            (
                if NATIVE { "running" } else { "stopped" }.into(),
                json!(bundle.tip)
            )
        );
        applied(&env.simple("hub.stop").await);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn start_requires_present_well_formed_clear_kill_switch() {
    timed!(async {
        let mut env = Env::new(0x31, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        let kill = env.root.join("kill.json");
        for (contents, expected) in [
            (Some("{\"latched\":true}"), "permission_denied"),
            (None, "unavailable"),
            (Some("not json"), "unavailable"),
            (Some("{}"), "unavailable"),
            (Some("{\"latched\":\"false\"}"), "unavailable"),
            (Some("{\"latched\":false,\"extra\":true}"), "unavailable"),
        ] {
            if let Some(contents) = contents {
                std::fs::write(&kill, contents).unwrap();
            } else {
                std::fs::remove_file(&kill).unwrap();
            }
            let reply = env.simple("hub.start").await;
            assert_eq!(error_code(&reply), expected, "{contents:?}: {reply}");
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(bundle.tip))
            );
            applied(&env.simple("hub.stop").await);
        }
        // An absent configuration path also fails closed, independently of file IO.
        env.config.kill_switch = None;
        env.restart_handler().await;
        assert_eq!(error_code(&env.simple("hub.start").await), "unavailable");
        env.config.kill_switch = Some(kill.clone());
        std::fs::write(kill, b"{\"latched\":false}").unwrap();
        env.restart_handler().await;
        let start = env.simple("hub.start").await;
        if NATIVE {
            applied(&start);
        } else {
            assert_eq!(error_code(&start), "unavailable");
        }
        applied(&env.simple("hub.stop").await);
    });
}

/// Explicit local scheduler invocation, NOT an adapter-socket execution claim.
/// The service and code come from the authenticated handler's real installation.
#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handler_installed_native_guest_commits_a_real_step_and_journal() {
    timed!(async {
        let mut env = Env::new(0x36, Options::default()).await.unwrap();
        let (bundle, _) = pushed_and_installed(&mut env).await;
        applied(&env.simple("hub.start").await);
        let shared = env
            .handler
            .as_ref()
            .unwrap()
            .share_service(&env.coglet)
            .unwrap();
        let journal = {
            let mut service = shared.lock().unwrap();
            let setup = service.managed_setup().clone();
            let code = hex::decode(
                setup["initial_files"]["/code/bin/step.wasm"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(code, wat::parse_str(WASM_WAT).unwrap());
            let cases: Value =
                serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
            let mut inputs = cases
                .as_array()
                .unwrap()
                .iter()
                .find(|c| c["name"] == "no-effects")
                .unwrap()["steps"][0]["send"]["inputs"]
                .clone();
            inputs["budgets"] = setup["manifest"]["budgets"].clone();
            inputs["grants"] = env.config.grants["operations"].clone();
            let event = inputs["events"][0].clone();
            coglet_runtime::validate("event", &event).unwrap();
            service.enqueue(event).unwrap();
            assert!(service.records().is_empty());
            let mut engine =
                coglet_runtime::NativeEngine::new(code, &inputs["budgets"], 8_388_608, 65_536)
                    .unwrap();
            let result = service
                .run_next(&inputs, &mut engine)
                .unwrap()
                .expect("queued event executes");
            assert_eq!(result["journal"]["status"], "committed", "{result}");
            assert_eq!(result["journal"]["step"], inputs["step"]);
            assert_eq!(result["journal"]["coglet"], env.coglet);
            assert_eq!(
                result["journal"]["engine"]["policy"],
                coglet_engine_wamr::COMPUTE_MODE
            );
            assert!(result["journal"]["usage"]["compute"].as_u64().unwrap() > 0);
            assert_eq!(service.records(), &[result["journal"].clone()]);
            assert_eq!(service.queued(), 0);
            result["journal"].clone()
        };
        applied(&env.simple("hub.stop").await);
        drop(shared);
        env.restart_handler().await;
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), json!(bundle.tip))
        );
        let recovered = env
            .handler
            .as_ref()
            .unwrap()
            .share_service(&env.coglet)
            .unwrap();
        assert_eq!(
            recovered.lock().unwrap().records(),
            std::slice::from_ref(&journal)
        );
        // Release all handler/service owners before opening the private WAL;
        // the runtime deliberately enforces a single writer even for this read.
        // Session::serve drops its per-stream task set on abort. Those aborts
        // release handler clones asynchronously; wait for actual coordinator
        // retirement rather than racing its single-writer lock.
        let retired = Arc::downgrade(&recovered);
        drop(recovered);
        env.serve.abort();
        let _ = (&mut env.serve).await;
        drop(env.handler.take());
        tokio::time::timeout(Duration::from_secs(5), async {
            while retired.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all Session request tasks release the coordinator");
        let wal = env
            .config
            .wal_root
            .join(coglet_runtime::digest(env.coglet.as_bytes()))
            .join("state.wal");
        // Weak upgrade fails when the last Arc begins Drop, before its nested
        // store destructor necessarily releases the OS flock. Observe the lock.
        let durable = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match coglet_runtime::DurableStore::open(&wal) {
                    Ok(store) => break store,
                    Err(e) if e.starts_with("WAL already owned:") => tokio::task::yield_now().await,
                    Err(e) => panic!("WAL recovery failed: {e}"),
                }
            }
        })
        .await
        .expect("retired coordinator releases WAL lock");
        assert_eq!(durable.records(), &[journal]);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn first_install_expiry_leaves_no_code_identity_and_new_ref_retries_cleanly() {
    timed!(async {
        for (seed, restart) in [(0x34, false), (0x35, true)] {
            let mut env = Env::new(seed, Options::default()).await.unwrap();
            let first = env.bundle(1, None, &[("marker.txt", b"old")]);
            assert_eq!(env.push(&first).await["result"]["state"], "queued");
            // Fourth sample: immediately before the first managed WAL frame,
            // after construction. The first three authorize and enter callback.
            let clock = env.clock.clone();
            *env.trap.lock().unwrap() = Some((
                env.reads.load(Ordering::SeqCst) + 4,
                Box::new(move || clock.store(DELEGATION_EXPIRES, Ordering::SeqCst)),
            ));
            let rejected = env.install_request(correlation(6000), &first.signed).await;
            denied(&rejected);
            assert!(
                env.trap.lock().unwrap().is_none(),
                "precommit gate executed"
            );
            assert!(
                env.handler
                    .as_ref()
                    .unwrap()
                    .share_service(&env.coglet)
                    .is_none()
            );
            let wal = env
                .config
                .wal_root
                .join(coglet_runtime::digest(env.coglet.as_bytes()))
                .join("state.wal");
            {
                let store = coglet_runtime::DurableStore::open(&wal).unwrap();
                assert!(store.state().is_none(), "no unmanaged initial code frame");
                assert!(store.records().is_empty());
            }
            env.clock.store(T0, Ordering::SeqCst);
            if restart {
                env.restart_handler().await;
            }
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), Value::Null)
            );
            let next = env.bundle(2, Some(&first.tip), &[("marker.txt", b"new")]);
            assert_eq!(env.push(&next).await["result"]["state"], "queued");
            let receipt = env.install_request(correlation(6001), &next.signed).await;
            applied(&receipt);
            let shared = env
                .handler
                .as_ref()
                .unwrap()
                .share_service(&env.coglet)
                .unwrap();
            assert_eq!(
                shared.lock().unwrap().managed_setup()["initial_files"]["/code/marker.txt"],
                hex::encode(b"new")
            );
            drop(shared);
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(next.tip))
            );
            assert_eq!(
                env.install_request(correlation(6002), &next.signed).await["result"],
                receipt["result"]
            );
        }
    });
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn lifecycle_operation_retries_cannot_reverse_later_transitions_after_restart() {
    timed!(async {
        let mut env = Env::new(0x37, Options::default()).await.unwrap();
        pushed_and_installed(&mut env).await;
        let start = env.request("hub.start", json!({}));
        let started = env.call(start.clone()).await;
        applied(&started);
        let stop = env.request("hub.stop", json!({}));
        let stopped = env.call(stop.clone()).await;
        applied(&stopped);
        assert_eq!(env.call(start.clone()).await["result"], started["result"]);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        env.restart_handler().await;
        let mut timeout_retry = start.clone();
        timeout_retry["timeout_ms"] = json!(299999);
        assert_eq!(env.call(timeout_retry).await["result"], started["result"]);
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        let mut reused = start;
        reused["method"] = json!("hub.stop");
        assert_eq!(error_code(&env.call(reused).await), "conflict");
        applied(&env.simple("hub.start").await);
        env.restart_handler().await;
        assert_eq!(env.call(stop).await["result"], stopped["result"]);
        assert_eq!(state(&env.simple("hub.status").await).0, "running");
        applied(&env.simple("hub.stop").await);
    });
}

/// Own every subprocess immediately: assertions/timeouts also SIGKILL and reap.
struct ChildGuard(std::process::Child);
impl ChildGuard {
    fn spawn(config: &Path, log: &Path) -> Self {
        let stderr = std::fs::File::create(log).unwrap();
        Self(
            Command::new(env!("CARGO_BIN_EXE_cogletd"))
                .arg("--router-service")
                .arg(config)
                .env("TOKIO_WORKER_THREADS", "2")
                .stdout(std::process::Stdio::null())
                .stderr(stderr)
                .spawn()
                .unwrap(),
        )
    }
    fn kill_and_wait(&mut self) {
        self.0.kill().unwrap();
        let status = self.0.wait().unwrap();
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(status.signal(), Some(libc::SIGKILL));
    }
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

async fn child_session(
    hub: &MockHub,
    device: iroh::EndpointId,
    child: &mut ChildGuard,
    log: &Path,
) -> Session {
    for _ in 0..400 {
        if let Some(status) = child.0.try_wait().unwrap() {
            panic!(
                "cogletd exited {status}: {}",
                std::fs::read_to_string(log).unwrap()
            );
        }
        if let Some(session) = hub.session(device).await {
            // Hub registration precedes enrollment; requests queue until the child
            // starts its handler. A completed lifecycle request proves readiness.
            return session;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!(
        "cogletd never connected: {}",
        std::fs::read_to_string(log).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cogletd_subprocess_enrolls_installs_starts_stops_and_replays_after_sigkill() {
    timed!(async {
        use std::os::unix::fs::PermissionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};
        let mut env = Env::new(0x33, Options::default()).await.unwrap();
        // A distinct device ensures the child's first enrollment consumes a real
        // invite; the in-process fixture device never provisions this hub peer.
        let device_key = key(0x33, 4);
        let device = device_key.public();
        let root = env.root.join("child");
        std::fs::create_dir(&root).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires = now.max(T0) + 100_000;
        let delegation = Env::delegation_for(
            &env.coglet_key,
            &env.coglet,
            env.hub.id().to_string(),
            device.to_string(),
            DELEGATION_GEN,
            expires,
            &Options::default().delegation_methods,
        );
        let authority_path = root.join("authority.sqlite");
        {
            let authority = Authority::open(&authority_path).unwrap();
            authority.open_window(Duration::from_secs(60)).unwrap();
            let grants: Vec<_> = METHODS
                .iter()
                .map(|method| Grant {
                    method: (*method).into(),
                    target: Some(env.coglet.clone()),
                    expires,
                    generation: GRANT_GEN,
                })
                .collect();
            let invite = authority
                .issue_invite(env.hub.id(), Duration::from_secs(60), grants.clone())
                .unwrap();
            authority.enroll(env.hub.id(), Some(&invite)).unwrap();
            let scope = Scope {
                actor: env.actor.clone(),
                ticket: Some(env.ticket.clone()),
            };
            for grant in &grants {
                authority
                    .provision_scope(env.hub.id(), &scope, grant)
                    .unwrap();
            }
            authority
                .accept_delegation(env.hub.id(), device, &env.coglet, &delegation, || Some(now))
                .unwrap();
        }
        let identity = root.join("identity");
        // Create privately, not write-then-chmod, so no transient public key file.
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&identity)
            .unwrap()
            .write_all(&device_key.to_bytes())
            .unwrap();
        assert_eq!(
            std::fs::metadata(&identity).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let invite = env
            .hub
            .invite(
                device,
                Duration::from_secs(60),
                vec![Grant {
                    method: "hub.health".into(),
                    target: None,
                    expires,
                    generation: 0,
                }],
            )
            .await
            .unwrap();
        let addr = *env.hub.addr().ip_addrs().next().unwrap();
        let config_path = root.join("router.json");
        let mut config = json!({"hub":env.hub.id().to_string(),"device":device.to_string(),
            "identity_path":identity,"authority_path":authority_path,"validator_path":validator_path(),
            "wal_root":root.join("wal"),"quarantine":root.join("quarantine"),
            "catalog":env.config.catalog,"grants":env.config.grants,"targets":{(env.coglet.clone()):{
                "grant_generation":GRANT_GEN,"delegation_generation":DELEGATION_GEN,
                "native_schedule":if NATIVE { json!({"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8_388_608,"stack_bytes":65_536}) } else { Value::Null }}},
            "kill_switch":env.root.join("kill.json"),"hub_addr":addr,"invite":invite});
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let log = root.join("child.log");
        let mut child = ChildGuard::spawn(&config_path, &log);
        env.session = child_session(&env.hub, device, &mut child, &log).await;
        assert_eq!(
            state(&env.simple("hub.status").await),
            ("stopped".into(), Value::Null)
        );
        let bundle = env.bundle(1, None, &[]);
        assert_eq!(env.push(&bundle).await["result"]["state"], "queued");
        let receipt = env.install(&bundle.signed, &delegation).await;
        applied(&receipt);
        let start = env.simple("hub.start").await;
        if NATIVE {
            applied(&start);
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("running".into(), json!(bundle.tip))
            );
        } else {
            assert_eq!(error_code(&start), "unavailable");
            assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        }
        applied(&env.simple("hub.stop").await);
        let stopped = env.simple("hub.status").await["result"].clone();
        assert_eq!(stopped["state"], "stopped");
        let wal = root
            .join("wal")
            .join(coglet_runtime::digest(env.coglet.as_bytes()))
            .join("state.wal");
        assert!(wal.is_file(), "child must create its own real WAL");
        let before = std::fs::read(&wal).unwrap();
        child.kill_and_wait();
        // Wait for the old authenticated transport to disappear before observing
        // a replacement session, rather than accidentally reusing the stale one.
        // QUIC cannot send a graceful close after SIGKILL; allow its bounded
        // idle timeout to retire the old connection (not just process exit).
        for _ in 0..2000 {
            if env.hub.session(device).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(env.hub.session(device).await.is_none());
        config["invite"] = Value::Null; // recover already committed enrollment
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let mut restarted = ChildGuard::spawn(&config_path, &root.join("restart.log"));
        env.session =
            child_session(&env.hub, device, &mut restarted, &root.join("restart.log")).await;
        assert_eq!(env.simple("hub.status").await["result"], stopped);
        assert_eq!(
            std::fs::read(&wal).unwrap(),
            before,
            "recovery must not replace/reinitialize WAL"
        );
        // Exact same correlation and a new correlation both recover the original
        // install receipt from the original child's persisted state.
        let replay = env.request_with(
            "hub.install",
            receipt["correlation_id"].as_str().unwrap().into(),
            json!({"initial":bundle.signed,"delegation":delegation}),
            &env.actor,
            Some(env.ticket.clone()),
        );
        assert_eq!(env.call(replay).await, receipt);
        let replay = env.install(&bundle.signed, &delegation).await;
        assert_eq!(replay["result"], receipt["result"]);
        applied(&env.simple("hub.stop").await);
        restarted.kill_and_wait();
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_preinstall_original_push_completes_before_install() {
    timed!(async {
        let mut env = Env::new(0x51, Options::receipts()).await.unwrap();
        let bundle = env.bundle(1, None, &[]);
        let pushed = env.push(&bundle).await;
        let id = operation_id(&pushed);
        let mut terminal = Value::Null;
        for _ in 0..200 {
            let receipt = receipt_of(&mut env, &id).await;
            let found = result_of(&receipt);
            assert_eq!(found["operation_id"], id);
            if found["state"] == "applied" {
                terminal = found;
                break;
            }
            assert_eq!(found["state"], "queued", "{receipt}");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            terminal["state"], "applied",
            "original preinstall push never completed"
        );
        let status = env.simple("hub.status").await;
        assert_eq!(status["result"]["live_commit"], Value::Null);
        assert_eq!(status["result"]["step"], 0);
        assert_eq!(terminal["step"], 0);
        assert!(terminal.get("generation").is_none());
        assert_eq!(result_of(&receipt_of(&mut env, &id).await), terminal);
        let delegation = env.delegation();
        applied(&env.install(&bundle.signed, &delegation).await);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_original_push_retry_retains_single_boundary() {
    timed!(async {
        let mut env = Env::new(0x52, Options::receipts()).await.unwrap();
        let bundle = env.bundle(1, None, &[]);
        let request = env.request(
            "hub.git.push",
            json!({
                "update":bundle.signed,"size":bundle.bytes.len(),
                "sha256":hex::encode(Sha256::digest(&bundle.bytes))
            }),
        );
        let first = env
            .session
            .push_git(request.clone(), &mut bundle.bytes.as_slice())
            .await
            .unwrap();
        let id = operation_id(&first);
        let terminal = result_of(&receipt_of(&mut env, &id).await);
        assert_eq!(terminal["state"], "applied");
        for restart in [false, true] {
            if restart {
                env.restart_handler().await;
            }
            let retry = env
                .session
                .push_git(request.clone(), &mut bundle.bytes.as_slice())
                .await
                .unwrap();
            assert_eq!(result_of(&retry), terminal);
            let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
            for table in [
                "proto_ref_audit",
                "proto_git_operations",
                "proto_git_intents",
                "proto_preinstall_boundaries",
            ] {
                let count: i64 = db
                    .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 1, "{table}");
            }
            assert_eq!(
                env.simple("hub.status").await["result"]["live_commit"],
                Value::Null
            );
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_constructor_refuses_missing_recovery_runtime() {
    timed!(async {
        let env = Env::new(0x53, Options::receipts()).await.unwrap();
        let config = env.config.clone();
        let authority = Authority::open(env.root.join("authority.sqlite")).unwrap();
        let error = std::thread::spawn(move || {
            DeviceHandler::new(config, authority, validator_path(), Arc::new(|| Some(T0)))
                .err()
                .expect("startup must not silently skip durable recovery")
        })
        .join()
        .unwrap();
        assert_eq!(error, "device recovery requires an active Tokio runtime");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_pending_recovery_selects_current_original_intent() {
    timed!(async {
        let mut env = Env::new(0x54, Options::receipts()).await.unwrap();
        let (installed, _) = pushed_and_installed(&mut env).await;
        let second = env.bundle(2, Some(&installed.tip), &[("v", b"2")]);
        let queued_second = env.push(&second).await;
        let third = env.bundle(3, Some(&second.tip), &[("v", b"3")]);
        let queued_third = env.push(&third).await;
        env.restart_handler().await;
        let store = coglet_proto::authority::accepted_refs::AcceptedRefStore::new(
            Authority::open(env.root.join("authority.sqlite")).unwrap(),
        )
        .unwrap();
        let pending = store.pending_pushes(env.hub.id(), &env.coglet).unwrap();
        assert!(pending.is_empty());
        // Each installed intent lacks the delegated push method. The installed
        // boundary now resolves it to an original-ID failure instead of leaving
        // the initial-deploy selector to keep it queued indefinitely.
        for original in [&queued_second, &queued_third] {
            let receipt = receipt_of(&mut env, &operation_id(original)).await;
            assert_eq!(receipt["result"]["state"], "failed");
            assert_eq!(receipt["result"]["error"]["code"], "permission_denied");
        }
    });
}

// ---- hub.receipt: original-scope operation receipts -------------------------

/// Actual process death with only a durable queued push present. This proves
/// queued recovery, not ref activation: install has a separate terminal receipt.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_cogletd_sigkill_recovers_original_scope_and_no_duplicate_install() {
    original_push_daemon_crash(false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_push_crash_reconciles_exactly_once() {
    original_push_daemon_crash(true).await;
}

async fn original_push_daemon_crash(installed_target: bool) {
    timed!(async {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        use std::time::{SystemTime, UNIX_EPOCH};
        let seed = if installed_target { 0x5a } else { 0x4a };
        let mut options = Options::receipts();
        if installed_target {
            options.delegation_methods.push("hub.git.push");
        }
        let mut env = Env::new(seed, options.clone()).await.unwrap();
        let device_key = key(seed, 4);
        let device = device_key.public();
        let root = env.root.join("receipt-child");
        std::fs::create_dir(&root).unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let expires = now.max(T0) + 100_000;
        let delegation = Env::delegation_for(
            &env.coglet_key,
            &env.coglet,
            env.hub.id().to_string(),
            device.to_string(),
            DELEGATION_GEN,
            expires,
            &options.delegation_methods,
        );
        let authority_path = root.join("authority.sqlite");
        {
            let authority = Authority::open(&authority_path).unwrap();
            authority.open_window(Duration::from_secs(60)).unwrap();
            let grants: Vec<_> = METHODS
                .iter()
                .map(|method| Grant {
                    method: (*method).into(),
                    target: Some(env.coglet.clone()),
                    expires,
                    generation: GRANT_GEN,
                })
                .collect();
            let invite = authority
                .issue_invite(env.hub.id(), Duration::from_secs(60), grants.clone())
                .unwrap();
            authority.enroll(env.hub.id(), Some(&invite)).unwrap();
            // Both tickets are authorized, but only the original may read a
            // receipt. This is not a missing-grant negative control.
            for ticket in [&env.ticket, &OTHER_TICKET.to_owned()] {
                let scope = Scope {
                    actor: env.actor.clone(),
                    ticket: Some(ticket.clone()),
                };
                for grant in &grants {
                    authority
                        .provision_scope(env.hub.id(), &scope, grant)
                        .unwrap();
                }
            }
            authority
                .accept_delegation(env.hub.id(), device, &env.coglet, &delegation, || Some(now))
                .unwrap();
        }
        let identity = root.join("identity");
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&identity)
            .unwrap()
            .write_all(&device_key.to_bytes())
            .unwrap();
        let invite = env
            .hub
            .invite(
                device,
                Duration::from_secs(60),
                vec![Grant {
                    method: "hub.health".into(),
                    target: None,
                    expires,
                    generation: 0,
                }],
            )
            .await
            .unwrap();
        let addr = *env.hub.addr().ip_addrs().next().unwrap();
        let config_path = root.join("router.json");
        let mut config = json!({"hub":env.hub.id().to_string(),"device":device.to_string(),
            "identity_path":identity,"authority_path":authority_path,"validator_path":validator_path(),
            "wal_root":root.join("wal"),"quarantine":root.join("quarantine"),
            "catalog":env.config.catalog,"grants":env.config.grants,"targets":{(env.coglet.clone()):{
                "grant_generation":GRANT_GEN,"delegation_generation":DELEGATION_GEN,
                "native_schedule":if NATIVE { json!({"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8_388_608,"stack_bytes":65_536}) } else { Value::Null }}},
            "kill_switch":env.root.join("kill.json"),"hub_addr":addr,"invite":invite});
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        use std::os::unix::fs::PermissionsExt;
        let wrapper = root.join("validator-barrier");
        let first = root.join("first-validation");
        let entered = root.join("boundary-validation");
        let release = root.join("release-boundary");
        std::fs::write(&wrapper, format!(
            "#!/bin/sh\nif [ -f '{}' ]; then\n : > '{}'\n while [ ! -f '{}' ]; do /bin/sleep 0.01; done\nelse\n : > '{}'\nfi\nexec '{}' \"$@\"\n",
            first.display(), entered.display(), release.display(), first.display(), validator_path().display()
        )).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        config["validator_path"] = json!(wrapper);
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let log = root.join("child.log");
        let mut child = ChildGuard::spawn(&config_path, &log);
        env.session = child_session(&env.hub, device, &mut child, &log).await;
        assert_eq!(state(&env.simple("hub.status").await).0, "stopped");
        let bundle = if installed_target {
            std::fs::write(&release, b"initial verification may complete").unwrap();
            let initial = env.bundle(1, None, &[]);
            env.push(&initial).await;
            applied(&env.install(&initial.signed, &delegation).await);
            std::fs::remove_file(&release).unwrap();
            std::fs::remove_file(&first).unwrap();
            std::fs::remove_file(&entered).unwrap();
            env.bundle(2, Some(&initial.tip), &[("revision", b"two")])
        } else {
            env.bundle(1, None, &[])
        };
        let push_request = env.request("hub.git.push", json!({"update":bundle.signed,"size":bundle.bytes.len(),"sha256":hex::encode(Sha256::digest(&bundle.bytes))}));
        let push_id = push_request["correlation_id"].as_str().unwrap().to_owned();
        let session = env.session.clone();
        let bytes = bundle.bytes.clone();
        let push =
            tokio::spawn(
                async move { session.push_git(push_request, &mut bytes.as_slice()).await },
            );
        tokio::time::timeout(Duration::from_secs(15), async {
            while !entered.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second validation did not start after admission");
        let queued = result_of(&receipt_of(&mut env, &push_id).await);
        assert_eq!(queued["state"], "queued");
        assert_eq!(queued["step"], Value::Null);
        child.kill_and_wait();
        let _ = push.await;
        for _ in 0..2000 {
            if env.hub.session(device).await.is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert!(env.hub.session(device).await.is_none());
        config["invite"] = Value::Null;
        std::fs::write(&config_path, serde_json::to_vec(&config).unwrap()).unwrap();
        let restart_log = root.join("restart.log");
        let mut restarted = ChildGuard::spawn(&config_path, &restart_log);
        env.session = child_session(&env.hub, device, &mut restarted, &restart_log).await;
        assert_eq!(result_of(&receipt_of(&mut env, &push_id).await), queued);
        if !installed_target {
            denied(&env.install(&bundle.signed, &delegation).await);
        }
        if installed_target {
            // Force the exact WAL -> SQL-mirror crash window without a daemon
            // fault switch. The WAL may confirm the effect but SQL stays queued.
            rusqlite::Connection::open(&authority_path)
                .unwrap()
                .execute_batch(
                    "CREATE TRIGGER test_hold_terminal BEFORE UPDATE ON proto_git_operations
                 WHEN json_extract(NEW.receipt,'$.state')='applied'
                 BEGIN SELECT RAISE(ABORT,'test mirror interruption'); END;",
                )
                .unwrap();
        }
        std::fs::write(&release, b"release").unwrap();
        if installed_target {
            tokio::time::timeout(Duration::from_secs(15), async {
                loop {
                    if env.simple("hub.status").await["result"]["live_commit"] == bundle.tip {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("runtime WAL completion before SQL mirror");
            assert_eq!(result_of(&receipt_of(&mut env, &push_id).await), queued);
            restarted.kill_and_wait();
            for _ in 0..2000 {
                if env.hub.session(device).await.is_none() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            assert!(env.hub.session(device).await.is_none());
            rusqlite::Connection::open(&authority_path)
                .unwrap()
                .execute_batch("DROP TRIGGER test_hold_terminal")
                .unwrap();
            let mirror_log = root.join("mirror-restart.log");
            restarted = ChildGuard::spawn(&config_path, &mirror_log);
            env.session = child_session(&env.hub, device, &mut restarted, &mirror_log).await;
        }
        let terminal = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let receipt = result_of(&receipt_of(&mut env, &push_id).await);
                if receipt["state"] == "applied" {
                    break receipt;
                }
                assert_eq!(receipt["state"], "queued");
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(terminal["operation_id"], push_id);
        assert_eq!(terminal["step"], 0);
        assert_eq!(
            env.simple("hub.status").await["result"]["live_commit"],
            if installed_target {
                json!(bundle.tip)
            } else {
                Value::Null
            }
        );
        let db = rusqlite::Connection::open(&authority_path).unwrap();
        assert_eq!(
            db.query_row(
                "SELECT count(*) FROM proto_preinstall_boundaries",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        let actor = env.actor.clone();
        let denied = receipt_as(&mut env, &push_id, &actor, Some(OTHER_TICKET)).await;
        let unknown = receipt_as(&mut env, &correlation(424_250), &actor, Some(OTHER_TICKET)).await;
        assert_eq!(error_code(&denied), "not_found", "{denied}");
        assert_eq!(error_of(&denied), error_of(&unknown));
        if installed_target {
            let wal = root
                .join("wal")
                .join(coglet_runtime::digest(env.coglet.as_bytes()))
                .join("state.wal");
            let before = std::fs::read(&wal).unwrap();
            let retry = env.request_with(
                "hub.git.push",
                push_id.clone(),
                json!({"update":bundle.signed,
                "size":bundle.bytes.len(),"sha256":hex::encode(Sha256::digest(&bundle.bytes))}),
                &env.actor,
                Some(env.ticket.clone()),
            );
            let retried = env
                .session
                .push_git(retry, &mut bundle.bytes.as_slice())
                .await
                .unwrap();
            assert_eq!(result_of(&retried), terminal);
            assert_eq!(
                std::fs::read(&wal).unwrap(),
                before,
                "confirmed update must not append another WAL effect"
            );
            restarted.kill_and_wait();
            let saved = coglet_runtime::DurableStore::open(&wal).unwrap();
            let state = saved.state().unwrap();
            assert_eq!(
                state["completed_segments"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| !s["code_revision"].is_null())
                    .count(),
                1
            );
            assert_eq!(
                state["managed"]["operations"][&push_id]["receipt"],
                terminal
            );
            return;
        }
        let installed = env.install(&bundle.signed, &delegation).await;
        applied(&installed);
        let id = operation_id(&installed);
        assert_eq!(
            result_of(&receipt_of(&mut env, &id).await),
            installed["result"]
        );
        let wal = root
            .join("wal")
            .join(coglet_runtime::digest(env.coglet.as_bytes()))
            .join("state.wal");
        let before = std::fs::read(&wal).unwrap();
        let retry = env.request_with(
            "hub.install",
            id,
            json!({"initial":bundle.signed,"delegation":delegation}),
            &env.actor,
            Some(env.ticket.clone()),
        );
        assert_eq!(env.call(retry).await["result"], installed["result"]);
        assert_eq!(
            std::fs::read(&wal).unwrap(),
            before,
            "retry must not append another effect"
        );
        // Install and retry cannot change the original terminal push.
        assert_eq!(result_of(&receipt_of(&mut env, &push_id).await), terminal);
        restarted.kill_and_wait();
    });
}

// Test-local validator seam: first pass admits, only the second pass blocks.
// Later validations remain usable to test a superseding original operation.
struct InitialBoundaryBarrier {
    entered: PathBuf,
    release: PathBuf,
}
impl Drop for InitialBoundaryBarrier {
    fn drop(&mut self) {
        let _ = std::fs::write(&self.release, b"release");
    }
}
impl InitialBoundaryBarrier {
    async fn install(env: &mut Env) -> Self {
        use std::os::unix::fs::PermissionsExt;
        env.serve.abort();
        let _ = (&mut env.serve).await;
        drop(env.handler.take());
        let wrapper = env.root.join("initial-boundary-validator");
        let first = env.root.join("validator-first");
        let entered = env.root.join("validator-second");
        let release = env.root.join("validator-release");
        std::fs::write(&wrapper, format!(
            "#!/bin/sh\nif /bin/mkdir '{}' 2>/dev/null; then :; elif /bin/mkdir '{}' 2>/dev/null; then while [ ! -f '{}' ]; do /bin/sleep 0.01; done; fi\nexec '{}' \"$@\"\n",
            first.display(), entered.display(), release.display(), validator_path().display()
        )).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let handler = DeviceHandler::new(
            env.config.clone(),
            Authority::open(env.root.join("authority.sqlite")).unwrap(),
            &wrapper,
            clock_fn(&env.clock, &env.reads, &env.trap),
        )
        .unwrap();
        env.serve = tokio::spawn(serve(env.device_session.clone(), handler.clone()));
        env.handler = Some(handler);
        Self { entered, release }
    }
    async fn wait(&self) {
        tokio::time::timeout(Duration::from_secs(15), async {
            while !self.entered.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second validation must follow durable admission");
    }
    fn release(&self) {
        std::fs::write(&self.release, b"release").unwrap();
    }
}

fn begin_push(env: &mut Env, bundle: &Bundle) -> (String, JoinHandle<Value>) {
    begin_push_with_timeout(env, bundle, 30_000)
}

fn begin_push_with_timeout(
    env: &mut Env,
    bundle: &Bundle,
    timeout_ms: u64,
) -> (String, JoinHandle<Value>) {
    let mut request = env.request(
        "hub.git.push",
        json!({"update":bundle.signed,
        "size":bundle.bytes.len(),"sha256":hex::encode(Sha256::digest(&bundle.bytes))}),
    );
    request["timeout_ms"] = json!(timeout_ms);
    let id = request["correlation_id"].as_str().unwrap().to_owned();
    let session = env.session.clone();
    let bytes = bundle.bytes.clone();
    (
        id,
        tokio::spawn(async move {
            session
                .push_git(request, &mut bytes.as_slice())
                .await
                .unwrap()
        }),
    )
}

async fn wait_applied(env: &mut Env, id: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let receipt = result_of(&receipt_of(env, id).await);
            if receipt["state"] == "applied" {
                break receipt;
            }
            assert_eq!(receipt["state"], "queued");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_preinstall_rechecks_authority_after_queued_admission() {
    timed!(async {
        // Expired signed delegation, revoked delegation, and a new signed
        // delegation that disagrees with the device's pinned generation.
        for mode in 0..3u8 {
            let mut env = Env::new(0x70 + mode, Options::receipts()).await.unwrap();
            let barrier = InitialBoundaryBarrier::install(&mut env).await;
            let bundle = env.bundle(1, None, &[]);
            let (id, push) = begin_push(&mut env, &bundle);
            barrier.wait().await;
            assert_eq!(
                result_of(&receipt_of(&mut env, &id).await)["state"],
                "queued"
            );
            let authority = Authority::open(env.root.join("authority.sqlite")).unwrap();
            let renewed = Env::delegation_for(
                &env.coglet_key,
                &env.coglet,
                env.hub.id().to_string(),
                env.endpoint.id().to_string(),
                DELEGATION_GEN + 1,
                T0 + 100_000,
                &Options::receipts().delegation_methods,
            );
            match mode {
                0 => env.clock.store(DELEGATION_EXPIRES, Ordering::SeqCst),
                1 => authority
                    .revoke_delegation(env.hub.id(), &env.coglet)
                    .unwrap(),
                _ => authority
                    .replace_delegation(
                        env.hub.id(),
                        env.endpoint.id(),
                        &env.coglet,
                        &renewed,
                        || Some(T0),
                    )
                    .unwrap(),
            }
            barrier.release();
            assert_eq!(push.await.unwrap()["result"]["state"], "queued");
            let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM proto_preinstall_boundaries",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                0,
                "mode {mode}"
            );
            let retained: String = db
                .query_row(
                    "SELECT receipt FROM proto_git_operations WHERE operation_id=?1",
                    [&id],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&retained).unwrap()["state"],
                "queued"
            );
            let proof = env.delegation();
            denied(&env.install(&bundle.signed, &proof).await);
            {
                if mode != 2 {
                    authority
                        .replace_delegation(
                            env.hub.id(),
                            env.endpoint.id(),
                            &env.coglet,
                            &renewed,
                            || Some(env.clock.load(Ordering::SeqCst)),
                        )
                        .unwrap();
                }
                env.config
                    .targets
                    .get_mut(&env.coglet)
                    .unwrap()
                    .delegation_generation = DELEGATION_GEN + 1;
            }
            env.restart_handler().await;
            assert_eq!(wait_applied(&mut env, &id).await["operation_id"], id);
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM proto_preinstall_boundaries",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
                1
            );
            assert_eq!(
                env.simple("hub.status").await["result"]["live_commit"],
                Value::Null
            );
            // Exported quarantine leases may be inherited by unrelated forked
            // validators. Their documented deferred orphan sweep is not a
            // completion oracle; boundary/receipt SQL custody above is.
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_preinstall_superseded_original_never_borrows_new_boundary() {
    timed!(async {
        let mut env = Env::new(0x73, Options::receipts()).await.unwrap();
        let barrier = InitialBoundaryBarrier::install(&mut env).await;
        let old = env.bundle(1, None, &[]);
        let (old_id, old_push) = begin_push(&mut env, &old);
        barrier.wait().await;
        let latest = env.bundle(2, Some(&old.tip), &[("revision", b"two")]);
        let (new_id, new_push) = begin_push(&mut env, &latest);
        assert_eq!(new_push.await.unwrap()["result"]["state"], "queued");
        let terminal = wait_applied(&mut env, &new_id).await;
        barrier.release();
        assert_eq!(old_push.await.unwrap()["result"]["state"], "queued");
        assert_eq!(
            result_of(&receipt_of(&mut env, &old_id).await)["state"],
            "queued"
        );
        let proof = env.delegation();
        denied(&env.install(&old.signed, &proof).await);
        env.restart_handler().await;
        assert_eq!(result_of(&receipt_of(&mut env, &new_id).await), terminal);
        assert_eq!(
            result_of(&receipt_of(&mut env, &old_id).await)["state"],
            "queued"
        );
        let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
        let ids: Vec<String> = db
            .prepare("SELECT operation_id FROM proto_preinstall_boundaries")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(ids, vec![new_id]);
        applied(&env.install(&latest.signed, &proof).await);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_git_and_lifecycle_ids_conflict_across_ledgers_and_restart() {
    timed!(async {
        let mut env = Env::new(0x74, Options::receipts()).await.unwrap();
        let initial = env.bundle(1, None, &[]);
        let push = env.push(&initial).await;
        let push_id = operation_id(&push);
        let original = wait_applied(&mut env, &push_id).await;
        let collision = env.install_request(push_id.clone(), &initial.signed).await;
        assert_eq!(error_code(&collision), "conflict", "{collision}");
        assert_eq!(result_of(&receipt_of(&mut env, &push_id).await), original);
        let proof = env.delegation();
        let install = env.install(&initial.signed, &proof).await;
        applied(&install);
        let stopped = env.simple("hub.stop").await;
        applied(&stopped);
        let next = env.bundle(2, Some(&initial.tip), &[("revision", b"next")]);
        for reopen in [false, true] {
            if reopen {
                env.restart_handler().await;
            }
            for receipt in [&install, &stopped] {
                let id = operation_id(receipt);
                let wal = env
                    .config
                    .wal_root
                    .join(coglet_runtime::digest(env.coglet.as_bytes()))
                    .join("state.wal");
                let before = std::fs::read(&wal).unwrap();
                let request = env.request_with("hub.git.push", id.clone(),
                    json!({"update":next.signed,"size":next.bytes.len(),"sha256":hex::encode(Sha256::digest(&next.bytes))}),
                    &env.actor, Some(env.ticket.clone()));
                let result = env
                    .session
                    .push_git(request, &mut next.bytes.as_slice())
                    .await
                    .unwrap();
                assert_eq!(error_code(&result), "conflict", "{result}");
                assert_eq!(
                    result_of(&receipt_of(&mut env, &id).await),
                    receipt["result"]
                );
                assert_eq!(std::fs::read(&wal).unwrap(), before);
            }
            let conflict = env.install_request(push_id.clone(), &initial.signed).await;
            assert_eq!(error_code(&conflict), "conflict", "{conflict}");
            assert_eq!(result_of(&receipt_of(&mut env, &push_id).await), original);
        }
        let db = rusqlite::Connection::open(env.root.join("authority.sqlite")).unwrap();
        assert_eq!(
            db.query_row("SELECT count(*) FROM proto_ref_audit", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM proto_git_operations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
    });
}

/// `hub.receipt {operation_id}` as `actor`/`ticket`, against `env.coglet`.
async fn receipt_as(env: &mut Env, id: &str, actor: &str, ticket: Option<&str>) -> Value {
    let correlation_id = env.next();
    let request = env.request_with(
        "hub.receipt",
        correlation_id,
        json!({ "operation_id": id }),
        actor,
        ticket.map(str::to_owned),
    );
    env.call(request).await
}

async fn receipt_of(env: &mut Env, id: &str) -> Value {
    let (actor, ticket) = (env.actor.clone(), env.ticket.clone());
    receipt_as(env, id, &actor, Some(&ticket)).await
}

/// Expected terminal projection for an initial push's earlier admission
/// observation. No reply is injected; callers poll the actual device above.
fn terminal_initial(reply: &Value) -> Value {
    let mut result = reply["result"].clone();
    if result["state"] == "queued" {
        result["state"] = json!("applied");
        result["step"] = json!(0);
    }
    result
}

fn result_of(reply: &Value) -> Value {
    assert_eq!(reply["type"], "reply", "expected a receipt: {reply}");
    reply["result"].clone()
}

/// Typed error body without the per-request correlation ID.
fn error_of(reply: &Value) -> Value {
    assert_eq!(reply["type"], "error", "expected an error: {reply}");
    reply["error"].clone()
}

fn operation_id(reply: &Value) -> String {
    reply["result"]["operation_id"].as_str().unwrap().to_owned()
}

fn status_step(reply: &Value) -> u64 {
    reply["result"]["step"].as_u64().unwrap()
}

/// push -> install -> stop under the primary scope; returns the three replies.
async fn push_install_stop(env: &mut Env) -> (Value, Value, Value) {
    let bundle = env.bundle(1, None, &[]);
    let pushed = env.push(&bundle).await;
    assert_eq!(pushed["result"]["state"], "queued", "{pushed}");
    let delegation = env.delegation();
    let installed = env.install(&bundle.signed, &delegation).await;
    applied(&installed);
    let stopped = env.simple("hub.stop").await;
    applied(&stopped);
    (pushed, installed, stopped)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_original_scope_roundtrip() {
    timed!(async {
        let mut env = Env::new(0x41, Options::receipts()).await.unwrap();
        let (pushed, installed, stopped) = push_install_stop(&mut env).await;
        for (original, state) in [(&installed, "applied"), (&stopped, "applied")] {
            let id = operation_id(original);
            let polled = receipt_of(&mut env, &id).await;
            // The device returns the very receipt it committed at admission.
            assert_eq!(result_of(&polled), original["result"], "{polled}");
            assert_eq!(polled["result"]["state"], state);
            assert_eq!(polled["result"]["error"], Value::Null);
            coglet_runtime::validate("hub-receipt-applied", &polled["result"]).unwrap();
            // Receipt step is consistent with (<=) status, without atomicity.
            let status = env.simple("hub.status").await;
            assert!(
                status_step(&polled) <= status_step(&status),
                "{polled} {status}"
            );
        }
        // Initial push completed at its own verified quarantine boundary,
        // before install. It does not claim guest execution or a generation.
        let push_id = operation_id(&pushed);
        let polled = receipt_of(&mut env, &push_id).await;
        assert_eq!(result_of(&polled), terminal_initial(&pushed));
        coglet_runtime::validate("hub-receipt-applied", &polled["result"]).unwrap();
        // The receipt carries no applied generation or commit: exactly the four
        // frozen receipt fields.
        let mut keys: Vec<_> = polled["result"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        keys.sort();
        assert_eq!(keys, ["error", "operation_id", "state", "step"]);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_denies_cross_identity() {
    timed!(async {
        let mut env = Env::new(0x42, Options::receipts()).await.unwrap();
        let (pushed, installed, stopped) = push_install_stop(&mut env).await;
        let (actor, other_actor) = (env.actor.clone(), env.other_actor.clone());
        let never = correlation(777_777);
        let identities: [(&str, &str, Option<&str>); 4] = [
            ("another ticket", &actor, Some(OTHER_TICKET)),
            ("no ticket", &actor, None),
            ("another actor", &other_actor, Some(TICKET)),
            // May poll receipts but holds no grant for the original method.
            ("no original-method grant", &actor, Some(NARROW_TICKET)),
        ];
        for original in [&pushed, &installed, &stopped] {
            let id = operation_id(original);
            for (label, who, ticket) in identities {
                let known = receipt_as(&mut env, &id, who, ticket).await;
                let unknown = receipt_as(&mut env, &never, who, ticket).await;
                assert_eq!(error_code(&known), "not_found", "{label}: {known}");
                // Same typed reply as a never-admitted ID: existence is hidden.
                assert_eq!(error_of(&known), error_of(&unknown), "{label}");
            }
        }
        // An unprovisioned actor is denied identically for known and unknown IDs.
        let stranger = format!("hub:{}", key(0xEE, 1).public());
        let id = operation_id(&installed);
        let known = receipt_as(&mut env, &id, &stranger, Some(TICKET)).await;
        let unknown = receipt_as(&mut env, &never, &stranger, Some(TICKET)).await;
        denied(&known);
        assert_eq!(error_of(&known), error_of(&unknown));
        // Another target: the same denial whether or not the ID exists there.
        let other_target = format!("coglet:{}", key(0xEE, 3).public());
        for probe in [&id, &never] {
            let correlation_id = env.next();
            let mut request = env.request_with(
                "hub.receipt",
                correlation_id,
                json!({ "operation_id": probe }),
                &actor,
                Some(TICKET.to_owned()),
            );
            request["target"] = json!(other_target);
            denied(&env.call(request).await);
        }
        // The original caller still reads every receipt.
        for original in [&pushed, &installed, &stopped] {
            let polled = receipt_of(&mut env, &operation_id(original)).await;
            assert_eq!(result_of(&polled), terminal_initial(original));
        }
        // A cross-identity retry of the original method does not borrow it.
        let reused = env.request_with(
            "hub.stop",
            id.clone(),
            json!({}),
            &actor,
            Some(OTHER_TICKET.to_owned()),
        );
        assert_eq!(error_code(&env.call(reused).await), "conflict");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_survives_restart() {
    timed!(async {
        let mut env = Env::new(0x43, Options::receipts()).await.unwrap();
        let (pushed, installed, stopped) = push_install_stop(&mut env).await;
        let before_status = env.simple("hub.status").await["result"].clone();
        env.restart_handler().await;
        for original in [&pushed, &installed, &stopped] {
            let polled = receipt_of(&mut env, &operation_id(original)).await;
            assert_eq!(result_of(&polled), terminal_initial(original), "{polled}");
        }
        // Original scope survived too: other identities stay denied after restart.
        let (actor, id) = (env.actor.clone(), operation_id(&installed));
        let other = receipt_as(&mut env, &id, &actor, Some(OTHER_TICKET)).await;
        assert_eq!(error_code(&other), "not_found");
        assert_eq!(env.simple("hub.status").await["result"], before_status);
        // Initial pre-install completion survives even without any install.
        let mut early = Env::new(0x44, Options::receipts()).await.unwrap();
        let bundle = early.bundle(1, None, &[]);
        let queued = early.push(&bundle).await;
        assert_eq!(queued["result"]["state"], "queued");
        early.restart_handler().await;
        let polled = receipt_of(&mut early, &operation_id(&queued)).await;
        assert_eq!(result_of(&polled), terminal_initial(&queued));
        assert_eq!(polled["result"]["step"], 0);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_identical_retry_no_reexecute() {
    timed!(async {
        let mut env = Env::new(0x45, Options::receipts()).await.unwrap();
        let (_, installed, _) = push_install_stop(&mut env).await;
        let stop = env.request("hub.stop", json!({}));
        let first = env.call(stop.clone()).await;
        applied(&first);
        let status = env.simple("hub.status").await["result"].clone();
        for _ in 0..2 {
            assert_eq!(env.call(stop.clone()).await["result"], first["result"]);
        }
        env.restart_handler().await;
        assert_eq!(env.call(stop.clone()).await["result"], first["result"]);
        let polled = receipt_of(&mut env, &operation_id(&first)).await;
        assert_eq!(result_of(&polled), first["result"]);
        assert_eq!(env.simple("hub.status").await["result"], status);
        // Retrying the install under its original ID returns the original receipt.
        let again = env
            .install_request(operation_id(&installed), &installed_ref(&env).await)
            .await;
        assert_eq!(again["result"], installed["result"]);
    });
}

/// The signed initial ref accepted for the primary coglet (for install retries).
async fn installed_ref(env: &Env) -> Value {
    env.handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .map(|s| s.lock().unwrap().managed_identity().unwrap()["initial"].clone())
        .unwrap_or_else(|| panic!("service not open"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_conflicting_intent_typed() {
    timed!(async {
        let mut env = Env::new(0x46, Options::receipts()).await.unwrap();
        let (_, _, stopped) = push_install_stop(&mut env).await;
        let id = operation_id(&stopped);
        let actor = env.actor.clone();
        // Same ID, different method.
        let as_start = env.request_with(
            "hub.start",
            id.clone(),
            json!({}),
            &actor,
            Some(TICKET.to_owned()),
        );
        assert_eq!(error_code(&env.call(as_start).await), "conflict");
        // Same ID, same method, different ticket scope.
        let other_ticket = env.request_with(
            "hub.stop",
            id.clone(),
            json!({}),
            &actor,
            Some(OTHER_TICKET.to_owned()),
        );
        assert_eq!(error_code(&env.call(other_ticket).await), "conflict");
        // Neither conflict changed the original receipt.
        let polled = receipt_of(&mut env, &id).await;
        assert_eq!(result_of(&polled), stopped["result"]);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_install_retry_cannot_borrow_scope_or_reuse_stop_id() {
    timed!(async {
        let mut env = Env::new(0x4b, Options::receipts()).await.unwrap();
        let (_, installed, stopped) = push_install_stop(&mut env).await;
        let initial = installed_ref(&env).await;
        let delegation = env.delegation();
        let actor = env.actor.clone();
        for restart in [false, true] {
            if restart {
                env.restart_handler().await;
            }
            for (id, ticket) in [
                (operation_id(&installed), OTHER_TICKET),
                (operation_id(&stopped), TICKET),
            ] {
                let request = env.request_with(
                    "hub.install",
                    id,
                    json!({"initial":initial,"delegation":delegation}),
                    &actor,
                    Some(ticket.to_owned()),
                );
                let reply = env.call(request).await;
                assert_eq!(error_code(&reply), "conflict", "{reply}");
            }
            assert_eq!(
                result_of(&receipt_of(&mut env, &operation_id(&installed)).await),
                installed["result"]
            );
        }
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_queued_never_applied_without_boundary() {
    timed!(async {
        let mut env = Env::new(0x47, Options::receipts()).await.unwrap();
        // Installed-target activation is owned by a separate producer. Staging
        // and recovery cannot promote this push without its actual boundary.
        let (initial, _) = pushed_and_installed(&mut env).await;
        let bundle = env.bundle(2, Some(&initial.tip), &[]);
        let pushed = env.push(&bundle).await;
        let id = operation_id(&pushed);
        for round in 0..2 {
            let polled = receipt_of(&mut env, &id).await;
            let result = result_of(&polled);
            assert_eq!(result["state"], "failed", "round {round}: {polled}");
            assert_eq!(result["step"], Value::Null);
            assert_eq!(result["error"]["code"], "permission_denied");
            coglet_runtime::validate("hub-receipt-failed", &result).unwrap();
            // Status still names the original installed commit, not staging.
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(initial.tip))
            );
            env.restart_handler().await;
        }
        // An unknown ID never becomes a fabricated queued/applied receipt.
        let unknown = receipt_of(&mut env, &correlation(424_242)).await;
        assert_eq!(error_code(&unknown), "not_found");
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn installed_push_original_id_queued_then_applied() {
    timed!(async {
        let mut options = Options::receipts();
        options.delegation_methods.push("hub.git.push");
        let mut env = Env::new(0x48, options).await.unwrap();
        let (initial, installed) = pushed_and_installed(&mut env).await;
        let bundle = env.bundle(2, Some(&initial.tip), &[]);
        let barrier = InitialBoundaryBarrier::install(&mut env).await;
        let (original_id, pushing) = begin_push(&mut env, &bundle);
        barrier.wait().await;
        assert_eq!(
            result_of(&receipt_of(&mut env, &original_id).await)["state"],
            "queued"
        );
        // Verification is outside the step mutex, but durable admission must
        // prevent any next-step input/code pinning while it is pending.
        {
            let service = env
                .handler
                .as_ref()
                .unwrap()
                .share_service(&env.coglet)
                .unwrap();
            let mut locked = service.lock().unwrap();
            assert!(
                locked
                    .run_next(
                        &Value::Null,
                        &mut coglet_runtime::MockEngine::new(Value::Null)
                    )
                    .unwrap()
                    .is_none()
            );
        }
        barrier.release();
        let pushed = pushing.await.unwrap();
        applied(&installed);
        assert_eq!(pushed["result"]["state"], "applied");
        assert_eq!(operation_id(&pushed), original_id);
        let mut terminal = Value::Null;
        for _ in 0..2 {
            let polled = receipt_of(&mut env, &original_id).await;
            let result = result_of(&polled);
            assert_eq!(result["state"], "applied", "{polled}");
            assert_eq!(result["operation_id"], original_id);
            if terminal.is_null() {
                terminal = result.clone();
            } else {
                assert_eq!(terminal, result);
            }
            assert_eq!(
                state(&env.simple("hub.status").await),
                ("stopped".into(), json!(bundle.tip))
            );
            env.restart_handler().await;
        }
        // A historical install retry cannot overwrite the new live code.
        let delegation = env.delegation();
        let reply = env.install(&initial.signed, &delegation).await;
        assert_eq!(error_code(&reply), "permission_denied");
        assert_eq!(state(&env.simple("hub.status").await).1, bundle.tip);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hub_receipt_request_validation_and_fixtures() {
    timed!(async {
        let mut env = Env::new(0x49, Options::receipts()).await.unwrap();
        let (_, installed, _) = push_install_stop(&mut env).await;
        let id = operation_id(&installed);
        // The frozen hub-receipt-* corpus consists of response-schema cases,
        // not RPC envelopes. In particular, '*-unknown' adds an unknown FIELD;
        // it is not a lookup of an unknown operation ID. Validate the unchanged
        // corpus and separately validate actual receiver responses below.
        let fixtures: Value =
            serde_json::from_str(include_str!("../../../spec/fixtures/hub/cases.json")).unwrap();
        let cases: Vec<_> = fixtures
            .as_array()
            .unwrap()
            .iter()
            .filter(|c| c["name"].as_str().unwrap().starts_with("hub-receipt-"))
            .collect();
        assert!(!cases.is_empty());
        for case in cases {
            let valid =
                coglet_runtime::validate(case["type"].as_str().unwrap(), &case["value"]).is_ok();
            assert_eq!(valid, case["valid"].as_bool().unwrap(), "{}", case["name"]);
        }
        let polled = receipt_of(&mut env, &id).await;
        coglet_runtime::validate("hub-receipt-applied", &polled["result"]).unwrap();
        let unknown = receipt_of(&mut env, &correlation(424_249)).await;
        assert_eq!(error_code(&unknown), "not_found", "{unknown}");
        // Exercise the actual receiver directly for malformed envelopes: Session
        // validates outgoing requests and would otherwise hide receiver bugs.
        use coglet_proto::transport::{OperationHandler, RequestContext};
        let context = || RequestContext {
            peer: env.config.hub,
            local: env.config.device,
            deadline: tokio::time::Instant::now() + Duration::from_secs(5),
        };
        let handler = env.handler.as_ref().unwrap().clone();
        let receiver_context = context();
        let correlation_id = env.next();
        let actor = env.actor.clone();
        let mut missing = env.request_with(
            "hub.receipt",
            correlation_id,
            json!({}),
            &actor,
            Some(TICKET.to_owned()),
        );
        missing["params"] = json!({});
        let frame = handler.handle(receiver_context.clone(), missing).await;
        assert_eq!(error_code(&frame), "invalid_argument", "{frame}");
        let correlation_id = env.next();
        let mut bad_id = env.request_with(
            "hub.receipt",
            correlation_id,
            json!({"operation_id": "not-a-correlation-id"}),
            &actor,
            Some(TICKET.to_owned()),
        );
        bad_id["params"]["operation_id"] = json!("nope");
        let frame = handler.handle(receiver_context.clone(), bad_id).await;
        assert_eq!(error_code(&frame), "invalid_argument", "{frame}");
        let correlation_id = env.next();
        let mut no_context = env.request_with(
            "hub.receipt",
            correlation_id,
            json!({ "operation_id": id }),
            &actor,
            None,
        );
        no_context.as_object_mut().unwrap().remove("context");
        let frame = handler.handle(receiver_context, no_context).await;
        assert_eq!(error_code(&frame), "invalid_argument", "{frame}");
    });
}
