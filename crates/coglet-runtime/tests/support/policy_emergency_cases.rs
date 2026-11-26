// Included by policy_producers.rs: ADR-0043 reserved emergency admission and
// bounded revoke recording against the real cogletd (real WAMR guest, real WAL
// reopen after SIGKILL). The only seam is the subprocess-local LD_PRELOAD shim's
// `stall` mode, which delays one genuine WAL write; nothing is faked or failed.

/// One frame on `path`; `None` when the daemon closed without any reply.
async fn exchange_on(path: &Path, frame: &Value) -> Option<Value> {
    let mut stream = UnixStream::connect(path).await.ok()?;
    let mut bytes = serde_json::to_vec(frame).unwrap();
    bytes.push(b'\n');
    let _ = stream.write_all(&bytes).await;
    let mut line = String::new();
    let read = tokio::time::timeout(
        Duration::from_secs(8),
        BufReader::new(stream).read_line(&mut line),
    )
    .await
    .expect("neither a reply nor a close from the producer");
    match read {
        Ok(n) if n > 0 => Some(serde_json::from_str(&line).unwrap()),
        _ => None,
    }
}

fn is_socket_0600(path: &Path) -> bool {
    use std::os::unix::fs::FileTypeExt;
    std::fs::symlink_metadata(path)
        .is_ok_and(|m| m.file_type().is_socket() && m.permissions().mode() & 0o777 == 0o600)
}

async fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(15), async {
        while !ready() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("timed out waiting for {what}"));
}

impl Daemon {
    fn ordinary(&self) -> PathBuf {
        self.private.path().join("p")
    }
    /// ADR-0043 discovery rule: the literal suffix on the complete socket path.
    fn lane(&self) -> PathBuf {
        self.private.path().join("p.revoke")
    }
    /// A request on the reserved `.revoke` endpoint; the reply (success or
    /// failure) must satisfy the same shared schema as the ordinary endpoint.
    async fn reserved_raw(&self, frame: Value) -> Value {
        let reply = exchange_on(&self.lane(), &frame)
            .await
            .expect("reserved endpoint closed without a reply");
        self.conform(&frame, &reply);
        reply
    }
    async fn reserved(&mut self, method: &str, params: Value) -> Value {
        let frame = self.frame(method, params);
        self.reserved_raw(frame).await
    }
    /// A valid authenticated ordinary request that the saturated pool must drop.
    async fn ordinary_dropped(&mut self, bundle: &Bundle) -> bool {
        let frame = self.frame("observe_health", self.pins(bundle));
        exchange_on(&self.ordinary(), &frame).await.is_none()
    }
    /// Deferred revoke recorder threads alive in the real daemon (Linux thread
    /// names are truncated to 15 bytes).
    fn recorders(&self) -> usize {
        std::fs::read_dir(format!("/proc/{}/task", self.child.0.id()))
            .unwrap()
            .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
            .filter(|comm| comm.trim_end() == &"cogletd-revoke-record"[..15])
            .count()
    }
    fn stall_files(&self) -> (PathBuf, PathBuf) {
        (
            self.root.join("fault-stall"),
            self.root.join("fault-stalled"),
        )
    }
    /// After a restart the retained original stop still fences a new lease, and
    /// only the original context resolves it.
    async fn assert_retained_stop_after_restart(&mut self, bundle: &Bundle) {
        assert_eq!(
            self.health(bundle).await["result"]["output_lease_state"],
            "closed",
            "boot-closed output"
        );
        applied(&self.env.simple("hub.start").await);
        let attested = self.local("attest_applied", self.pins(bundle)).await;
        assert_eq!(attested["ok"], true, "{attested}");
        let mut params = self.pins(bundle);
        params["daemon_instance"] = attested["daemon_instance"].clone();
        let refused = self.local("enable_output_lease", params).await;
        assert_eq!(producer_code(&refused), "conflict", "{refused}");
        let unapplied = receipt_of(&mut self.env, &correlation(0xeeee)).await;
        assert!(
            unapplied["type"] == "error" || unapplied["result"]["state"] != "applied",
            "stop applied without reconciliation: {unapplied}"
        );
        let reconciled = self.local("reconcile_emergency_stop", json!({})).await;
        assert_eq!(reconciled["ok"], true, "{reconciled}");
        assert_eq!(reconciled["result"]["state"], "stopped");
        assert_eq!(reconciled["result"]["operation_id"], correlation(0xeeee));
        let receipt = receipt_of(&mut self.env, &correlation(0xeeee)).await;
        assert_eq!(receipt["result"]["state"], "applied", "{receipt}");
    }
}

