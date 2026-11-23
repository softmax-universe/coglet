use coglet_proto::authority::{Authority, Grant, Scope};
use iroh::SecretKey;
use std::time::Duration;

#[test]
fn ticket_omission_cannot_borrow_authority_and_ended_ticket_stays_revoked() {
    let path = std::env::temp_dir().join(format!(
        "coglet-scopes-{}.sqlite",
        SecretKey::generate().public()
    ));
    let authority = Authority::open(&path).unwrap();
    let peer = SecretKey::generate().public();
    let target = format!("coglet:{}", SecretKey::generate().public());
    let actor = format!("coglet:{}", SecretKey::generate().public());
    let grant = Grant {
        method: "hub.status".into(),
        target: Some(target.clone()),
        expires: 1000,
        generation: u64::MAX,
    };
    authority.open_window(Duration::from_secs(30)).unwrap();
    let secret = authority
        .issue_invite(peer, Duration::from_secs(30), vec![grant.clone()])
        .unwrap();
    authority.enroll(peer, Some(&secret)).unwrap();
    let scoped = Scope {
        actor: actor.clone(),
        ticket: Some("ab".repeat(16)),
    };
    let no_ticket = Scope {
        actor: actor.clone(),
        ticket: None,
    };
    assert!(
        authority
            .authorize_scope(peer, &scoped, &grant, Some(1))
            .is_err()
    );
    authority.provision_scope(peer, &scoped, &grant).unwrap();
    authority
        .authorize_scope(peer, &scoped, &grant, Some(1))
        .unwrap();
    assert!(
        authority
            .authorize_scope(peer, &no_ticket, &grant, Some(1))
            .is_err()
    );
    let foreign_actor = Scope {
        actor: format!("coglet:{}", SecretKey::generate().public()),
        ticket: scoped.ticket.clone(),
    };
    assert!(
        authority
            .authorize_scope(peer, &foreign_actor, &grant, Some(1))
            .is_err()
    );
    assert!(
        authority
            .authorize_scope(peer, &scoped, &grant, Some(1000))
            .is_err()
    );
    authority.end_ticket(&scoped).unwrap();
    assert!(
        authority
            .authorize_scope(peer, &scoped, &grant, Some(1))
            .is_err()
    );
    assert!(authority.provision_scope(peer, &scoped, &grant).is_err());
    drop(authority);
    let authority = Authority::open(&path).unwrap();
    assert!(
        authority
            .authorize_scope(peer, &scoped, &grant, Some(1))
            .is_err()
    );
    authority.provision_scope(peer, &no_ticket, &grant).unwrap();
    authority
        .authorize_scope(peer, &no_ticket, &grant, Some(1))
        .unwrap();
    authority.revoke_peer(peer).unwrap();
    assert!(
        authority
            .authorize_scope(peer, &no_ticket, &grant, Some(1))
            .is_err()
    );
    drop(authority);
    std::fs::remove_file(path).unwrap();
}
