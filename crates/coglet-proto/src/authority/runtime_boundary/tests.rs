use super::*;
use crate::{authority::accepted_refs::AcceptedRefStore, signing};
use serde_json::json;
use std::{cell::Cell, time::Duration};

const START_ID: &str = "11111111111111111111111111111111";
const STOP_ID: &str = "22222222222222222222222222222222";

struct Fixture {
    authority: Authority,
    hub: EndpointId,
    device: EndpointId,
    coglet: String,
    start_scope: Scope,
    stop_scope: Scope,
    start: Grant,
    stop: Grant,
    signed: Value,
}

impl Fixture {
    // Separate peer-grant, exact-scope and delegation expiries ensure that no
    // unrelated window can mask a missing check in either half of admission.
    fn new(path: impl AsRef<Path>, expiry: [u64; 5]) -> Self {
        let authority = Authority::open(path).unwrap();
        AcceptedRefStore::new(authority.clone()).unwrap();
        let owner = SecretKey::from_bytes(&[1; 32]);
        let hub = SecretKey::from_bytes(&[2; 32]).public();
        let device = SecretKey::from_bytes(&[3; 32]).public();
        let coglet = format!("coglet:{}", owner.public());
        let start_scope = Scope {
            actor: format!("coglet:{}", SecretKey::from_bytes(&[4; 32]).public()),
            ticket: Some("aa".repeat(16)),
        };
        // The emergency obligation need not borrow the original start's actor
        // or ticket: both exact contexts must independently remain authorized.
        let stop_scope = Scope {
            actor: format!("coglet:{}", SecretKey::from_bytes(&[5; 32]).public()),
            ticket: Some("bb".repeat(16)),
        };
        let start = Grant {
            method: "hub.start".into(),
            target: Some(coglet.clone()),
            expires: expiry[0],
            generation: 7,
        };
        let stop = Grant {
            method: "hub.stop".into(),
            target: Some(coglet.clone()),
            expires: expiry[2],
            generation: 11,
        };
        authority.open_window(Duration::from_secs(60)).unwrap();
        let invite = authority
            .issue_invite(
                hub,
                Duration::from_secs(60),
                vec![start.clone(), stop.clone()],
            )
            .unwrap();
        authority.enroll(hub, Some(&invite)).unwrap();
        let mut start_scoped = start.clone();
        start_scoped.expires = expiry[1];
        let mut stop_scoped = stop.clone();
        stop_scoped.expires = expiry[3];
        authority
            .provision_scope(hub, &start_scope, &start_scoped)
            .unwrap();
        authority
            .provision_scope(hub, &stop_scope, &stop_scoped)
            .unwrap();
        let statement = json!({
            "purpose":"delegation", "coglet":coglet,
            "hub":hub.to_string(), "device":device.to_string(),
            "generation":13, "expires":expiry[4],
            "methods":["hub.start", "hub.stop"]
        });
        let signature = owner.sign(&signing::preimage("delegation", &statement).unwrap());
        let signed = json!({"statement":statement, "signer":owner.public().to_string(),
            "signature":hex::encode(signature.to_bytes())});
        authority
            .accept_delegation(hub, device, &coglet, &signed, || Some(1))
            .unwrap();
        Self {
            authority,
            hub,
            device,
            coglet,
            start_scope,
            stop_scope,
            start,
            stop,
            signed,
        }
    }

    fn boundary<T>(
        &self,
        clock: impl Fn() -> Option<u64>,
        apply: impl FnOnce(
            &Value,
            &dyn Fn() -> std::result::Result<u64, String>,
        ) -> std::result::Result<T, String>,
    ) -> std::result::Result<T, String> {
        self.authority.runtime_boundary_obligation(
            self.hub,
            self.device,
            &self.coglet,
            &self.start_scope,
            &self.start,
            13,
            None,
            Some(START_ID),
            Some((&self.stop_scope, &self.stop, STOP_ID)),
            clock,
            apply,
        )
    }
}