/// `blocked == false`: all 16 ordinary slots are held by idle and partial-frame
/// connections. `blocked == true`: all 16 are held by authenticated operations
/// that are inside the producer, waiting on runtime custody behind one stalled
/// genuine WAL write, while receiving-side SQL authority is exclusively held.
/// (SQL contention alone fails fast in this producer: there is no busy wait, so
/// it cannot by itself keep an ordinary slot occupied.)
async fn saturation_case(seed: u8, blocked: bool) {
    let mut d = Daemon::with(
        seed,
        Opts {
            action: true,
            shim: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    // Real guest-generated output is physically received BEFORE the revoke.
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
    assert!(is_socket_0600(&d.lane()) && is_socket_0600(&d.ordinary()));
    let (stall, stalled) = d.stall_files();
    let mut held = vec![];
    if blocked {
        std::fs::write(&stall, b"stall").unwrap();
        for _ in 0..16 {
            let frame = d.frame("observe_health", d.pins(&a));
            let mut stream = UnixStream::connect(d.ordinary()).await.unwrap();
            let mut bytes = serde_json::to_vec(&frame).unwrap();
            bytes.push(b'\n');
            stream.write_all(&bytes).await.unwrap();
            held.push((stream, Some(frame)));
        }
        // One of them (or a guest step) is inside its WAL write; every other
        // operation waits for the same runtime custody.
        wait_for("a stalled WAL write", || {
            std::fs::metadata(&stalled).is_ok_and(|m| m.len() > 0)
        })
        .await;
    } else {
        for i in 0..16 {
            let mut stream = UnixStream::connect(d.ordinary()).await.unwrap();
            if i % 2 == 0 {
                stream.write_all(b"{\"version\":1,").await.unwrap();
            }
            held.push((stream, None));
        }
    }
    // Occupancy oracle, no timing guess: accepts are FIFO, so a 17th valid
    // authenticated request is dropped only if all 16 slots are taken.
    assert!(
        d.ordinary_dropped(&a).await,
        "ordinary pool is not saturated"
    );
    let authority = d.hold_authority();
    let started = tokio::time::Instant::now();
    let revoked = d.reserved("revoke_output_lease", json!({})).await;
    let elapsed = started.elapsed();
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed", "{revoked}");
    assert_eq!(revoked["result"]["lease"], "revoked", "{revoked}");
    assert_eq!(revoked["result"]["in_flight_drained"], true, "{revoked}");
    // Custody held: the record is deferred. Otherwise it is written at once,
    // unless a guest step owns the runtime for that instant (then deferred).
    let durable = revoked["result"]["durable"].as_str().unwrap();
    assert!(
        durable == "pending" || !blocked && durable == "recorded",
        "{revoked}"
    );
    assert!(elapsed < Duration::from_secs(2), "revoke waited: {elapsed:?}");
    // Everything physically exposed was written before the revoke returned.
    let queued = queued_bytes(&link);
    // The blockers were still holding every ordinary slot when it returned.
    assert!(
        d.ordinary_dropped(&a).await,
        "ordinary capacity was free before the reserved revoke returned"
    );
    for (stream, _) in &held {
        assert_eq!(queued_bytes(stream), 0, "a blocker was answered early");
    }
    if blocked {
        assert!(stall.exists());
        assert_eq!(d.recorders(), 1, "exactly one deferred recorder");
    }
    assert_eq!(
        queued_bytes(&link),
        queued,
        "output exposed while blocked"
    );
    // Release the blockers only now.
    authority.execute_batch("ROLLBACK;").unwrap();
    drop(authority);
    if blocked {
        std::fs::remove_file(&stall).unwrap();
        for (stream, frame) in held {
            let mut line = String::new();
            tokio::time::timeout(
                Duration::from_secs(15),
                BufReader::new(stream).read_line(&mut line),
            )
            .await
            .expect("blocked operation never completed")
            .unwrap();
            let reply: Value = serde_json::from_str(&line).unwrap();
            let frame = frame.unwrap();
            d.conform(&frame, &reply);
            // It was a real authenticated operation, answered only after release.
            assert_eq!(reply["ok"], true, "{reply}");
            assert_eq!(reply["nonce"], frame["nonce"]);
            assert_ne!(reply["result"]["output_lease_state"], "enabled", "{reply}");
        }
    } else {
        drop(held);
    }
    wait_for("recorder slot reclamation", || d.recorders() == 0).await;
    // The guest may keep running; not one further byte reaches the adapter.
    // (A fenced publication may disconnect the adapter: liveness is no oracle.)
    for seq in 3..=6 {
        let _ = feed(&mut link, &d.surface, seq).await;
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(queued_bytes(&link), queued, "output exposed after revoke");
    let mut raw = vec![0; queued];
    link.read_exact(&mut raw).await.unwrap();
    whole_frames(&raw);
    let mut params = d.pins(&a);
    params["daemon_instance"] = revoked["result"]["daemon_instance"].clone();
    assert_eq!(
        d.local("enable_output_lease", params).await["ok"],
        false,
        "a revoked gate never re-enables in this process"
    );
    drop(link);
    d.restart().await;
    d.assert_retained_stop_after_restart(&a).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn emergency_output_revoke_survives_ordinary_admission_saturation() {
    saturation_case(0xa1, false).await;
    saturation_case(0xa2, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reserved_emergency_endpoint_rejects_non_revoke_and_forged_bindings() {
    let mut d = Daemon::with(
        0xa3,
        Opts {
            action: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    let (mut link, _) = d.execute_action(&a).await;
    let enabled = d.local("attest_applied", d.pins(&a)).await;
    let instance = enabled["daemon_instance"].clone();
    // No ordinary operation is dispatched on the reserved endpoint.
    let mut enable = d.pins(&a);
    enable["daemon_instance"] = instance.clone();
    for (method, params) in [
        ("attest_applied", d.pins(&a)),
        ("observe_health", d.pins(&a)),
        ("enable_output_lease", enable),
        ("reconcile_emergency_stop", json!({})),
    ] {
        let frame = d.frame(method, params);
        let reply = d.reserved_raw(frame.clone()).await;
        assert_eq!(producer_code(&reply), "invalid_argument", "{method}");
        assert_eq!(reply["nonce"], frame["nonce"], "{method}: authenticated");
    }
    // Forged bindings are refused exactly as on the ordinary endpoint.
    let other_actor = d.env.other_actor.clone();
    for forge in [
        (|f: &mut Value, other: &str| f["actor"] = json!(other)) as fn(&mut Value, &str),
        |f, _| f["ticket"] = json!(TICKET),
        |f, _| f["ticket"] = Value::Null,
        |f, _| f["principal"] = json!("root"),
        |f, _| f["target"] = json!("coglet:other"),
    ] {
        let mut frame = d.frame("revoke_output_lease", json!({}));
        forge(&mut frame, &other_actor);
        let reply = d.reserved_raw(frame.clone()).await;
        assert_eq!(producer_code(&reply), "permission_denied", "{frame}");
        assert_eq!(reply["nonce"], Value::Null);
    }
    // Malformed, unknown-field and over-size frames.
    let mut unknown = d.frame("revoke_output_lease", json!({}));
    unknown["extra"] = json!(1);
    assert_eq!(
        producer_code(&d.reserved_raw(unknown).await),
        "invalid_argument"
    );
    let mut params = d.frame("revoke_output_lease", json!({"force":true}));
    params["nonce"] = json!("p-1");
    assert_eq!(
        producer_code(&d.reserved_raw(params).await),
        "invalid_argument"
    );
    let oversize = json!("x".repeat(70_000));
    let reply = exchange_on(&d.lane(), &oversize).await.unwrap();
    assert_eq!(producer_code(&reply), "resource_exhausted", "{reply}");
    // Reserved capacity: four idle/partial connections exhaust it; a fifth
    // valid revoke is closed, never queued and never served elsewhere.
    let mut idle = vec![];
    for i in 0..4 {
        let mut stream = UnixStream::connect(d.lane()).await.unwrap();
        if i % 2 == 0 {
            stream.write_all(b"{\"version\":1,").await.unwrap();
        }
        idle.push(stream);
    }
    let fifth = d.frame("revoke_output_lease", json!({}));
    assert!(
        exchange_on(&d.lane(), &fifth).await.is_none(),
        "reserved pool admitted a fifth connection"
    );
    // After ALL of the above the lease is still enabled and the real guest
    // still reaches the adapter: nothing closed the gate or enabled anything.
    let health = d.health(&a).await;
    assert_eq!(health["result"]["output_lease_state"], "enabled", "{health}");
    assert!(feed(&mut link, &d.surface, 3).await);
    let act = wait_act(&mut link).await;
    assert_eq!(act["body"]["action"]["capability"], "speech.speak_chunk");
    // Each idle holder is closed without a reply at its 3 s request deadline,
    // which reclaims the slot for a subsequent valid revoke.
    let started = tokio::time::Instant::now();
    for mut stream in idle {
        let mut byte = [0u8; 1];
        let closed = tokio::time::timeout(Duration::from_secs(8), stream.read(&mut byte))
            .await
            .expect("idle reserved connection was never timed out");
        assert!(matches!(closed, Ok(0) | Err(_)), "reply to an idle holder");
    }
    assert!(started.elapsed() < Duration::from_secs(6));
    let mut revoked = None;
    for _ in 0..100 {
        let frame = d.frame("revoke_output_lease", json!({}));
        if let Some(reply) = exchange_on(&d.lane(), &frame).await {
            d.conform(&frame, &reply);
            revoked = Some(reply);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let revoked = revoked.expect("reserved slots were not reclaimed");
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed");
    assert_eq!(revoked["result"]["lease"], "revoked", "{revoked}");
    let queued = queued_bytes(&link);
    for seq in 4..=6 {
        let _ = feed(&mut link, &d.surface, seq).await;
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(queued_bytes(&link), queued, "output exposed after revoke");
    let mut params = d.pins(&a);
    params["daemon_instance"] = instance;
    assert_eq!(d.local("enable_output_lease", params).await["ok"], false);
    drop(link);
    d.restart().await;
    d.assert_retained_stop_after_restart(&a).await;
}

/// Native coverage of ADR-0043 recorder coalescing. Spawn-failure uncertainty
/// and the panic path are the deterministic unit seam
/// `producer_ops::recorder_tests`; they are not claimed here.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn repeated_revoke_coalesces_pending_recorder() {
    let mut d = Daemon::with(
        0xa4,
        Opts {
            action: true,
            shim: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    let (mut link, _) = d.execute_action(&a).await;
    assert_eq!(d.recorders(), 0);
    // Hold runtime custody: one genuine WAL write (a health observation's
    // sequence commit, or a guest step) stalls inside the daemon.
    let (stall, stalled) = d.stall_files();
    std::fs::write(&stall, b"stall").unwrap();
    let frame = d.frame("observe_health", d.pins(&a));
    let mut holder = UnixStream::connect(d.ordinary()).await.unwrap();
    let mut bytes = serde_json::to_vec(&frame).unwrap();
    bytes.push(b'\n');
    holder.write_all(&bytes).await.unwrap();
    wait_for("a stalled WAL write", || {
        std::fs::metadata(&stalled).is_ok_and(|m| m.len() > 0)
    })
    .await;
    let mut instance = Value::Null;
    for round in 0..24 {
        let revoked = if round % 2 == 0 {
            d.reserved("revoke_output_lease", json!({})).await
        } else {
            d.local("revoke_output_lease", json!({})).await
        };
        assert_eq!(revoked["ok"], true, "{revoked}");
        assert_eq!(revoked["result"]["gate"], "closed", "{revoked}");
        assert_eq!(revoked["result"]["lease"], "revoked", "{revoked}");
        assert_eq!(revoked["result"]["durable"], "pending", "{revoked}");
        // Repeated requests never spawn a second recorder for the target.
        assert_eq!(d.recorders(), 1, "round {round}");
        instance = revoked["result"]["daemon_instance"].clone();
    }
    assert!(stall.exists());
    assert_eq!(queued_bytes(&holder), 0, "custody was not held throughout");
    let queued = queued_bytes(&link);
    // Release custody: the one recorder writes the original stop-pending state
    // and its slot is reclaimed.
    std::fs::remove_file(&stall).unwrap();
    wait_for("recorder completion", || d.recorders() == 0).await;
    let mut line = String::new();
    tokio::time::timeout(
        Duration::from_secs(15),
        BufReader::new(holder).read_line(&mut line),
    )
    .await
    .unwrap()
    .unwrap();
    let held: Value = serde_json::from_str(&line).unwrap();
    d.conform(&frame, &held);
    assert_eq!(held["ok"], true, "{held}");
    // With custody free a further revoke records synchronously (a guest step
    // may still own the mutex for an instant: then it is one new recorder).
    let mut durable = Value::Null;
    for _ in 0..200 {
        let again = d.reserved("revoke_output_lease", json!({})).await;
        assert_eq!(again["result"]["lease"], "revoked", "{again}");
        assert!(d.recorders() <= 1);
        durable = again["result"]["durable"].clone();
        if durable == "recorded" {
            break;
        }
        assert_eq!(durable, "pending", "{again}");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(durable, "recorded", "slot was never reclaimed");
    // No implicit enable, in this process or from the recorded WAL state.
    let mut params = d.pins(&a);
    params["daemon_instance"] = instance;
    assert_eq!(d.local("enable_output_lease", params).await["ok"], false);
    assert_eq!(
        d.health(&a).await["result"]["output_lease_state"],
        "revoked"
    );
    for seq in 3..=5 {
        let _ = feed(&mut link, &d.surface, seq).await;
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(queued_bytes(&link), queued, "output exposed after revoke");
    drop(link);
    d.restart().await;
    d.assert_retained_stop_after_restart(&a).await;
}

impl Daemon {
    /// Spawn the real daemon with an edited copy of the config and require
    /// startup to FAIL (non-zero exit before any hub session). Returns its log.
    async fn failed_start(&self, edit: impl FnOnce(&mut Value)) -> String {
        let mut config = self.config.clone();
        edit(&mut config);
        let path = self.root.join("router-failing.json");
        std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
        let log = self.root.join("failing.log");
        let mut child = spawn_child(&path, &log, &self.child_env);
        let status = tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if let Some(status) = child.0.try_wait().unwrap() {
                    return status;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("daemon kept running with an unusable listener pair");
        assert!(!status.success(), "startup must fail closed");
        assert!(self.env.hub.session(self.device).await.is_none());
        std::fs::read_to_string(log).unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn producer_listener_pair_fails_closed_and_reopens() {
    let mut d = Daemon::new(0xa5).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let (ordinary, lane) = (d.ordinary(), d.lane());
    assert!(is_socket_0600(&ordinary) && is_socket_0600(&lane));
    drop(link);
    // SIGKILL leaves BOTH real sockets stale; the next start replaces the pair.
    let (o, l) = (ordinary.clone(), lane.clone());
    d.restart_full(
        |_| {},
        move |_| {
            for path in [&o, &l] {
                assert!(is_socket_0600(path), "stale socket missing");
                let refused = std::os::unix::net::UnixStream::connect(path).unwrap_err();
                assert_eq!(refused.kind(), std::io::ErrorKind::ConnectionRefused);
            }
        },
    )
    .await;
    assert!(is_socket_0600(&ordinary) && is_socket_0600(&lane));
    assert_eq!(d.health(&a).await["result"]["output_lease_state"], "closed");
    let revoked = d.reserved("revoke_output_lease", json!({})).await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed");
    // Fail-closed startup. Kill, wait for the old transport to retire.
    d.child.kill_and_wait();
    tokio::time::timeout(Duration::from_secs(50), async {
        while d.env.hub.session(d.device).await.is_some() {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("old SIGKILL transport retires");
    d.config["invite"] = Value::Null;
    let dead = |path: &Path| std::os::unix::net::UnixStream::connect(path).is_err();
    // (1) A non-socket at the sibling: refused, left byte-identical, and the
    // stale ordinary socket is not replaced by a live one.
    std::fs::remove_file(&lane).unwrap();
    std::fs::write(&lane, b"foreign").unwrap();
    let log = d.failed_start(|_| {}).await;
    assert!(log.contains("non-socket or foreign"), "{log}");
    assert_eq!(std::fs::read(&lane).unwrap(), b"foreign");
    assert!(dead(&ordinary), "ordinary listener started alone");
    std::fs::remove_file(&lane).unwrap();
    // (2) A LIVE listener at the sibling is never displaced.
    let live = std::os::unix::net::UnixListener::bind(&lane).unwrap();
    let log = d.failed_start(|_| {}).await;
    assert!(log.contains("live listener"), "{log}");
    assert!(dead(&ordinary), "ordinary listener started alone");
    std::os::unix::net::UnixStream::connect(&lane).expect("live sibling was displaced");
    drop(live);
    std::fs::remove_file(&lane).unwrap();
    // (3) A non-socket at the ORDINARY path with a free sibling: no sibling
    // listener is left behind.
    std::fs::remove_file(&ordinary).unwrap();
    std::fs::write(&ordinary, b"foreign").unwrap();
    d.failed_start(|_| {}).await;
    assert!(!lane.exists(), "reserved listener started alone");
    std::fs::remove_file(&ordinary).unwrap();
    // (4) Suffix-length overflow: the ordinary path fits, `<socket>.revoke`
    // does not. No listener is created or truncated.
    // SAFETY: plain old data; only the field size is read.
    let bound = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
        .sun_path
        .len();
    let base = d.private.path().to_owned();
    let long = base.join("s".repeat(bound - 2 - base.as_os_str().len()));
    assert_eq!(long.as_os_str().len(), bound - 1);
    let log = d
        .failed_start(|config| config["profile"]["socket"] = json!(long))
        .await;
    assert!(log.contains("exceeds the Unix socket bound"), "{log}");
    assert!(!long.exists() && !ordinary.exists() && !lane.exists());
    // (5) Absent profile stays compatible: the daemon runs with NEITHER socket.
    let profile = d.config["profile"].take();
    let path = d.root.join("router.json");
    std::fs::write(&path, serde_json::to_vec(&d.config).unwrap()).unwrap();
    let log = d.root.join("restart.log");
    d.child = spawn_child(&path, &log, &d.child_env);
    d.env.session = child_session(&d.env.hub, d.device, &mut d.child, &log).await;
    assert_eq!(d.env.simple("hub.status").await["type"], "reply");
    assert!(!ordinary.exists() && !lane.exists());
    // (6) Reopen with the profile: the pair is back and the retained original
    // stop from the first process is still the only way to resolve custody.
    d.restart_with(|config| config["profile"] = profile).await;
    assert!(is_socket_0600(&ordinary) && is_socket_0600(&lane));
    d.assert_retained_stop_after_restart(&a).await;
}
