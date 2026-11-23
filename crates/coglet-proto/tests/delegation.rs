use coglet_proto::{
    authority::{Authority, AuthorityError},
    signing,
};
use iroh::{EndpointId, SecretKey};
use serde_json::{Value, json};

fn proof(
    key: &SecretKey,
    hub: EndpointId,
    device: EndpointId,
    generation: u64,
    expires: u64,
) -> Value {
    let statement = json!({"purpose":"delegation", "coglet":format!("coglet:{}", key.public()), "hub":hub.to_string(), "device":device.to_string(), "generation":generation, "expires":expires, "methods":["hub.events", "hub.status"]});
    let signature = key.sign(&signing::preimage("delegation", &statement).unwrap());
    json!({"statement":statement, "signer":key.public().to_string(), "signature":hex::encode(signature.to_bytes())})
}

#[test]
fn concurrent_replacement_cannot_rewind_across_separate_database_handles() {
    let path = std::env::temp_dir().join(format!(
        "coglet-generation-race-{}.sqlite",
        SecretKey::generate().public()
    ));
    let key = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let initial = proof(&key, hub, device, 1, 100);
    let coglet = initial["statement"]["coglet"].as_str().unwrap().to_owned();
    let authority = Authority::open(&path).unwrap();
    authority
        .accept_delegation(hub, device, &coglet, &initial, || Some(1))
        .unwrap();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles: Vec<_> = [2, 3]
        .into_iter()
        .map(|generation| {
            let other = Authority::open(&path).unwrap();
            let proof = proof(&key, hub, device, generation, 100);
            let coglet = coglet.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                other.replace_delegation(hub, device, &coglet, &proof, || Some(1))
            })
        })
        .collect();
    barrier.wait();
    for handle in handles {
        let _ = handle.join().unwrap();
    }
    authority
        .authorize_delegation(hub, device, &coglet, "hub.events", || Some(1), 3)
        .unwrap();
    assert!(
        authority
            .authorize_delegation(hub, device, &coglet, "hub.events", || Some(1), 2)
            .is_err()
    );
    let stale = proof(&key, hub, device, 2, 100);
    assert!(
        authority
            .replace_delegation(hub, device, &coglet, &stale, || Some(1))
            .is_err()
    );
    drop(authority);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn foreign_coglet_and_wrong_signer_are_not_delegation_authority() {
    let authority = Authority::open(":memory:").unwrap();
    let key = SecretKey::generate();
    let foreign = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let signed = proof(&key, hub, device, 1, 100);
    let coglet = signed["statement"]["coglet"].as_str().unwrap();
    assert!(
        authority
            .accept_delegation(
                hub,
                device,
                &format!("coglet:{}", foreign.public()),
                &signed,
                || Some(1)
            )
            .is_err()
    );
    let mut forged = signed.clone();
    forged["signer"] = json!(foreign.public().to_string());
    forged["signature"] = json!(hex::encode(
        foreign
            .sign(&signing::preimage("delegation", &signed["statement"]).unwrap())
            .to_bytes()
    ));
    assert!(
        authority
            .accept_delegation(hub, device, coglet, &forged, || Some(1))
            .is_err()
    );
    assert!(
        authority
            .accept_delegation(hub, SecretKey::generate().public(), coglet, &signed, || {
                Some(1)
            })
            .is_err()
    );
}

#[test]
fn signed_delegation_binds_identity_method_and_trusted_expiry() {
    let authority = Authority::open(":memory:").unwrap();
    let key = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let signed = proof(&key, hub, device, 1, 100);
    let coglet = signed["statement"]["coglet"].as_str().unwrap();
    authority
        .accept_delegation(hub, device, coglet, &signed, || Some(99))
        .unwrap();
    authority
        .authorize_delegation(hub, device, coglet, "hub.events", || Some(99), 1)
        .unwrap();
    assert_eq!(
        authority.authorize_delegation(hub, device, coglet, "hub.actions", || Some(99), 1),
        Err(AuthorityError::PermissionDenied)
    );
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || None, 1)
            .is_err()
    );
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || Some(100), 1)
            .is_err()
    );
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || Some(99), 2)
            .is_err()
    );
    assert!(
        authority
            .authorize_delegation(
                hub,
                SecretKey::generate().public(),
                coglet,
                "hub.events",
                || Some(99),
                1
            )
            .is_err()
    );
    assert!(
        authority
            .accept_delegation(
                SecretKey::generate().public(),
                device,
                coglet,
                &signed,
                || Some(99)
            )
            .is_err()
    );
    let mut tampered = signed.clone();
    tampered["statement"]["expires"] = json!(200);
    assert!(
        authority
            .accept_delegation(hub, device, coglet, &tampered, || Some(99))
            .is_err()
    );
    // Exact retry does not replace or expand authority.
    authority
        .accept_delegation(hub, device, coglet, &signed, || Some(99))
        .unwrap();
}

