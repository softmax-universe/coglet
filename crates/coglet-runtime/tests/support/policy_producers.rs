//! Real ADR-0033 producer endpoint/daemon tests. No in-process producer substitutes.
use super::*;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

include!("policy_producer_helpers.rs");
include!("policy_producer_cases.rs");

struct Daemon {
    env: Env,
    child: ChildGuard,
    root: PathBuf,
    config: Value,
    device: iroh::EndpointId,
    delegation: Value,
    socket: PathBuf,
    surface: String,
    nonce: u64,
    // Separate short, private socket directory: Linux sockaddr_un is bounded.
    private: tempfile::TempDir,
    /// Environment of every cogletd spawn (initial and restarts), e.g. the
    /// subprocess-local LD_PRELOAD fault shim. Empty for production-like runs.
    child_env: Vec<(String, String)>,
    action: bool,
    barrier: Option<Barrier>,
}
/// Validator wrapper: while `arm` exists the SECOND validation of a push (the
/// one that follows durable queued admission) waits for `release`.
#[derive(Clone)]
struct Barrier {
    arm: PathBuf,
    entered: PathBuf,
    release: PathBuf,
}
#[derive(Clone, Copy, Default)]
struct Opts {
    /// Installed guest emits one real `speech.speak_chunk` action per step.
    action: bool,
    barrier: bool,
    /// LD_PRELOAD WAL-write/BOOTTIME fault shim (inert until armed).
    shim: bool,
}
impl Daemon {
    async fn new(seed: u8) -> Self {
        Self::with(seed, Opts::default()).await
    }
    async fn with(seed: u8, opts: Opts) -> Self {
        let mut options = Options::receipts();
        options.delegation_methods.push("hub.git.push");
        let mut env = Env::new(seed, options.clone()).await.unwrap();
        let device_key = key(seed, 4);
        let device = device_key.public();
        let root = env.root.join("policy-child");
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
            for ticket in [TICKET, OTHER_TICKET] {
                for grant in &grants {
                    authority
                        .provision_scope(
                            env.hub.id(),
                            &Scope {
                                actor: env.actor.clone(),
                                ticket: Some(ticket.into()),
                            },
                            grant,
                        )
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
        let mut validator = validator_path();
        let mut barrier = None;
        if opts.barrier {
            let b = Barrier {
                arm: root.join("barrier-arm"),
                entered: root.join("barrier-entered"),
                release: root.join("barrier-release"),
            };
            let (first, second) = (root.join("barrier-first"), root.join("barrier-second"));
            let wrapper = root.join("validator-barrier");
            std::fs::write(&wrapper, format!(
                "#!/bin/sh\nif [ -f '{arm}' ]; then\n if /bin/mkdir '{first}' 2>/dev/null; then :; elif /bin/mkdir '{second}' 2>/dev/null; then : > '{entered}'; while [ ! -f '{release}' ]; do /bin/sleep 0.01; done; fi\nfi\nexec '{real}' \"$@\"\n",
                arm = b.arm.display(), first = first.display(), second = second.display(),
                entered = b.entered.display(), release = b.release.display(), real = validator.display()
            )).unwrap();
            std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
            validator = wrapper;
            barrier = Some(b);
        }
        let mut child_env = vec![];
        if opts.shim {
            let library = root.join("policy_faults.so");
            let source =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/support/policy_faults.c");
            assert!(
                Command::new("cc")
                    .args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"])
                    .arg(&source)
                    .arg("-o")
                    .arg(&library)
                    .status()
                    .unwrap()
                    .success()
            );
            let canonical = std::fs::canonicalize(&root).unwrap();
            let wal = canonical
                .join("wal")
                .join(coglet_runtime::digest(env.coglet.as_bytes()))
                .join("state.wal");
            for (k, v) in [
                ("LD_PRELOAD", library.display().to_string()),
                ("COGLET_FAULT_WAL", wal.display().to_string()),
                (
                    "COGLET_FAULT_ARM",
                    root.join("fault-arm").display().to_string(),
                ),
                (
                    "COGLET_FAULT_HITS",
                    root.join("fault-hits").display().to_string(),
                ),
                ("COGLET_FAULT_LEFT", "4".into()),
                (
                    "COGLET_FAULT_CLOCK",
                    root.join("fault-clock").display().to_string(),
                ),
            ] {
                child_env.push((k.into(), v));
            }
        }
        let private = tempfile::Builder::new().prefix("pp-").tempdir().unwrap();
        std::fs::set_permissions(private.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let socket = private.path().join("a");
        let producer = private.path().join("p");
        let surface = format!("surface:{}", key(seed, 5).public());
        let binding = root.join("binding.json");
        std::fs::write(
            &binding,
            serde_json::to_vec(&json!({"surface":surface,"permitted_coglets":[env.coglet]}))
                .unwrap(),
        )
        .unwrap();
        let config = json!({"hub":env.hub.id().to_string(),"device":device.to_string(),
            "identity_path":identity,"authority_path":authority_path,"validator_path":validator,
            "wal_root":root.join("wal"),"quarantine":root.join("quarantine"),
            "catalog":if opts.action { act_catalog() } else { restart_adapter::catalog() },
            "grants":{"coglet":env.coglet,"operations":operations(opts.action),"resources":["audio.mic","audio.speaker"]},
            "targets":{(env.coglet.clone()):{"grant_generation":GRANT_GEN,"delegation_generation":DELEGATION_GEN,
                "native_schedule":{"trace_id":"11".repeat(16),"seed":1,"runtime_bytes":8_388_608,"stack_bytes":65_536}}},
            "kill_switch":env.root.join("kill.json"),"hub_addr":env.hub.addr().ip_addrs().next().unwrap(),"invite":invite,
            "adapter":{"socket":socket,"binding":binding,"uid":unsafe {libc::geteuid()},"coglet":env.coglet},
            "profile":{"socket":producer,"principals":[{"principal":"policy","uid":unsafe {libc::geteuid()},
                "actor":env.actor,"target":env.coglet,"ticket":OTHER_TICKET,
                "stop":{"actor":env.actor,"ticket":TICKET,"operation_id":correlation(0xeeee),"params":{}}}]}});
        let path = root.join("router.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let log = root.join("child.log");
        let mut child = spawn_child(&path, &log, &child_env);
        env.session = child_session(&env.hub, device, &mut child, &log).await;
        assert_eq!(env.simple("hub.status").await["result"]["state"], "stopped");
        Self {
            env,
            child,
            root,
            config,
            device,
            delegation,
            socket,
            surface,
            nonce: 0,
            private,
            child_env,
            action: opts.action,
            barrier,
        }
    }
    fn pins(&self, bundle: &Bundle) -> Value {
        let generation = bundle.signed["statement"]["generation"].as_u64().unwrap();
        let wasm = std::fs::read(
            self.env
                .root
                .join(format!("git-{generation}/bin/step.wasm")),
        )
        .unwrap();
        json!({"generation":generation,"commit":bundle.tip,"executable_sha256":coglet_runtime::digest(&wasm)})
    }
    fn frame(&mut self, method: &str, params: Value) -> Value {
        self.nonce += 1;
        json!({"version":1,"method":method,"nonce":format!("n{}",self.nonce),"principal":"policy",
            "actor":self.env.actor,"target":self.env.coglet,"ticket":OTHER_TICKET,"params":params})
    }
    async fn raw(&self, frame: Value) -> Value {
        let mut stream = UnixStream::connect(self.private.path().join("p"))
            .await
            .unwrap();
        let mut bytes = serde_json::to_vec(&frame).unwrap();
        bytes.push(b'\n');
        stream.write_all(&bytes).await.unwrap();
        let mut line = String::new();
        tokio::time::timeout(
            Duration::from_secs(8),
            BufReader::new(stream).read_line(&mut line),
        )
        .await
        .unwrap()
        .unwrap();
        serde_json::from_str(&line).unwrap_or_else(|e| panic!("producer reply {line:?}: {e}"))
    }
    async fn local(&mut self, method: &str, params: Value) -> Value {
        let frame = self.frame(method, params);
        self.raw(frame).await
    }
    async fn install(&mut self, wat: &str) -> Bundle {
        let manifest = serde_json::to_vec(&json!({"version":"0.1","id":self.env.coglet,
            "subscriptions":["speech.start"],"required_capabilities":operations(self.action),
            "exclusive_outputs":["audio.mic","audio.speaker"],"budgets":{"compute":10000,
            "compute_mode":native_compute_mode(),"memory_pages":2,"wall_time_us":2_000_000,"max_events":16,"host_bytes":65536}})).unwrap();
        let bundle = self
            .env
            .bundle_wasm(1, None, &[("manifest.json", &manifest)], wat);
        let pushed = self.env.push(&bundle).await;
        assert_eq!(pushed["result"]["state"], "queued", "{pushed}");
        let installed = self.env.install(&bundle.signed, &self.delegation).await;
        applied(&installed);
        let attested = self.local("attest_applied", self.pins(&bundle)).await;
        assert_eq!(attested["ok"], true, "{attested}");
        assert_eq!(
            attested["result"]["initial"]["code"]["operation_id"],
            operation_id(&pushed)
        );
        assert_eq!(
            attested["result"]["initial"]["install"]["operation_id"],
            operation_id(&installed)
        );
        assert_eq!(
            attested["result"]["initial"]["code"]["scope"]["ticket"],
            TICKET
        );
        bundle
    }
    async fn enable(&mut self, bundle: &Bundle) -> Value {
        let attested = self.local("attest_applied", self.pins(bundle)).await;
        assert_eq!(attested["ok"], true, "{attested}");
        let mut params = self.pins(bundle);
        params["daemon_instance"] = attested["daemon_instance"].clone();
        let enabled = self.local("enable_output_lease", params).await;
        assert_eq!(enabled["ok"], true, "{enabled}");
        enabled
    }
    async fn execute(&mut self, bundle: &Bundle) -> UnixStream {
        applied(&self.env.simple("hub.start").await);
        self.enable(bundle).await;
        let link =
            restart_adapter::events(&self.socket, &self.surface, &self.env.coglet, 1, 1).await;
        restart_adapter::step_at_least(&mut self.env, 1).await;
        link
    }
    async fn failed_health(&mut self, bundle: &Bundle) -> Value {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let sample = self.local("observe_health", self.pins(bundle)).await;
                assert_eq!(sample["ok"], true, "{sample}");
                if sample["result"]["counters"]["failed"].as_u64().unwrap() > 0 {
                    return sample;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("real failed native attempt reaches producer counters")
    }
    fn wal(&self) -> PathBuf {
        self.root
            .join("wal")
            .join(coglet_runtime::digest(self.env.coglet.as_bytes()))
            .join("state.wal")
    }
    async fn restart(&mut self) -> Vec<Value> {
        self.restart_with(|_| {}).await
    }
    /// SIGKILL, take exclusive WAL custody, then respawn with edited config.
    async fn restart_with(&mut self, edit: impl FnOnce(&mut Value)) -> Vec<Value> {
        self.child.kill_and_wait();
        // Exclusive WAL custody is obtained only after the exact child was reaped.
        let records = {
            let store = coglet_runtime::DurableStore::open(&self.wal()).unwrap();
            store.records().to_vec()
        };
        assert!(
            !records.is_empty(),
            "every named test must execute a real native guest"
        );
        for record in &records {
            assert_eq!(
                record["engine"]["policy"],
                native_compute_mode(),
                "{record}"
            );
            assert!(record["usage"]["compute"].as_u64().unwrap() > 0, "{record}");
        }
        if self.socket.exists() {
            std::fs::remove_file(&self.socket).unwrap();
        }
        tokio::time::timeout(Duration::from_secs(50), async {
            while self.env.hub.session(self.device).await.is_some() {
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .expect("old SIGKILL transport retires");
        self.config["invite"] = Value::Null;
        edit(&mut self.config);
        let path = self.root.join("router.json");
        std::fs::write(&path, serde_json::to_vec(&self.config).unwrap()).unwrap();
        let log = self.root.join("restart.log");
        self.child = spawn_child(&path, &log, &self.child_env);
        self.env.session = child_session(&self.env.hub, self.device, &mut self.child, &log).await;
        records
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attestation_matches_applied_generation_after_restart() {
    let mut d = Daemon::new(0x91).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let initial = d.local("attest_applied", d.pins(&a)).await;
    let b = d.env.bundle(2, Some(&a.tip), &[("revision", b"B")]);
    let pushed_b = d.env.push(&b).await;
    applied(&pushed_b);
    let attested_b = d.local("attest_applied", d.pins(&b)).await;
    assert_eq!(attested_b["ok"], true, "{attested_b}");
    assert_eq!(attested_b["result"]["generation"], 2);
    assert_eq!(
        attested_b["result"]["code"]["operation_id"],
        operation_id(&pushed_b)
    );
    assert_eq!(
        attested_b["result"]["initial"],
        initial["result"]["initial"]
    );
    // Return to the EXACT A commit, with a new signed generation and original
    // admission ID. Commit equality alone cannot describe this activation.
    let mut statement = a.signed["statement"].clone();
    statement["old"] = json!(b.tip);
    statement["generation"] = json!(3);
    let returned = Bundle {
        bytes: a.bytes.clone(),
        tip: a.tip.clone(),
        signed: sign(&d.env.hub_key, "hub-ref", statement),
    };
    let pushed_a = d.env.push(&returned).await;
    applied(&pushed_a);
    let mut returned_pins = d.pins(&a);
    returned_pins["generation"] = json!(3);
    let before = d.local("attest_applied", returned_pins.clone()).await;
    assert_eq!(before["ok"], true, "{before}");
    assert_eq!(before["result"]["commit"], a.tip);
    assert_eq!(before["result"]["generation"], 3);
    assert_eq!(
        before["result"]["code"]["operation_id"],
        operation_id(&pushed_a)
    );
    assert_eq!(before["result"]["initial"], initial["result"]["initial"]);
    assert_eq!(
        d.local("attest_applied", d.pins(&a)).await["ok"],
        false,
        "A commit must not make generation 1 current again"
    );
    let mut forged = d.frame("attest_applied", d.pins(&a));
    forged["actor"] = json!(d.env.other_actor);
    assert_eq!(d.raw(forged).await["error"]["code"], "permission_denied");
    // Unknown method / principal / extra member through the real daemon.
    let mut unknown = d.frame("attest_applied", d.pins(&a));
    unknown["method"] = json!("promote_generation");
    assert_eq!(d.raw(unknown).await["ok"], false);
    let mut stranger = d.frame("attest_applied", d.pins(&a));
    stranger["principal"] = json!("stranger");
    assert_eq!(d.raw(stranger).await["error"]["code"], "permission_denied");
    let mut extra = d.pins(&a);
    extra["generation_hint"] = json!(2);
    assert_eq!(d.local("attest_applied", extra).await["ok"], false);
    let mut changed = d.pins(&a);
    changed["executable_sha256"] = json!("00".repeat(32));
    assert_eq!(d.local("attest_applied", changed).await["ok"], false);
    drop(link);
    d.restart().await;
    let after = d.local("attest_applied", returned_pins).await;
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(after["result"], before["result"]);
    assert_ne!(after["daemon_instance"], before["daemon_instance"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn health_counters_bound_to_generation_and_reject_stale() {
    let mut d = Daemon::new(0x92).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let first = d.local("observe_health", d.pins(&a)).await;
    assert_eq!(first["ok"], true, "{first}");
    assert!(first["result"]["counters"]["committed"].as_u64().unwrap() > 0);
    let second = d.local("observe_health", d.pins(&a)).await;
    assert!(
        second["result"]["observation_sequence"].as_u64().unwrap()
            > first["result"]["observation_sequence"].as_u64().unwrap(),
        "{second}"
    );
    let mut stale = d.pins(&a);
    stale["generation"] = json!(2);
    assert_eq!(d.local("observe_health", stale).await["ok"], false);
    let mut stale = d.pins(&a);
    stale["commit"] = json!("00".repeat(20));
    assert_eq!(d.local("observe_health", stale).await["ok"], false);
    let mut forged = d.frame("observe_health", d.pins(&a));
    forged["params"]["counters"] = json!({"committed":999});
    assert_eq!(d.raw(forged).await["ok"], false);
    drop(link);
    d.restart().await;
    let after = d.local("observe_health", d.pins(&a)).await;
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(after["result"]["counters"], second["result"]["counters"]);
    assert_eq!(after["result"]["output_lease_state"], "closed");
    // Independent real signed activations exercise trap and budget outcomes.
    // Each is crash/reopened too; no synthetic counter injection.
    for (seed, body, budget) in [
        (0x95, "unreachable))", false),
        (0x96, "(loop br 0) i32.const 0))", true),
    ] {
        let mut failure = Daemon::new(seed).await;
        let wat = WASM_WAT.replace("i32.const 0))", body);
        let bundle = failure.install(&wat).await;
        applied(&failure.env.simple("hub.start").await);
        failure.enable(&bundle).await;
        let adapter =
            restart_adapter::events(&failure.socket, &failure.surface, &failure.env.coglet, 1, 1)
                .await;
        let observed = failure.failed_health(&bundle).await;
        assert_eq!(observed["result"]["counters"]["committed"], 0);
        assert_eq!(
            observed["result"]["counters"]["budget_exceeded"]
                .as_u64()
                .unwrap()
                > 0,
            budget,
            "{observed}"
        );
        drop(adapter);
        failure.restart().await;
        let recovered = failure.local("observe_health", failure.pins(&bundle)).await;
        assert_eq!(recovered["ok"], true, "{recovered}");
        assert_eq!(
            recovered["result"]["counters"],
            observed["result"]["counters"]
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emergency_output_revoke_is_synchronous_without_router() {
    let mut d = Daemon::with(
        0x93,
        Opts {
            action: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    // Real guest action output is physically received BEFORE the revoke.
    let (mut link, act) = d.execute_action(&a).await;
    assert!(
        put(
            &mut link,
            "accept",
            json!({"id":act["body"]["action"]["id"],
        "correlation_id":act["body"]["action"]["correlation_id"],"accepted":true})
        )
        .await
    );
    // A real SQLite exclusive writer makes receiving-side authority unavailable.
    let db = rusqlite::Connection::open(d.root.join("authority.sqlite")).unwrap();
    db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
    let start = tokio::time::Instant::now();
    let revoked = d.local("revoke_output_lease", json!({})).await;
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "revoke waited for SQL: {revoked}"
    );
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed");
    assert_eq!(revoked["result"]["in_flight_drained"], true, "{revoked}");
    db.execute_batch("ROLLBACK;").unwrap();
    // The guest KEEPS running after revoke (observation continues) and keeps
    // producing actions, but not one further byte reaches the adapter.
    let queued = queued_bytes(&link);
    let step = d.step().await;
    for seq in 3..=6 {
        let _ = feed(&mut link, &d.surface, seq).await;
    }
    restart_adapter::step_at_least(&mut d.env, step + 1).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(queued_bytes(&link), queued, "action exposed after revoke");
    let mut raw = vec![0; queued];
    link.read_exact(&mut raw).await.unwrap();
    whole_frames(&raw);
    // The profile fences ALL adapter frames, including a new hello reply.
    let mut probe = UnixStream::connect(&d.socket).await.unwrap();
    let hello=coglet_runtime::cbor(&json!({"type":"hello","body":{"surface":d.surface,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"d".repeat(32)}})).unwrap();
    probe.write_u32(hello.len() as u32).await.unwrap();
    probe.write_all(&hello).await.unwrap();
    let mut byte = [0];
    let result = tokio::time::timeout(Duration::from_millis(300), probe.read(&mut byte)).await;
    assert!(
        !matches!(result,Ok(Ok(n)) if n>0),
        "frame exposed after revoke"
    );
    drop(link);
    d.restart().await;
    assert_eq!(
        d.local("observe_health", d.pins(&a)).await["result"]["output_lease_state"],
        "closed"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emergency_stop_reconciles_original_context_after_restart() {
    let mut d = Daemon::new(0x94).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let revoked = d.local("revoke_output_lease", json!({})).await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    drop(link);
    let records = d.restart().await;
    let mut wrong = d.frame("reconcile_emergency_stop", json!({}));
    wrong["ticket"] = json!(TICKET);
    assert_eq!(d.raw(wrong).await["error"]["code"], "permission_denied");
    // Held pending-stop authority: a real exclusive SQL writer makes the original
    // stop unknowable. Reconcile fails retryably, applies nothing, and the
    // exact original obligation remains available afterwards.
    let held = d.hold_authority();
    let unavailable = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(unavailable["ok"], false, "{unavailable}");
    assert_eq!(unavailable["error"]["retryable"], true, "{unavailable}");
    held.execute_batch("ROLLBACK;").unwrap();
    drop(held);
    let unapplied = receipt_of(&mut d.env, &correlation(0xeeee)).await;
    assert!(
        unapplied["type"] == "error" || unapplied["result"]["state"] != "applied",
        "held reconcile applied the stop: {unapplied}"
    );
    // Other principal fields are equally bound; only the retained original runs.
    let mut wrong_actor = d.frame("reconcile_emergency_stop", json!({}));
    wrong_actor["actor"] = json!(d.env.other_actor);
    assert_eq!(
        d.raw(wrong_actor).await["error"]["code"],
        "permission_denied"
    );
    let reconciled = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(reconciled["ok"], true, "{reconciled}");
    let repeated = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(
        repeated["result"], reconciled["result"],
        "same original is idempotent"
    );
    assert_eq!(reconciled["result"]["state"], "stopped");
    assert_eq!(reconciled["result"]["operation_id"], correlation(0xeeee));
    let receipt = receipt_of(&mut d.env, &correlation(0xeeee)).await;
    assert_eq!(receipt["result"]["state"], "applied", "{receipt}");
    let other = d.env.request_with(
        "hub.receipt",
        correlation(0xffff),
        json!({"operation_id":correlation(0xeeee)}),
        &d.env.actor,
        Some(OTHER_TICKET.into()),
    );
    assert_eq!(d.env.call(other).await["type"], "error");
    assert_eq!(
        d.env.simple("hub.status").await["result"]["state"],
        "stopped"
    );
    assert_eq!(
        d.local("observe_health", d.pins(&a)).await["result"]["output_lease_state"],
        "closed"
    );
    d.child.kill_and_wait();
    let store = coglet_runtime::DurableStore::open(&d.wal()).unwrap();
    assert_eq!(
        store.records().len(),
        records.len(),
        "reconciliation must not execute guest steps"
    );
}
