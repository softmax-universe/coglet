use coglet_proto::authority::{Authority, AuthorityError};
use iroh::SecretKey;
use std::time::Duration;

#[test]
fn invite_that_expires_waiting_for_transaction_is_not_consumed() {
    let path = std::env::temp_dir().join(format!(
        "coglet-expiry-{}.sqlite",
        SecretKey::generate().public()
    ));
    let authority = Authority::open(&path).unwrap();
    let peer = SecretKey::generate().public();
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(peer, Duration::from_millis(100), vec![])
        .unwrap();
    let blocker = rusqlite::Connection::open(&path).unwrap();
    blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let a = authority.clone();
    let (started, waiting) = std::sync::mpsc::channel();
    let redemption = std::thread::spawn(move || {
        started.send(()).unwrap();
        a.enroll(peer, Some(&secret))
    });
    waiting.recv().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    blocker.execute_batch("COMMIT").unwrap();
    assert_eq!(
        redemption.join().unwrap(),
        Err(AuthorityError::Unauthenticated)
    );
    assert!(authority.enroll(peer, None).is_err());
    let used: bool = blocker
        .query_row("SELECT used FROM proto_invites", [], |r| r.get(0))
        .unwrap();
    assert!(!used);
    drop(authority);
    drop(blocker);
    std::fs::remove_file(path).unwrap();
}