#[test]
fn expiry_or_clock_loss_during_admission_rolls_back_binding() {
    use std::cell::Cell;
    let authority = Authority::open(":memory:").unwrap();
    let key = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let signed = proof(&key, hub, device, 1, 100);
    let coglet = signed["statement"]["coglet"].as_str().unwrap();
    let calls = Cell::new(0);
    let clock = || {
        let n = calls.get();
        calls.set(n + 1);
        Some(if n == 0 { 99 } else { 100 })
    };
    assert_eq!(
        authority.accept_delegation(hub, device, coglet, &signed, clock),
        Err(AuthorityError::PermissionDenied)
    );
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || Some(99), 1)
            .is_err()
    );
    authority
        .accept_delegation(hub, device, coglet, &signed, || Some(99))
        .unwrap();
    let next = proof(&key, hub, device, 2, 100);
    calls.set(0);
    let clock = || {
        let n = calls.get();
        calls.set(n + 1);
        if n == 0 { Some(99) } else { None }
    };
    assert_eq!(
        authority.replace_delegation(hub, device, coglet, &next, clock),
        Err(AuthorityError::PermissionDenied)
    );
    authority
        .authorize_delegation(hub, device, coglet, "hub.events", || Some(99), 1)
        .unwrap();
}

#[test]
fn peer_revocation_fences_delegation_even_after_reenrollment() {
    use std::time::Duration;
    let authority = Authority::open(":memory:").unwrap();
    let key = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let signed = proof(&key, hub, device, 1, 100);
    let coglet = signed["statement"]["coglet"].as_str().unwrap();
    authority.open_window(Duration::from_secs(60)).unwrap();
    let invite = authority
        .issue_invite(device, Duration::from_secs(60), vec![])
        .unwrap();
    authority.enroll(device, Some(&invite)).unwrap();
    authority
        .accept_delegation(hub, device, coglet, &signed, || Some(1))
        .unwrap();
    authority.revoke_peer(device).unwrap();
    let invite = authority
        .issue_invite(device, Duration::from_secs(60), vec![])
        .unwrap();
    authority.enroll(device, Some(&invite)).unwrap();
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || Some(1), 1)
            .is_err()
    );
    assert!(
        authority
            .accept_delegation(hub, device, coglet, &signed, || Some(1))
            .is_err()
    );
}

#[test]
fn absent_delegation_revocation_does_not_claim_a_fence() {
    let authority = Authority::open(":memory:").unwrap();
    let hub = SecretKey::generate().public();
    let coglet = format!("coglet:{}", SecretKey::generate().public());
    assert_eq!(
        authority.revoke_delegation(hub, &coglet),
        Err(AuthorityError::PermissionDenied)
    );
}

#[test]
fn replacement_is_operator_only_and_tombstone_survives_restart() {
    let path = std::env::temp_dir().join(format!(
        "coglet-delegation-{}.sqlite",
        SecretKey::generate().public()
    ));
    let key = SecretKey::generate();
    let hub = SecretKey::generate().public();
    let device = SecretKey::generate().public();
    let replacement = SecretKey::generate().public();
    let initial = proof(&key, hub, device, 7, 100);
    let next = proof(&key, hub, replacement, 8, 200);
    let coglet = initial["statement"]["coglet"].as_str().unwrap();
    let authority = Authority::open(&path).unwrap();
    authority
        .accept_delegation(hub, device, coglet, &initial, || Some(0))
        .unwrap();
    assert!(
        authority
            .accept_delegation(hub, replacement, coglet, &next, || Some(0))
            .is_err()
    );
    authority
        .replace_delegation(hub, replacement, coglet, &next, || Some(0))
        .unwrap();
    assert!(
        authority
            .authorize_delegation(hub, device, coglet, "hub.events", || Some(0), 7)
            .is_err()
    );
    authority
        .authorize_delegation(hub, replacement, coglet, "hub.events", || Some(0), 8)
        .unwrap();
    // Repeating an operator replacement is not a new generation.
    assert!(
        authority
            .replace_delegation(hub, replacement, coglet, &next, || Some(0))
            .is_err()
    );
    authority.revoke_delegation(hub, coglet).unwrap();
    drop(authority);
    let authority = Authority::open(&path).unwrap();
    assert!(
        authority
            .authorize_delegation(hub, replacement, coglet, "hub.events", || Some(0), 8)
            .is_err()
    );
    assert!(
        authority
            .accept_delegation(hub, replacement, coglet, &next, || Some(0))
            .is_err()
    );
    assert!(
        authority
            .replace_delegation(hub, device, coglet, &initial, || Some(0))
            .is_err()
    );
    let max = proof(&key, hub, device, u64::MAX, 200);
    authority
        .replace_delegation(hub, device, coglet, &max, || Some(0))
        .unwrap();
    authority.revoke_delegation(hub, coglet).unwrap();
    assert!(
        authority
            .replace_delegation(hub, device, coglet, &max, || Some(0))
            .is_err()
    );
    assert!(
        authority
            .replace_delegation(
                hub,
                device,
                coglet,
                &proof(&key, hub, device, 0, 200),
                || Some(0)
            )
            .is_err()
    );
    drop(authority);
    std::fs::remove_file(path).unwrap();
}
