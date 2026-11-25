// Included by policy_producers.rs: additional real-daemon qualification cases.

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn guest_action_output_races_revoke_without_exposure_after_return() {
    let mut d = Daemon::with(
        0x97,
        Opts {
            action: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    // The real guest's action is physically received BEFORE any revoke.
    let (link, first) = d.execute_action(&a).await;
    let first_id = first["body"]["action"]["id"].as_str().unwrap().to_owned();
    let (mut read, mut write) = link.into_split();
    let surface = d.surface.clone();
    // The guest keeps executing and producing actions while revoke arrives.
    let writer = tokio::spawn(async move {
        for seq in 3..=120 {
            if !feed(&mut write, &surface, seq).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
        write
    });
    let running = d.step().await;
    restart_adapter::step_at_least(&mut d.env, running + 1).await;
    let started = tokio::time::Instant::now();
    let revoked = d.local("revoke_output_lease", json!({})).await;
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "unbounded revoke: {revoked}"
    );
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed");
    assert_eq!(revoked["result"]["lease"], "revoked", "{revoked}");
    assert_eq!(revoked["result"]["in_flight_drained"], true, "{revoked}");
    // Everything physically exposed was written before revoke returned: the
    // client never reads while racing, so kernel-queued bytes are exact.
    let queued = queued_bytes(read.as_ref() as &UnixStream);
    let at_revoke = d.step().await;
    let write = writer.await.unwrap();
    restart_adapter::step_at_least(&mut d.env, at_revoke + 1).await;
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(
        queued_bytes(read.as_ref() as &UnixStream),
        queued,
        "bytes were exposed after revoke returned"
    );
    let mut raw = vec![0; queued];
    read.read_exact(&mut raw).await.unwrap();
    let frames = whole_frames(&raw);
    let acts: Vec<_> = frames.iter().filter(|f| f["type"] == "act").collect();
    assert!(
        !acts.is_empty(),
        "no guest output crossed before revoke: {frames:?}"
    );
    assert!(
        acts.iter().all(|f| f["body"]["action"]["id"] != first_id),
        "duplicate output"
    );
    drop((read, write));
    d.restart().await;
    assert_eq!(d.health(&a).await["result"]["output_lease_state"], "closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn failed_revoke_wal_write_leaves_original_stop_unresolved_until_reconciled() {
    let mut d = Daemon::with(
        0x98,
        Opts {
            action: true,
            shim: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(&action_wat(&d.env.coglet)).await;
    let (mut link, _) = d.execute_action(&a).await;
    let (arm, hits) = (d.root.join("fault-arm"), d.root.join("fault-hits"));
    assert!(!hits.exists(), "shim fired before it was armed");
    // Arm the exact WAL-file write shim: the stop-pending frame cannot land.
    std::fs::write(&arm, b"armed").unwrap();
    let revoked = d.local("revoke_output_lease", json!({})).await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    assert_eq!(revoked["result"]["gate"], "closed");
    assert!(
        matches!(
            revoked["result"]["durable"].as_str(),
            Some("pending" | "uncertain")
        ),
        "a failed record must never be reported recorded: {revoked}"
    );
    // A pending record runs on a worker once the runtime mutex frees; its
    // injected failure must turn the sticky gate state into `uncertain`.
    let mut last = (false, Value::Null);
    let settled = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let injected = std::fs::metadata(&hits)
                .map(|m| m.len() > 0)
                .unwrap_or(false);
            let sample = d.health(&a).await;
            let state = sample["result"]["output_lease_state"].clone();
            let failed = sample["ok"] == false;
            last = (injected, sample);
            if injected && (state == "uncertain" || failed) {
                break;
            }
            assert_ne!(state, "enabled", "failed revoke left the lease enabled");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "failed WAL write never surfaced as uncertain custody: {last:?}"
    );
    assert!(
        std::fs::metadata(&hits)
            .map(|m| m.len() > 0)
            .unwrap_or(false),
        "the exact WAL write shim never injected a failure"
    );
    // The service is fenced by the failed write: health is unavailable (fail
    // closed) or reports an uncertain lease, never an enabled one.
    let health = d.health(&a).await;
    assert!(
        health["ok"] == false && health["error"]["code"] == "unavailable"
            || health["result"]["output_lease_state"] == "uncertain",
        "{health}"
    );
    let mut params = d.pins(&a);
    params["daemon_instance"] = revoked["result"]["daemon_instance"].clone();
    assert_eq!(
        d.local("enable_output_lease", params).await["ok"],
        false,
        "uncertain is sticky"
    );
    // No further physical output after the failed revoke returned.
    let queued = queued_bytes(&link);
    for seq in 3..=5 {
        let _ = feed(&mut link, &d.surface, seq).await;
    }
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(queued_bytes(&link), queued, "output after failed revoke");
    std::fs::remove_file(&arm).unwrap();
    drop(link);
    d.restart().await;
    // The failed record is indistinguishable from "never revoked": the
    // retained obligation is unresolved and a new lease must be refused.
    applied(&d.env.simple("hub.start").await);
    let attested = d.local("attest_applied", d.pins(&a)).await;
    assert_eq!(attested["ok"], true, "{attested}");
    let mut params = d.pins(&a);
    params["daemon_instance"] = attested["daemon_instance"].clone();
    let refused = d.local("enable_output_lease", params.clone()).await;
    assert_eq!(producer_code(&refused), "conflict", "{refused}");
    let reconciled = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(reconciled["ok"], true, "{reconciled}");
    assert_eq!(reconciled["result"]["operation_id"], correlation(0xeeee));
    let receipt = receipt_of(&mut d.env, &correlation(0xeeee)).await;
    assert_eq!(receipt["result"]["state"], "applied", "{receipt}");
    // The resolved ID is spent: re-enabling under the same stop ID is refused.
    applied(&d.env.simple("hub.start").await);
    let again = d.local("enable_output_lease", params).await;
    assert_eq!(producer_code(&again), "conflict", "{again}");
    assert_eq!(d.health(&a).await["result"]["output_lease_state"], "closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn queued_code_change_cannot_advance_attestation_before_cut() {
    let mut d = Daemon::with(
        0x9b,
        Opts {
            barrier: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let before = d.local("attest_applied", d.pins(&a)).await;
    assert_eq!(before["ok"], true, "{before}");
    let barrier = d.barrier.clone().unwrap();
    std::fs::write(&barrier.arm, b"armed").unwrap();
    let b = d.env.bundle(2, Some(&a.tip), &[("revision", b"B")]);
    let (push_id, pushing) = begin_push(&mut d.env, &b);
    tokio::time::timeout(Duration::from_secs(20), async {
        while !barrier.entered.exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("post-admission validation barrier");
    let queued = result_of(&receipt_of(&mut d.env, &push_id).await);
    assert_eq!(queued["state"], "queued", "{queued}");
    // Durably admitted but before the cut: producers see only generation 1.
    assert_eq!(d.local("attest_applied", d.pins(&b)).await["ok"], false);
    assert_eq!(d.health(&b).await["ok"], false);
    let still = d.local("attest_applied", d.pins(&a)).await;
    assert_eq!(still["ok"], true, "{still}");
    assert_eq!(still["result"], before["result"]);
    assert_eq!(
        d.env.simple("hub.status").await["result"]["live_commit"],
        a.tip
    );
    std::fs::write(&barrier.release, b"release").unwrap();
    assert_eq!(pushing.await.unwrap()["result"]["state"], "applied");
    let after = d.local("attest_applied", d.pins(&b)).await;
    assert_eq!(after["ok"], true, "{after}");
    assert_eq!(after["result"]["generation"], 2);
    assert_eq!(after["result"]["code"]["operation_id"], push_id);
    assert_eq!(d.local("attest_applied", d.pins(&a)).await["ok"], false);
    drop(link);
    d.restart().await;
    assert_eq!(
        d.local("attest_applied", d.pins(&b)).await["result"],
        after["result"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn changed_clock_and_overflow_fail_closed_after_real_wal_reopen() {
    let mut d = Daemon::with(
        0x99,
        Opts {
            shim: true,
            ..Opts::default()
        },
    )
    .await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let baseline = d.health(&a).await;
    assert_eq!(baseline["ok"], true, "{baseline}");
    drop(link);
    let uptime: u64 = std::fs::read_to_string("/proc/uptime")
        .unwrap()
        .split('.')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let clock = d.root.join("fault-clock");
    for (label, fault) in [
        (
            "rollback before the durable anchor",
            format!("sub {}", uptime.saturating_sub(1)),
        ),
        (
            "microsecond overflow",
            "set 18446744073709 999999999".to_owned(),
        ),
        ("seconds overflow", format!("set {} 0", i64::MAX)),
    ] {
        d.restart_with(|_| std::fs::write(&clock, &fault).unwrap())
            .await;
        let sample = d.health(&a).await;
        assert_eq!(sample["ok"], false, "{label} produced a sample: {sample}");
        if d.env.simple("hub.start").await["result"]["state"] == "applied" {
            let attested = d.local("attest_applied", d.pins(&a)).await;
            let mut params = d.pins(&a);
            params["daemon_instance"] = attested["daemon_instance"].clone();
            let enabled = d.local("enable_output_lease", params).await;
            assert_eq!(enabled["ok"], false, "{label} granted a lease: {enabled}");
        }
    }
    d.restart_with(|_| std::fs::remove_file(&clock).unwrap())
        .await;
    let recovered = d.health(&a).await;
    assert_eq!(recovered["ok"], true, "{recovered}");
    assert_eq!(
        recovered["result"]["counters"],
        baseline["result"]["counters"]
    );
    assert_eq!(recovered["result"]["output_lease_state"], "closed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_uid_principal_and_stop_id_collision_are_refused_by_real_daemon() {
    let mut d = Daemon::new(0x9a).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let attested = d.local("attest_applied", d.pins(&a)).await;
    let code_id = attested["result"]["initial"]["code"]["operation_id"].clone();
    let install_id = attested["result"]["initial"]["install"]["operation_id"].clone();
    drop(link);
    // Real configured daemon: this process's UID is not the configured UID.
    let wrong_uid = unsafe { libc::geteuid() } + 1;
    d.restart_with(|c| c["profile"]["principals"][0]["uid"] = json!(wrong_uid))
        .await;
    let mut stream = UnixStream::connect(d.private.path().join("p"))
        .await
        .unwrap();
    let frame = d.frame("attest_applied", d.pins(&a));
    let mut bytes = serde_json::to_vec(&frame).unwrap();
    bytes.push(b'\n');
    let _ = stream.write_all(&bytes).await;
    let mut reply = vec![];
    let read = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut reply)).await;
    assert!(read.is_ok(), "wrong-UID peer was kept open");
    assert!(
        reply.is_empty(),
        "wrong-UID peer received data: {:?}",
        String::from_utf8_lossy(&reply)
    );
    // Configured UID restored: a different principal name is still refused.
    let euid = unsafe { libc::geteuid() };
    d.restart_with(|c| c["profile"]["principals"][0]["uid"] = json!(euid))
        .await;
    let mut other = d.frame("attest_applied", d.pins(&a));
    other["principal"] = json!("intruder");
    let refused = d.raw(other).await;
    assert_eq!(producer_code(&refused), "permission_denied", "{refused}");
    assert_eq!(refused["nonce"], Value::Null, "{refused}");
    // The configured stop ID may not collide with any prior operation ID.
    for colliding in [code_id, install_id] {
        d.restart_with(|c| {
            c["profile"]["principals"][0]["stop"]["operation_id"] = colliding.clone()
        })
        .await;
        applied(&d.env.simple("hub.start").await);
        let attested = d.local("attest_applied", d.pins(&a)).await;
        assert_eq!(attested["ok"], true, "{attested}");
        let mut params = d.pins(&a);
        params["daemon_instance"] = attested["daemon_instance"].clone();
        let enabled = d.local("enable_output_lease", params).await;
        assert_eq!(
            enabled["ok"], false,
            "colliding stop ID {colliding} granted a lease: {enabled}"
        );
        assert_eq!(d.health(&a).await["result"]["output_lease_state"], "closed");
    }
}
