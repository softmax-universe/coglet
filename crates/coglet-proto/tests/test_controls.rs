#![cfg(feature = "test-controls")]
use coglet_proto::authority::{Authority, AuthorityError, Grant};
use iroh::SecretKey;
use std::time::Duration;

#[test]
fn expired_known_invite_is_not_an_unknown_token_fixture() {
    let authority = Authority::open(":memory:").unwrap();
    let peer = SecretKey::generate().public();
    let secret = "11".repeat(32);
    authority.open_window(Duration::from_secs(60)).unwrap();
    // Successful insertion proves this is a known invite, not an arbitrary negative.
    authority
        .test_issue_invite(peer, &secret, Duration::from_millis(1), vec![])
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));
    assert_eq!(
        authority.enroll(peer, Some(&secret)),
        Err(AuthorityError::Unauthenticated)
    );
    let control = "22".repeat(32);
    authority
        .test_issue_invite(peer, &control, Duration::from_secs(60), vec![])
        .unwrap();
    authority.enroll(peer, Some(&control)).unwrap();
}

#[test]
fn closed_window_rejects_known_live_invite_then_reopening_accepts_it() {
    let authority = Authority::open(":memory:").unwrap();
    let peer = SecretKey::generate().public();
    let secret = "11".repeat(32);
    authority.open_window(Duration::from_secs(60)).unwrap();
    authority
        .test_issue_invite(peer, &secret, Duration::from_secs(60), vec![])
        .unwrap();
    authority.close_window().unwrap();
    assert_eq!(
        authority.enroll(peer, Some(&secret)),
        Err(AuthorityError::Unauthenticated)
    );
    authority.open_window(Duration::from_secs(60)).unwrap();
    authority.enroll(peer, Some(&secret)).unwrap();
}

#[test]
fn invalid_fixture_secrets_ttl_grants_and_duplicates_fail_closed() {
    let authority = Authority::open(":memory:").unwrap();
    let peer = SecretKey::generate().public();
    authority.open_window(Duration::from_secs(60)).unwrap();
    for secret in [
        "11".repeat(31),
        "AA".repeat(32),
        "gg".repeat(32),
        "11".repeat(33),
    ] {
        assert_eq!(
            authority.test_issue_invite(peer, &secret, Duration::from_secs(60), vec![]),
            Err(AuthorityError::InvalidArgument)
        );
    }
    let secret = "11".repeat(32);
    assert_eq!(
        authority.test_issue_invite(peer, &secret, Duration::ZERO, vec![]),
        Err(AuthorityError::InvalidArgument)
    );
    assert_eq!(
        authority.test_issue_invite(
            peer,
            &secret,
            Duration::from_secs(60),
            vec![Grant {
                method: "*".into(),
                target: None,
                expires: 100,
                generation: 1
            }]
        ),
        Err(AuthorityError::InvalidArgument)
    );
    authority
        .test_issue_invite(peer, &secret, Duration::from_secs(60), vec![])
        .unwrap();
    assert!(
        authority
            .test_issue_invite(
                SecretKey::generate().public(),
                &secret,
                Duration::from_secs(60),
                vec![]
            )
            .is_err()
    );
    authority.enroll(peer, Some(&secret)).unwrap();
}

#[test]
fn fixture_attempts_use_production_rate_limits() {
    let authority = Authority::open(":memory:").unwrap();
    let peer = SecretKey::generate().public();
    authority.open_window(Duration::from_secs(60)).unwrap();
    let secret = "11".repeat(32);
    authority
        .test_issue_invite(peer, &secret, Duration::from_secs(60), vec![])
        .unwrap();
    for _ in 0..8 {
        assert_eq!(
            authority.enroll(peer, Some(&"22".repeat(32))),
            Err(AuthorityError::Unauthenticated)
        );
    }
    assert_eq!(
        authority.enroll(peer, Some(&secret)),
        Err(AuthorityError::ResourceExhausted)
    );
}

#[test]
fn fixture_persists_only_hash_and_restart_invalidates_live_secret() {
    use sha2::{Digest, Sha256};
    let path = std::env::temp_dir().join(format!(
        "coglet-test-controls-{}.sqlite",
        SecretKey::generate().public()
    ));
    let peer = SecretKey::generate().public();
    let secret = "11".repeat(32);
    let authority = Authority::open(&path).unwrap();
    authority.open_window(Duration::from_secs(60)).unwrap();
    authority
        .test_issue_invite(peer, &secret, Duration::from_secs(60), vec![])
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    let hash: String = db
        .query_row("SELECT hash FROM proto_invites", [], |r| r.get(0))
        .unwrap();
    assert_eq!(hash, hex::encode(Sha256::digest(secret.as_bytes())));
    drop(db);
    drop(authority);
    let restarted = Authority::open(&path).unwrap();
    restarted.open_window(Duration::from_secs(60)).unwrap();
    assert_eq!(
        restarted.enroll(peer, Some(&secret)),
        Err(AuthorityError::Unauthenticated)
    );
    assert_eq!(
        restarted.enroll(peer, None),
        Err(AuthorityError::Unauthenticated)
    );
    // Re-provisioning cannot revive the persisted hash after restart.
    assert!(
        restarted
            .test_issue_invite(peer, &secret, Duration::from_secs(60), vec![])
            .is_err()
    );
    drop(restarted);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn fixed_invite_preserves_one_use_peer_binding_and_grants() {
    let authority = Authority::open(":memory:").unwrap();
    let peer = SecretKey::generate().public();
    let other = SecretKey::generate().public();
    let secret = "11".repeat(32);
    authority.open_window(Duration::from_secs(60)).unwrap();
    authority
        .test_issue_invite(
            peer,
            &secret,
            Duration::from_secs(60),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: 100,
                generation: 1,
            }],
        )
        .unwrap();
    assert_eq!(
        authority.enroll(other, Some(&secret)),
        Err(AuthorityError::Unauthenticated)
    );
    authority.enroll(peer, Some(&secret)).unwrap();
    assert_eq!(
        authority.enroll(peer, Some(&secret)),
        Err(AuthorityError::Unauthenticated)
    );
    authority.enroll(peer, None).unwrap();
    authority
        .authorize(peer, "hub.health", None, Some(1), 1)
        .unwrap();
}
