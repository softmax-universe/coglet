//! Handler admission controls supplement the real-process guest proof.
use super::*;

fn faulted(env: &Env) {
    let service = env
        .handler
        .as_ref()
        .unwrap()
        .share_service(&env.coglet)
        .unwrap();
    let locked = service.lock().unwrap();
    assert_eq!(locked.managed_status()["state"], "faulted");
    assert!(!locked.managed_status()["error"].is_null());
    assert!(locked.records().is_empty());
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_revalidation_refuses_expired_or_changed_authority() {
    timed!(async {
        for mode in 0..9 {
            let mut env = Env::new(0x70 + mode, Options::default()).await.unwrap();
            pushed_and_installed(&mut env).await;
            applied(&env.simple("hub.start").await);
            match mode {
                0 => {
                    env.clock.store(DELEGATION_EXPIRES, Ordering::SeqCst);
                }
                1 => {
                    env.config
                        .targets
                        .get_mut(&env.coglet)
                        .unwrap()
                        .grant_generation += 1;
                }
                2 => {
                    env.config
                        .targets
                        .get_mut(&env.coglet)
                        .unwrap()
                        .grant_generation -= 1;
                }
                3 => {
                    env.config
                        .targets
                        .get_mut(&env.coglet)
                        .unwrap()
                        .delegation_generation += 1;
                }
                4 => {
                    env.config
                        .targets
                        .get_mut(&env.coglet)
                        .unwrap()
                        .delegation_generation -= 1;
                }
                5 => {
                    Authority::open(env.root.join("authority.sqlite"))
                        .unwrap()
                        .revoke_delegation(env.hub.id(), &env.coglet)
                        .unwrap();
                }
                6 => {
                    let out = Command::new("sqlite3")
                        .arg(env.root.join("authority.sqlite"))
                        .arg("DELETE FROM proto_scopes WHERE method='hub.start';")
                        .output()
                        .unwrap();
                    assert!(out.status.success());
                }
                7 | 8 => {
                    // Boot authority sample 1 validates SQL, sample 2 follows
                    // signature work, 3 precedes native enable, 4 follows it.
                    // Expire only at the final gates, not before validation.
                    let clock = env.clock.clone();
                    let at = env.reads.load(Ordering::SeqCst) + u64::from(mode - 4);
                    *env.trap.lock().unwrap() = Some((
                        at,
                        Box::new(move || {
                            clock.store(DELEGATION_EXPIRES, Ordering::SeqCst);
                        }),
                    ));
                }
                _ => unreachable!(),
            }
            env.restart_handler().await;
            assert!(
                env.trap.lock().unwrap().is_none(),
                "final admission gate was exercised"
            );
            faulted(&env);
        }
    });
}

#[cfg(feature = "native-engine")]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn restart_kill_switch_refuses_resume() {
    timed!(async {
        for missing in [false, true] {
            let mut env = Env::new(if missing { 0x79 } else { 0x78 }, Options::default())
                .await
                .unwrap();
            pushed_and_installed(&mut env).await;
            let start = env.simple("hub.start").await;
            applied(&start);
            if missing {
                std::fs::remove_file(env.root.join("kill.json")).unwrap();
            } else {
                std::fs::write(env.root.join("kill.json"), b"{\"latched\":true}").unwrap();
            }
            env.restart_handler().await;
            faulted(&env);
            // Historical receipt retry must not clear a recovery fault.
            std::fs::write(env.root.join("kill.json"), b"{\"latched\":false}").unwrap();
            let retry = env.request_with(
                "hub.start",
                start["correlation_id"].as_str().unwrap().into(),
                json!({}),
                &env.actor,
                Some(env.ticket.clone()),
            );
            assert_eq!(env.call(retry).await["result"], start["result"]);
            faulted(&env);
            applied(&env.simple("hub.start").await);
            assert_eq!(state(&env.simple("hub.status").await).0, "running");
        }
    });
}
