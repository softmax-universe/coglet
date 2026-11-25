//! Receiver tests share only the real encrypted-iroh lifecycle fixture. They
//! invoke production DeviceHandler and independently reopen its WAL after retire.
use super::*;
#[path = "receiver_fetch_tests.rs"]
mod fetch_tests;

async fn receiver_env(seed: u8) -> Env {
    receiver_env_guest(seed, WASM_WAT, 10000).await
}
async fn receiver_env_guest(seed: u8, wat: &str, compute: u64) -> Env {
    let mut options = Options {
        back_channel: true,
        extra_scopes: true,
        ..Options::default()
    };
    options.delegation_methods.extend([
        "debug.read",
        "debug.write",
        "hub.inbox",
        "hub.cancel",
        "debug.pause",
        "debug.step",
        "hub.actions",
        "hub.git.fetch",
    ]);
    let methods = options.delegation_methods.clone();
    let mut env = Env::new(seed, options).await.unwrap();
    let bundle = env.bundle_guest(1, None, &[], wat, compute);
    assert_eq!(env.push(&bundle).await["type"], "reply");
    let delegation = Env::delegation_for(
        &env.coglet_key,
        &env.coglet,
        env.hub.id().to_string(),
        env.endpoint.id().to_string(),
        DELEGATION_GEN,
        DELEGATION_EXPIRES,
        &methods,
    );
    assert_eq!(
        env.install(&bundle.signed, &delegation).await["type"],
        "reply"
    );
    env
}

fn initial_hash(env: &Env) -> String {
    let service = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    let setup = service.lock().unwrap().managed_setup().clone();
    let files = serde_json::from_value(setup["initial_files"].clone()).unwrap();
    coglet_runtime::Snapshot::new(files)
        .unwrap()
        .tree_hash()
        .unwrap()
}

fn intervention(env: &mut Env, method: &str, mut params: Value) -> Value {
    params["actor"] = json!(env.actor);
    params["ticket"] = json!(env.ticket);
    let mut request = env.request(method, params);
    request.as_object_mut().unwrap().remove("context");
    request
}