#[test]
fn joint_obligation_accepts_distinct_original_contexts_and_rechecks_before_effect() {
    let f = Fixture::new(":memory:", [100; 5]);
    let samples = Cell::new(0);
    let result = f.boundary(
        || {
            samples.set(samples.get() + 1);
            Some(99)
        },
        |signed, recheck| {
            assert_eq!(signed, &f.signed);
            assert_eq!(recheck()?, 99);
            Ok("lease-enabled")
        },
    );
    assert_eq!(result, Ok("lease-enabled"));
    assert_eq!(
        samples.get(),
        3,
        "admission, pre-apply, immediate-before-effect"
    );
}

#[test]
fn joint_obligation_each_expiry_is_exclusive_at_admission_preapply_and_effect() {
    for window in 0..5 {
        let mut expiry = [1000; 5];
        expiry[window] = 100;
        let f = Fixture::new(":memory:", expiry);
        // Expire independently at each boundary, with no wall-clock sleeps.
        for expire_at in 0..3 {
            let samples = Cell::new(0);
            let entered = Cell::new(false);
            let effect = Cell::new(false);
            let result = f.boundary(
                || {
                    let sample = samples.get();
                    samples.set(sample + 1);
                    Some(if sample >= expire_at { 100 } else { 99 })
                },
                |_, recheck| {
                    entered.set(true);
                    recheck()?;
                    effect.set(true);
                    Ok(())
                },
            );
            assert_eq!(
                result,
                Err("permission_denied".into()),
                "window={window}, boundary={expire_at}"
            );
            assert_eq!(
                entered.get(),
                expire_at == 2,
                "window={window}, boundary={expire_at}"
            );
            assert!(!effect.get(), "window={window}, boundary={expire_at}");
        }
    }
}

#[test]
fn joint_obligation_missing_or_regressed_final_clock_never_enables_effect() {
    let f = Fixture::new(":memory:", [1000; 5]);
    for final_time in [None, Some(98)] {
        let samples = Cell::new(0);
        let entered = Cell::new(false);
        let effect = Cell::new(false);
        let result = f.boundary(
            || {
                let sample = samples.get();
                samples.set(sample + 1);
                if sample < 2 { Some(99) } else { final_time }
            },
            |_, recheck| {
                entered.set(true);
                recheck()?;
                effect.set(true);
                Ok(())
            },
        );
        assert_eq!(result, Err("permission_denied".into()));
        assert!(entered.get());
        assert!(!effect.get());
    }
}

#[test]
fn joint_obligation_revoked_start_or_stop_authority_denies_before_disclosure() {
    for revoked in [
        "start-ticket",
        "stop-ticket",
        "start-grant",
        "stop-grant",
        "delegation",
        "peer",
    ] {
        let f = Fixture::new(":memory:", [1000; 5]);
        assert_eq!(
            f.boundary(|| Some(99), |_, gate| gate().map(|_| ())),
            Ok(())
        );
        match revoked {
            "start-ticket" => f.authority.end_ticket(&f.start_scope).unwrap(),
            "stop-ticket" => f.authority.end_ticket(&f.stop_scope).unwrap(),
            "start-grant" | "stop-grant" => {
                let method = if revoked == "start-grant" {
                    "hub.start"
                } else {
                    "hub.stop"
                };
                f.authority
                    .0
                    .lock()
                    .unwrap()
                    .db
                    .execute(
                        "DELETE FROM proto_grants WHERE peer=?1 AND method=?2 AND target=?3",
                        params![f.hub.to_string(), method, f.coglet],
                    )
                    .unwrap();
            }
            "delegation" => f.authority.revoke_delegation(f.hub, &f.coglet).unwrap(),
            "peer" => f.authority.revoke_peer(f.hub).unwrap(),
            _ => unreachable!(),
        }
        let result = f.boundary(
            || Some(99),
            |_, _| -> std::result::Result<(), String> {
                panic!("revoked {revoked} must not disclose signed delegation or enter apply")
            },
        );
        assert_eq!(result, Err("permission_denied".into()), "{revoked}");
    }
}

