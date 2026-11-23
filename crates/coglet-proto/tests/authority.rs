use coglet_proto::authority::{Authority, Grant};
use iroh::SecretKey;
use std::time::Duration;

fn database_path() -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "coglet-authority-{}.sqlite",
        SecretKey::generate().public()
    ))
}

#[test]
fn closed_expired_and_restart_windows_never_redeem_secrets() {
    let path = database_path();
    let peer = SecretKey::generate().public();
    let authority = Authority::open(&path).unwrap();
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(peer, Duration::from_secs(30), vec![])
        .unwrap();
    authority.close_window().unwrap();
    assert!(authority.enroll(peer, Some(&secret)).is_err());
    authority.open_window(Duration::from_secs(30)).unwrap();
    let short = authority
        .issue_invite(peer, Duration::from_millis(1), vec![])
        .unwrap();
    std::thread::sleep(Duration::from_millis(5));
    assert!(authority.enroll(peer, Some(&short)).is_err());
    drop(authority);
    let restarted = Authority::open(&path).unwrap();
    restarted.open_window(Duration::from_secs(30)).unwrap();
    assert!(restarted.enroll(peer, Some(&secret)).is_err());
    assert!(restarted.enroll(peer, None).is_err());
    drop(restarted);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn failed_grant_write_rolls_back_invite_consumption_and_allowlist() {
    let path = database_path();
    let peer = SecretKey::generate().public();
    let authority = Authority::open(&path).unwrap();
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(
            peer,
            Duration::from_secs(30),
            vec![Grant {
                method: "hub.health".into(),
                target: None,
                expires: 1000,
                generation: 1,
            }],
        )
        .unwrap();
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_grant BEFORE INSERT ON proto_grants BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;").unwrap();
    assert!(authority.enroll(peer, Some(&secret)).is_err());
    assert!(authority.enroll(peer, None).is_err());
    db.execute_batch("DROP TRIGGER fail_grant;").unwrap();
    authority.enroll(peer, Some(&secret)).unwrap();
    authority
        .authorize(peer, "hub.health", None, Some(0), 1)
        .unwrap();
    let stored: String = db
        .query_row("SELECT hash FROM proto_invites", [], |r| r.get(0))
        .unwrap();
    assert_ne!(stored, secret);
    assert!(
        !std::fs::read(&path)
            .unwrap()
            .windows(secret.len())
            .any(|b| b == secret.as_bytes())
    );
    drop(db);
    drop(authority);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn concurrent_redemptions_have_one_winner_and_attempts_are_limited() {
    use coglet_proto::authority::AuthorityError;
    let path = database_path();
    let peer = SecretKey::generate().public();
    let authority = Authority::open(&path).unwrap();
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(peer, Duration::from_secs(30), vec![])
        .unwrap();
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let authority = authority.clone();
            let secret = secret.clone();
            std::thread::spawn(move || authority.enroll(peer, Some(&secret)))
        })
        .collect();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
    for _ in 0..4 {
        authority.enroll(peer, None).unwrap();
    }
    assert_eq!(
        authority.enroll(peer, None),
        Err(AuthorityError::ResourceExhausted)
    );
    // One peer's quota does not consume the global quota.
    assert_eq!(
        authority.enroll(SecretKey::generate().public(), None),
        Err(AuthorityError::Unauthenticated)
    );
    drop(authority);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn one_use_enrollment_commits_exact_grants_and_survives_restart() {
    let path = std::env::temp_dir().join(format!(
        "coglet-authority-{}.sqlite",
        SecretKey::generate().public()
    ));
    let peer = SecretKey::generate().public();
    let stranger = SecretKey::generate().public();
    let grant = Grant {
        method: "hub.health".into(),
        target: None,
        expires: 1000,
        generation: 1,
    };
    let authority = Authority::open(&path).unwrap();
    assert!(authority.enroll(peer, None).is_err());
    assert!(
        authority
            .issue_invite(peer, Duration::from_secs(30), vec![grant.clone()])
            .is_err()
    );
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(peer, Duration::from_secs(30), vec![grant])
        .unwrap();
    assert!(authority.enroll(stranger, Some(&secret)).is_err());
    authority.enroll(peer, Some(&secret)).unwrap();
    assert!(authority.enroll(peer, Some(&secret)).is_err());
    authority.enroll(peer, None).unwrap();
    assert!(
        authority
            .authorize(peer, "hub.health", None, Some(999), 1)
            .is_ok()
    );
    assert!(
        authority
            .authorize(peer, "hub.health", Some(""), Some(999), 1)
            .is_err()
    );
    assert!(
        authority
            .authorize(peer, "hub.health", None, Some(1000), 1)
            .is_err()
    );
    assert!(
        authority
            .authorize(peer, "hub.health", None, None, 1)
            .is_err()
    );
    assert!(
        authority
            .authorize(peer, "hub.health", None, Some(999), 2)
            .is_err()
    );
    assert!(
        authority
            .authorize(peer, "hub.status", None, Some(999), 1)
            .is_err()
    );
    assert!(
        authority
            .authorize(peer, "hub.health", Some("foreign"), Some(999), 1)
            .is_err()
    );
    drop(authority);
    let restarted = Authority::open(&path).unwrap();
    restarted.enroll(peer, None).unwrap();
    restarted
        .authorize(peer, "hub.health", None, Some(999), 1)
        .unwrap();
    restarted.revoke_peer(peer).unwrap();
    assert!(restarted.enroll(peer, None).is_err());
    assert!(
        restarted
            .authorize(peer, "hub.health", None, Some(999), 1)
            .is_err()
    );
    drop(restarted);
    std::fs::remove_file(path).unwrap();
}