async fn terminal(env: &mut Env, operation: &str) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let request = env.request("hub.receipt", json!({"operation_id":operation}));
            let reply = env.call(request).await;
            assert_eq!(reply["type"], "reply", "{reply}");
            if reply["result"]["state"] != "queued" {
                return reply["result"].clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("receiver reaches a durable terminal receipt")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_channel_real_receiver_write_read_receipt_and_restart() {
    let mut env = receiver_env(200).await;
    let hash = initial_hash(&env);
    let write = intervention(
        &mut env,
        "debug.write",
        json!({"path":"/data/notes/a","text":"private λ","expected_hash":hash}),
    );
    let op = write["correlation_id"].as_str().unwrap().to_owned();
    assert_eq!(env.call(write.clone()).await["result"]["state"], "applied");
    assert_eq!(terminal(&mut env, &op).await["state"], "applied");
    let read = env.request(
        "debug.read",
        json!({"path":"/data/notes/a","offset":0,"limit":65536}),
    );
    assert_eq!(env.call(read).await["result"]["text"], "private λ");
    env.restart_handler().await;
    let read = env.request(
        "debug.read",
        json!({"path":"/data/notes/a","offset":0,"limit":65536}),
    );
    assert_eq!(env.call(read).await["result"]["text"], "private λ");
    assert_eq!(env.call(write).await["result"]["state"], "applied");
    let other = env.request_with(
        "hub.receipt",
        correlation(900),
        json!({"operation_id":op}),
        &env.other_actor,
        Some(env.ticket.clone()),
    );
    assert_eq!(env.call(other).await["error"]["code"], "not_found");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_channel_real_receiver_scope_and_path_limits() {
    let mut env = receiver_env(201).await;
    let hash = initial_hash(&env);
    for path in ["/data/inbox.jsonl", "/data/notes/../other", "/data/outside"] {
        let write = intervention(
            &mut env,
            "debug.write",
            json!({"path":path,"text":"bad","expected_hash":hash}),
        );
        assert_eq!(
            env.call(write).await["error"]["code"],
            "permission_denied",
            "{path}"
        );
    }
    let write = intervention(
        &mut env,
        "debug.write",
        json!({"path":"/data/notes/a","text":"bad","expected_hash":"0".repeat(64)}),
    );
    assert_eq!(env.call(write).await["error"]["code"], "conflict");
    for (actor, ticket) in [
        (env.actor.clone(), None),
        (env.actor.clone(), Some(NARROW_TICKET.into())),
        (
            format!("hub:{}", key(201, 8).public()),
            Some(env.ticket.clone()),
        ),
    ] {
        let request = env.request_with(
            "debug.read",
            correlation(800),
            json!({"path":"/data/notes/a","offset":0,"limit":10}),
            &actor,
            ticket,
        );
        assert_eq!(
            env.call(request).await["error"]["code"],
            "permission_denied"
        );
    }
    let authority = Authority::open(env.root.join("authority.sqlite")).unwrap();
    authority
        .end_ticket(&Scope {
            actor: env.actor.clone(),
            ticket: Some(env.ticket.clone()),
        })
        .unwrap();
    let read = env.request(
        "debug.read",
        json!({"path":"/data/notes/a","offset":0,"limit":10}),
    );
    assert_eq!(env.call(read).await["error"]["code"], "permission_denied");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_channel_real_receiver_inbox_dedup_reopened_wal_oracle() {
    let mut env = receiver_env(202).await;
    let message = json!({"id":"a1".repeat(16),"sender":env.actor,"event":{"source":env.actor,"kind":"test.message","seq":1,
        "logical_time":0,"payload":{"note":"not synthesized"},"correlation_id":"b2".repeat(16)}});
    let request = intervention(&mut env, "hub.inbox", json!({"message":message}));
    let op = request["correlation_id"].as_str().unwrap().to_owned();
    assert_eq!(env.call(request).await["result"]["state"], "applied");
    assert_eq!(terminal(&mut env, &op).await["state"], "applied");
    env.restart_handler().await;
    let duplicate = intervention(&mut env, "hub.inbox", json!({"message":message}));
    let op = duplicate["correlation_id"].as_str().unwrap().to_owned();
    assert_eq!(env.call(duplicate).await["result"]["state"], "applied");
    assert_eq!(terminal(&mut env, &op).await["state"], "applied");
    let read = env.request(
        "debug.read",
        json!({"path":"/data/inbox.jsonl","offset":0,"limit":65536}),
    );
    let result = env.call(read).await;
    let text = result["result"]["text"].as_str().unwrap();
    let lines: Vec<Value> = text
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["index"], 0);
    assert_eq!(lines[0]["event"], message["event"]);
    let mut changed = message.clone();
    changed["event"]["payload"] = json!({"note":"changed"});
    let conflict = intervention(&mut env, "hub.inbox", json!({"message":changed}));
    assert_eq!(env.call(conflict).await["error"]["code"], "conflict");
    // Retire all handler custody and open the actual persisted WAL, not a reply
    // projection, for independent byte/receipt/audit verification.
    env.serve.abort();
    let _ = (&mut env.serve).await;
    drop(env.handler.take());
    let wal = env
        .config
        .wal_root
        .join(coglet_runtime::digest(env.coglet.as_bytes()))
        .join("state.wal");
    let store = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match coglet_runtime::DurableStore::open(&wal) {
                Ok(store) => break store,
                Err(e) if e.starts_with("WAL already owned:") => tokio::task::yield_now().await,
                Err(e) => panic!("{e}"),
            }
        }
    })
    .await
    .unwrap();
    let state = store.state().unwrap();
    assert_eq!(
        state["receiver"]["operations"].as_object().unwrap().len(),
        2
    );
    assert!(state["receiver"]["pending"].as_array().unwrap().is_empty());
    assert_eq!(state["receiver"]["audit"].as_array().unwrap().len(), 4);
    let raw = hex::decode(
        state["staged_boundary"]["runtime"]["files"]["/data/inbox.jsonl"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(std::str::from_utf8(&raw).unwrap(), text);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_channel_pause_is_durable_and_single_step_requires_pause() {
    let mut env = receiver_env(203).await;
    let step = env.simple("debug.step").await;
    assert_eq!(step["error"]["code"], "conflict");
    let pause = env.simple("debug.pause").await;
    assert_eq!(pause["result"]["state"], "applied", "{pause}");
    assert_eq!(env.simple("hub.status").await["result"]["state"], "paused");
    env.restart_handler().await;
    assert_eq!(env.simple("hub.status").await["result"]["state"], "paused");
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_bounded_guest_allows_durable_inbox_cancel_while_running() {
    // This guest actually enters handle and runs until its pinned deadline.
    // No host/fault hook fabricates the in-progress step or its observation.
    let wat = r#"(module (memory (export "memory") 1 2)
        (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
        (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
          (loop $busy br $busy) i32.const 0))"#;
    let mut env = receiver_env_guest(208, wat, 2_000_000_000).await;
    assert_eq!(
        env.simple("debug.pause").await["result"]["state"],
        "applied"
    );
    let step = env.request("debug.step", json!({}));
    let session = env.session.clone();
    let running = tokio::spawn(async move { session.request(step).await.unwrap() });
    let shared = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if shared
                .try_lock()
                .is_ok_and(|locked| locked.has_unresolved_attempt())
            {
                break;
            }
            assert!(
                !running.is_finished(),
                "must observe real preguest fence while guest is still in progress"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let message = json!({"id":correlation(820),"sender":env.actor,
        "event":{"source":env.actor,"kind":"test.note","seq":0,"logical_time":T0,"payload":{},"correlation_id":correlation(821)}});
    let inbox = intervention(&mut env, "hub.inbox", json!({"message":message}));
    let id = inbox["correlation_id"].as_str().unwrap().to_owned();
    let queued = env.call(inbox.clone()).await;
    assert_eq!(queued["result"]["state"], "queued", "{queued}");
    assert!(
        !running.is_finished(),
        "admission must not wait for guest completion"
    );
    let cancel = env.request("hub.cancel", json!({"operation_id":id}));
    let cancelled = env.call(cancel.clone()).await;
    assert_eq!(cancelled["result"]["state"], "cancelled", "{cancelled}");
    assert!(
        !running.is_finished(),
        "cancel must not wait for guest completion"
    );
    let result = running.await.unwrap();
    assert_eq!(
        result["result"]["receipt"]["state"], "failed",
        "bounded real guest abort is a known terminal observation: {result}"
    );
    {
        let locked = shared.lock().unwrap();
        assert!(!locked.has_unresolved_attempt());
        assert_eq!(locked.records().len(), 1);
        assert_eq!(locked.records()[0]["status"], "aborted");
        assert_eq!(locked.records()[0]["events"].as_array().unwrap().len(), 1);
        assert_eq!(locked.queued(), 0, "cancelled inbox created no wake");
    }
    drop(shared);
    env.restart_handler().await;
    assert_eq!(env.call(cancel).await, cancelled);
    let receipt = env.request("hub.receipt", json!({"operation_id":id}));
    assert_eq!(env.call(receipt).await["result"]["state"], "cancelled");
    let read = env.request(
        "debug.read",
        json!({"path":"/data/inbox.jsonl","offset":0,"limit":65536}),
    );
    assert_eq!(
        env.call(read).await["error"]["code"],
        "not_found",
        "cancel did not create an inbox file"
    );
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_guest_completion_drives_deferred_inbox_without_another_request() {
    use coglet_runtime::{SurfaceBinding, SurfaceSession, cbor};
    let wat = r#"(module (memory (export "memory") 1 2)
        (func (export "abi_alloc") (param i32) (result i32) i32.const 0)
        (func (export "handle") (param i32 i32 i64 i64 i64) (result i32)
          (loop $busy br $busy) i32.const 0))"#;
    let mut env = receiver_env_guest(209, wat, 2_000_000_000).await;
    let initial = json!({"id":correlation(830),"sender":env.actor,
        "event":{"source":env.actor,"kind":"test.note","seq":0,"logical_time":T0,"payload":{},"correlation_id":correlation(829)}});
    let initial = intervention(&mut env, "hub.inbox", json!({"message":initial}));
    let initial_id = initial["correlation_id"].as_str().unwrap().to_owned();
    assert_eq!(env.call(initial).await["result"]["state"], "applied");
    assert_eq!(terminal(&mut env, &initial_id).await["state"], "applied");
    assert_eq!(env.simple("hub.start").await["type"], "reply");
    let shared = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    // A real ordinary adapter pump, not debug.step (whose caller also drains).
    // The already applied inbox wake gives the guest one pending input.
    let service = shared.clone();
    let coglet = env.coglet.clone();
    let running = tokio::task::spawn_blocking(move || {
        let adapter = format!("surface:{}", "b".repeat(64));
        let mut session = SurfaceSession::with_service(
            SurfaceBinding::new(adapter.clone(), vec![coglet.clone()]).unwrap(),
            service.clone(),
        )
        .unwrap();
        for kind in ["hello", "capabilities", "grants"] {
            let now = service.lock().unwrap().supervised_now().unwrap();
            let body = match kind {
                "hello" => {
                    json!({"surface":adapter,"offer":{"versions":["0.1"]},"clock":now,"correlation_id":correlation(831)})
                }
                "capabilities" => {
                    json!({"revision":1,"clock":now,"entries":[],"correlation_id":correlation(832)})
                }
                _ => {
                    json!({"epoch":1,"entries":[{"coglet":coglet,"operations":[],"resources":[],"expires":now+10_000_000}],"correlation_id":correlation(833)})
                }
            };
            assert_eq!(
                session.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now),
                0
            );
        }
        let now = service.lock().unwrap().supervised_now().unwrap();
        session.pump_dispatch(now).unwrap();
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if shared.try_lock().is_ok_and(|s| s.has_unresolved_attempt()) {
                break;
            }
            assert!(!running.is_finished());
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let message = json!({"id":correlation(834),"sender":env.actor,
        "event":{"source":env.actor,"kind":"test.note","seq":1,"logical_time":T0,"payload":{},"correlation_id":correlation(835)}});
    let inbox = intervention(&mut env, "hub.inbox", json!({"message":message}));
    let id = inbox["correlation_id"].as_str().unwrap().to_owned();
    assert_eq!(env.call(inbox).await["result"]["state"], "queued");
    assert!(!running.is_finished());
    running.await.unwrap();
    // No receipt/read/network operation is sent while waiting: only the
    // runtime-owned driver can materialize this pending inbox boundary.
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            if shared.lock().unwrap().managed_status()["pending"] == 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("ordinary guest completion must not strand durable queue");
    assert_eq!(shared.lock().unwrap().records().len(), 1);
    drop(shared);
    env.restart_handler().await;
    let receipt = env.request("hub.receipt", json!({"operation_id":id}));
    assert_eq!(env.call(receipt).await["result"]["state"], "applied");
    let read = env.request(
        "debug.read",
        json!({"path":"/data/inbox.jsonl","offset":0,"limit":65536}),
    );
    let result = env.call(read).await;
    let lines: Vec<Value> = result["result"]["text"]
        .as_str()
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["id"], message["id"]);
    assert_eq!(lines[1]["index"], 1);
    assert_eq!(lines[1]["event"], message["event"]);
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn back_channel_single_step_runs_real_wasm_once_and_reuses_original_receipt() {
    let mut env = receiver_env(204).await;
    assert_eq!(
        env.simple("debug.pause").await["result"]["state"],
        "applied"
    );
    let step = env.request("debug.step", json!({}));
    let result = env.call(step.clone()).await;
    assert_eq!(result["result"]["receipt"]["state"], "applied", "{result}");
    assert_eq!(env.call(step.clone()).await, result);
    assert_eq!(env.simple("hub.status").await["result"]["state"], "paused");
    let shared = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    let record = shared.lock().unwrap().records().last().unwrap().clone();
    assert_eq!(record["status"], "committed");
    assert_eq!(record["engine"]["policy"], native_compute_mode());
    assert_eq!(record["events"].as_array().unwrap().len(), 1);
    assert_eq!(record["events"][0]["kind"], "runtime.debug_step");
    assert_eq!(
        record["events"][0]["correlation_id"],
        step["correlation_id"]
    );
    assert_eq!(record["events"][0]["source"], env.coglet);
    assert_eq!(shared.lock().unwrap().records().len(), 1);
    drop(shared);
    env.restart_handler().await;
    assert_eq!(env.call(step).await, result);
    let shared = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    assert_eq!(shared.lock().unwrap().records().len(), 1);
}