#[test]
fn joint_obligation_both_method_ids_conflict_with_git_without_scope_disclosure() {
    // ID collisions are keyed by peer/id/target, NOT actor/ticket or method.
    // A Git receipt owned by another context must still block either lifecycle ID.
    for id in [START_ID, STOP_ID] {
        for same_peer in [false, true] {
            for same_target in [false, true] {
                let f = Fixture::new(":memory:", [1000; 5]);
                let peer = if same_peer {
                    f.hub
                } else {
                    SecretKey::from_bytes(&[6; 32]).public()
                };
                let target = if same_target {
                    f.coglet.clone()
                } else {
                    format!("coglet:{}", SecretKey::from_bytes(&[7; 32]).public())
                };
                {
                    let state = f.authority.0.lock().unwrap();
                    state.db.execute(
                        "INSERT INTO proto_ref_audit(id,hub,coglet,ref,mode,signed,peer,actor,ticket,grant_generation,delegation_generation,accepted_at,bundle)
                         VALUES(1,?1,?2,'refs/heads/hub','authoritative','{}',?1,'foreign-actor','foreign-ticket','1','1','1',X'00')",
                        params![peer.to_string(), target],
                    ).unwrap();
                    state.db.execute(
                        "INSERT INTO proto_git_operations(peer,operation_id,target,digest,receipt,audit_id) VALUES(?1,?2,?3,'digest',?4,1)",
                        params![peer.to_string(), id, target, json!({"operation_id":id,"state":"applied"}).to_string()],
                    ).unwrap();
                }
                let entered = Cell::new(false);
                let result = f.boundary(
                    || Some(99),
                    |_, gate| {
                        entered.set(true);
                        gate().map(|_| ())
                    },
                );
                if same_peer && same_target {
                    assert_eq!(result, Err("conflict".into()), "id={id}");
                    assert!(
                        !entered.get(),
                        "collision must not disclose foreign receipt"
                    );
                } else {
                    assert_eq!(
                        result,
                        Ok(()),
                        "unrelated peer/target must not collide: {id}"
                    );
                    assert!(entered.get());
                }
            }
        }
    }
}

#[test]
fn joint_obligation_one_sql_write_guard_covers_both_methods_through_final_gate() {
    let path = std::env::temp_dir().join(format!(
        "coglet-joint-boundary-{}.sqlite",
        SecretKey::generate().public()
    ));
    {
        let f = Fixture::new(&path, [1000; 5]);
        let other = Connection::open(&path).unwrap();
        other.busy_timeout(Duration::ZERO).unwrap();
        let blocked = Cell::new(0);
        let attempt_revocations = || {
            for method in ["hub.start", "hub.stop"] {
                let err = other
                    .execute(
                        "DELETE FROM proto_grants WHERE peer=?1 AND method=?2 AND target=?3",
                        params![f.hub.to_string(), method, f.coglet],
                    )
                    .unwrap_err();
                assert!(matches!(err, rusqlite::Error::SqliteFailure(ref e, _)
                    if e.code == rusqlite::ErrorCode::DatabaseBusy));
                blocked.set(blocked.get() + 1);
            }
        };
        let result = f.boundary(
            || {
                // First clock is before either SQL authority read; second is
                // after both reads/signature work, third is the effect gate.
                attempt_revocations();
                Some(99)
            },
            |_, gate| {
                attempt_revocations();
                gate().map(|_| ())
            },
        );
        assert_eq!(result, Ok(()));
        assert_eq!(blocked.get(), 8);
        // The same write succeeds immediately after apply returns; the next
        // joint admission must observe that committed revocation.
        assert_eq!(
            other
                .execute(
                    "DELETE FROM proto_grants WHERE peer=?1 AND method='hub.stop' AND target=?2",
                    params![f.hub.to_string(), f.coglet],
                )
                .unwrap(),
            1
        );
        assert_eq!(
            f.boundary(|| Some(99), |_, _| Ok(())),
            Err("permission_denied".into())
        );
    }
    std::fs::remove_file(path).unwrap();
}
