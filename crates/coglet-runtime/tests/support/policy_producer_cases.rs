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
    // Sample before starting the finite event stream; sampling afterward can
    // accidentally wait for work beyond its end.
    let running = d.step().await;
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
    restart_adapter::step_at_least(&mut d.env, running + 1).await;
    // A committed step can precede its socket publication. Require queued
    // physical output without consuming it before racing the revoke; otherwise
    // a correct revoke can win before ANY second action reaches this socket.
    // The final whole-frame/nonempty-act checks still validate these bytes.
    tokio::time::timeout(Duration::from_secs(10), async {
        while queued_bytes(read.as_ref() as &UnixStream) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("no post-first guest output was physically queued before revoke");
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
    let write = writer.await.unwrap();
    // A revoked publication can disconnect the adapter and discard pending
    // events. That is legal fail-closed behavior, not a promise of another
    // guest commit. The control above proves real guest output during the
    // race; here the oracle is physical non-publication, not session liveness.
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
    let revoked = d.reserved("revoke_output_lease", json!({})).await;
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn all_five_daemon_methods_return_shared_schema_conforming_results() {
    // Every reply on this path went through `Daemon::conform` (shared closed
    // schema + envelope echo + pins + counter reconciliation) inside `raw`.
    let mut d = Daemon::new(0x9c).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await; // attest_applied + enable_output_lease
    let health = d.health(&a).await;
    assert_eq!(health["ok"], true, "{health}");
    let revoked = d.local("revoke_output_lease", json!({})).await;
    assert_eq!(revoked["ok"], true, "{revoked}");
    drop(link);
    d.restart().await;
    let reconciled = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(reconciled["ok"], true, "{reconciled}");
    assert_eq!(reconciled["result"]["state"], "stopped");
    // Negative controls: the validator rejects mutations of REAL daemon replies.
    let schema: Value = serde_json::from_str(reply_schema::SCHEMA).unwrap();
    assert!(reply_schema::valid(&schema, &schema, &health));
    for mutate in [
        |r: &mut Value| r["result"]["unexpected"] = json!(1),
        |r: &mut Value| r["result"]["counters"]["committed"] = json!(-1),
        |r: &mut Value| r["method"] = json!("promote_generation"),
        |r: &mut Value| r["result"]["output_lease_state"] = json!("open"),
    ] {
        let mut forged = health.clone();
        mutate(&mut forged);
        assert!(!reply_schema::valid(&schema, &schema, &forged), "{forged}");
    }
    let mut seen = d.seen.lock().unwrap().clone();
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen,
        [
            "attest_applied",
            "enable_output_lease",
            "observe_health",
            "reconcile_emergency_stop",
            "revoke_output_lease"
        ],
        "a method never produced a validated daemon success"
    );
}

fn wal_sequence(wal: &Path) -> (u64, usize) {
    let store = coglet_runtime::DurableStore::open(wal).unwrap();
    let sequence = store.state().unwrap()["managed"]["policy"]["observation_sequence"]
        .as_u64()
        .unwrap();
    (sequence, store.records().len())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn order_exhaustion_after_real_wal_reopen_refuses_without_wrapping() {
    let mut d = Daemon::new(0x9d).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let baseline = d.health(&a).await;
    assert_eq!(baseline["ok"], true, "{baseline}");
    drop(link);
    // The sequence is an independent durable order domain (not derived from
    // journal length), so a legitimate near-limit frame is seeded through the real
    // DurableStore after the daemon is SIGKILLed and reaped; recovery must admit it.
    d.restart_full(
        |_| {},
        |store| {
            let mut state = store.state().unwrap().clone();
            let sequence = state
                .pointer_mut("/managed/policy/observation_sequence")
                .unwrap();
            assert!(sequence.as_u64().unwrap() < 100);
            *sequence = json!(u64::MAX - 1);
            store.commit(&state, None).unwrap();
        },
    )
    .await;
    let last = d.health(&a).await;
    assert_eq!(last["ok"], true, "recovery rejected the seeded frame: {last}");
    assert_eq!(last["result"]["observation_sequence"], u64::MAX);
    assert_eq!(last["result"]["counters"], baseline["result"]["counters"]);
    for _ in 0..2 {
        let refused = d.health(&a).await;
        assert_eq!(producer_code(&refused), "resource_exhausted", "{refused}");
    }
    // Other methods are not poisoned by an exhausted observation order.
    assert_eq!(d.local("attest_applied", d.pins(&a)).await["ok"], true);
    // Real reopen again: still exhausted, never wrapped back to a smaller value.
    let records = d.restart().await;
    let again = d.health(&a).await;
    assert_eq!(producer_code(&again), "resource_exhausted", "{again}");
    d.child.kill_and_wait();
    assert_eq!(
        wal_sequence(&d.wal()),
        (u64::MAX, records.len()),
        "exhausted order wrapped or executed a guest"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forged_near_limit_counters_are_refused_by_real_wal_recovery() {
    // Counters are a pure function of retained journal records after the
    // activation offset (policy_validate), so near-u64::MAX counter exhaustion
    // cannot be legitimately seeded: it would need ~2^64 journal records. This
    // control proves the forged alternative is NOT admitted, which is why counter
    // exhaustion remains unit-tested only (policy_counter_exhaustion_refuses_before_entry).
    let mut d = Daemon::new(0x9f).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    drop(link);
    d.child.kill_and_wait();
    {
        let mut store = coglet_runtime::DurableStore::open(&d.wal()).unwrap();
        let mut state = store.state().unwrap().clone();
        *state
            .pointer_mut("/managed/policy/counters/attempted")
            .unwrap() = json!(u64::MAX);
        store.commit(&state, None).unwrap();
    }
    if d.socket.exists() {
        std::fs::remove_file(&d.socket).unwrap();
    }
    d.config["invite"] = Value::Null;
    let (path, log) = (d.root.join("router.json"), d.root.join("forged.log"));
    std::fs::write(&path, serde_json::to_vec(&d.config).unwrap()).unwrap();
    let mut child = spawn_child(&path, &log, &d.child_env);
    let status = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("daemon kept serving forged near-limit counters");
    assert_eq!(status.code(), Some(1), "{status}");
    let log = std::fs::read_to_string(&log).unwrap();
    assert!(
        log.contains("recovered policy counters differ from actual boundaries"),
        "{log}"
    );
    assert!(!d.private.path().join("p").exists() || UnixStream::connect(d.private.path().join("p")).await.is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_applied_but_custody_unrecorded_reconciles_once_and_refuses_id_reuse() {
    // Unknown stop outcome: the original hub.stop committed its receipt, but the
    // daemon was SIGKILLed before the emergency custody record moved to Stopped.
    let mut d = Daemon::new(0x9e).await;
    let a = d.install(WASM_WAT).await;
    let link = d.execute(&a).await;
    let actor = d.env.actor.clone();
    let stop = |d: &Daemon, ticket: &str| {
        d.env.request_with(
            "hub.stop",
            correlation(0xeeee),
            json!({}),
            &actor,
            Some(ticket.into()),
        )
    };
    let original = stop(&d, TICKET);
    let stopped = d.env.call(original).await;
    assert_eq!(applied(&stopped)["operation_id"], correlation(0xeeee));
    // The retained ID cannot be borrowed by another ticket/scope.
    let reused = stop(&d, OTHER_TICKET);
    let refused = d.env.call(reused).await;
    assert_eq!(refused["type"], "error", "{refused}");
    drop(link);
    let records = d.restart().await;
    assert_eq!(d.env.simple("hub.status").await["result"]["state"], "stopped");
    // Receipt exists and is applied, but custody still holds the unresolved
    // obligation until reconciliation: a new lease must be refused.
    let attested = d.local("attest_applied", d.pins(&a)).await;
    let mut params = d.pins(&a);
    params["daemon_instance"] = attested["daemon_instance"].clone();
    assert_eq!(d.local("enable_output_lease", params).await["ok"], false);
    let receipt = receipt_of(&mut d.env, &correlation(0xeeee)).await;
    assert_eq!(receipt["result"]["state"], "applied", "{receipt}");
    let reconciled = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(reconciled["ok"], true, "{reconciled}");
    assert_eq!(reconciled["result"]["state"], "stopped");
    assert_eq!(reconciled["result"]["operation_id"], correlation(0xeeee));
    assert_eq!(
        receipt_of(&mut d.env, &correlation(0xeeee)).await["result"],
        receipt["result"],
        "reconciliation must not mint a second stop effect"
    );
    let again = d.local("reconcile_emergency_stop", json!({})).await;
    assert_eq!(again["result"], reconciled["result"]);
    let reused = stop(&d, OTHER_TICKET);
    assert_eq!(d.env.call(reused).await["type"], "error");
    d.child.kill_and_wait();
    let store = coglet_runtime::DurableStore::open(&d.wal()).unwrap();
    assert_eq!(store.records().len(), records.len(), "guest entered");
}
