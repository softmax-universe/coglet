#![cfg(target_os = "linux")]
use coglet_proto::{
    authority::{
        Authority, Grant, Scope,
        accepted_refs::{Acceptance, AcceptedRefStore, Mode, StoreError},
    },
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    signing,
    transfer::DurableSink,
    validator::{Limits, Validator, VerifiedBundle},
};
use iroh::{EndpointId, SecretKey};
use rusqlite::Connection;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Barrier},
    time::Duration,
};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("coglet-store-{}", SecretKey::generate().public()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let out = Command::new("/usr/bin/git")
            .current_dir(&self.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        out.stdout
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Harness {
    authority: Authority,
    store: AcceptedRefStore,
    path: PathBuf,
    hub: EndpointId,
    peer: EndpointId,
    scope: Scope,
}
impl Harness {
    fn open(root: &Path) -> Self {
        let path = root.join("authority.sqlite");
        let authority = Authority::open(&path).unwrap();
        let store = AcceptedRefStore::new(authority.clone()).unwrap();
        Self {
            authority,
            store,
            path,
            hub: SecretKey::from([1; 32]).public(),
            peer: SecretKey::from([2; 32]).public(),
            scope: Scope {
                actor: format!("coglet:{}", SecretKey::from([4; 32]).public()),
                ticket: Some("ab".repeat(16)),
            },
        }
    }
    fn new(root: &Path) -> Self {
        let h = Self::open(root);
        h.authority.open_window(Duration::from_secs(30)).unwrap();
        let grants: Vec<_> = ["hub.git.push", "hub.git.fetch"]
            .map(|m| Grant {
                method: m.into(),
                target: Some(h.coglet()),
                expires: 100,
                generation: 7,
            })
            .into();
        let secret = h
            .authority
            .issue_invite(h.peer, Duration::from_secs(30), grants.clone())
            .unwrap();
        h.authority.enroll(h.peer, Some(&secret)).unwrap();
        for grant in grants {
            h.authority
                .provision_scope(h.peer, &h.scope, &grant)
                .unwrap();
        }
        let key = SecretKey::from([3; 32]);
        let statement = json!({"purpose":"delegation","coglet":h.coglet(),"hub":h.hub.to_string(),"device":h.peer.to_string(),"generation":9,"expires":100,"methods":["hub.git.push","hub.git.fetch"]});
        let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("delegation", &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
        h.authority
            .accept_delegation(h.hub, h.peer, &h.coglet(), &signed, || Some(1))
            .unwrap();
        h
    }
    fn coglet(&self) -> String {
        format!("coglet:{}", SecretKey::from([3; 32]).public())
    }
    fn context(&self, mode: Mode) -> Acceptance<'_> {
        Acceptance {
            hub: self.hub,
            coglet: self.coglet(),
            kind: RefKind::Logs,
            mode,
            peer: self.peer,
            scope: &self.scope,
            grant_generation: 7,
            delegation_generation: Some(9),
        }
    }
    fn current(&self) -> Option<coglet_proto::authority::accepted_refs::AcceptedRef> {
        self.store
            .current(&self.hub.to_string(), &self.coglet(), RefKind::Logs)
            .unwrap()
    }
    fn count(&self, table: &str) -> i64 {
        Connection::open(&self.path)
            .unwrap()
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap()
    }
}

fn fixture() -> Vec<u8> {
    fixture_data(b"{}\n")
}
fn fixture_data(data: &[u8]) -> Vec<u8> {
    let root = Root::new();
    root.git(&["init", "--initial-branch=logs"]);
    fs::create_dir(root.0.join("logs")).unwrap();
    fs::write(root.0.join("logs/events.jsonl"), data).unwrap();
    root.git(&["add", "."]);
    root.git(&["commit", "-m", "logs"]);
    root.git(&["bundle", "create", "transfer.bundle", "refs/heads/logs"]);
    fs::read(root.0.join("transfer.bundle")).unwrap()
}
fn metadata(bytes: &[u8], generation: u64, old: Option<&str>) -> VerifiedRef {
    // Extract the real bundle tip; archive from the same real Git commit.
    let root = Root::new();
    fs::write(root.0.join("input.bundle"), bytes).unwrap();
    root.git(&["clone", "--bare", "input.bundle", "repo.git"]);
    let archive = root.git(&[
        "--git-dir=repo.git",
        "archive",
        "--format=tar",
        "refs/heads/logs",
    ]);
    let tip = String::from_utf8(root.git(&["--git-dir=repo.git", "rev-parse", "refs/heads/logs"]))
        .unwrap();
    let key = SecretKey::from([3; 32]);
    let hub = SecretKey::from([1; 32]).public().to_string();
    let coglet = format!("coglet:{}", key.public());
    let statement = json!({"purpose":"logs-ref","hub":hub,"coglet":coglet,"ref":"refs/heads/logs","old":old,"new":tip.trim(),"archive_sha256":hex::encode(Sha256::digest(&archive)),"generation":generation});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("logs-ref", &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
    VerifiedRef::verify(&signed, RefKind::Logs, &hub, &coglet).unwrap()
}
async fn validated(root: &Root, bytes: &[u8], metadata: &VerifiedRef) -> VerifiedBundle {
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    for chunk in bytes.chunks(32768) {
        sink.append(chunk).await.unwrap();
    }
    let envelope = sink
        .check_envelope(metadata, &hex::encode(Sha256::digest(bytes)), 100000)
        .await
        .unwrap();
    envelope
        .validate(
            &Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator")),
            Limits::default(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn store_rejects_metadata_outside_trusted_routing_pins() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let m = metadata(&bytes, 1, None);
    for pin in ["hub", "coglet", "ref"] {
        let mut context = h.context(Mode::Observation);
        match pin {
            "hub" => context.hub = SecretKey::generate().public(),
            "coglet" => context.coglet = format!("coglet:{}", SecretKey::generate().public()),
            _ => context.kind = RefKind::Published,
        }
        assert!(
            matches!(
                h.store
                    .accept(validated(&root, &bytes, &m).await, &context, || Some(1)),
                Err(StoreError::Authority(_))
            ),
            "{pin}"
        );
    }
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
}

#[tokio::test]
async fn store_atomicity_rolls_back_audit_and_ref_on_write_failure() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let first = metadata(&bytes, 1, None);
    let db = Connection::open(&h.path).unwrap();
    db.execute_batch("CREATE TRIGGER refuse_ref BEFORE INSERT ON proto_accepted_refs BEGIN SELECT RAISE(ABORT,'injected write failure'); END;").unwrap();
    assert!(matches!(
        h.store.accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Observation),
            || Some(1)
        ),
        Err(StoreError::Storage)
    ));
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
    db.execute_batch("DROP TRIGGER refuse_ref").unwrap();
    let receipt = h
        .store
        .accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert_eq!(h.current().unwrap().audit_id, receipt.audit_id);
    assert_eq!(h.count("proto_ref_audit"), 1);
    assert_eq!(h.store.audit_bundle(receipt.audit_id).unwrap(), bytes);
    let newer_bytes = fixture_data(b"{\"event\":2}\n");
    let newer = metadata(&newer_bytes, 2, Some(first.new_tip()));
    db.execute_batch("CREATE TRIGGER refuse_update BEFORE UPDATE ON proto_accepted_refs BEGIN SELECT RAISE(ABORT,'injected update failure'); END;").unwrap();
    assert!(matches!(
        h.store.accept(
            validated(&root, &newer_bytes, &newer).await,
            &h.context(Mode::Observation),
            || Some(1)
        ),
        Err(StoreError::Storage)
    ));
    assert_eq!(h.current().unwrap().metadata.signed(), first.signed());
    assert_eq!(h.count("proto_ref_audit"), 1);
    db.execute_batch("DROP TRIGGER refuse_update").unwrap();
    h.store
        .accept(
            validated(&root, &newer_bytes, &newer).await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert_eq!(h.current().unwrap().metadata.new_tip(), newer.new_tip());
    assert_eq!(h.store.audit_bundle(receipt.audit_id).unwrap(), bytes);
}

#[tokio::test]
async fn store_cas_refuses_wrong_old_tip_and_authoritative_replay() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let first = metadata(&bytes, 1, None);
    h.store
        .accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Authoritative),
            || Some(1),
        )
        .unwrap();
    for bad in [
        first.clone(),
        metadata(&bytes, 2, Some(&"ff".repeat(20))),
        metadata(&bytes, 1, Some(first.new_tip())),
    ] {
        assert!(matches!(
            h.store.accept(
                validated(&root, &bytes, &bad).await,
                &h.context(Mode::Authoritative),
                || Some(1)
            ),
            Err(StoreError::Conflict)
        ));
    }
    let next = metadata(&bytes, u64::MAX, Some(first.new_tip()));
    h.store
        .accept(
            validated(&root, &bytes, &next).await,
            &h.context(Mode::Authoritative),
            || Some(1),
        )
        .unwrap();
    assert_eq!(h.current().unwrap().metadata.generation(), u64::MAX);
    assert_eq!(h.count("proto_ref_audit"), 2);
}

#[tokio::test]
async fn store_restart_preserves_watermark_statement_tip_and_retained_bundle() {
    let root = Root::new();
    let bytes = fixture();
    let first = metadata(&bytes, 7, Some(&"aa".repeat(20)));
    let receipt = {
        let h = Harness::new(&root.0);
        h.store
            .accept(
                validated(&root, &bytes, &first).await,
                &h.context(Mode::Observation),
                || Some(1),
            )
            .unwrap()
    };
    let h = Harness::open(&root.0);
    let recovered = h.current().unwrap();
    assert_eq!(recovered.metadata.signed(), first.signed());
    assert_eq!(recovered.audit_id, receipt.audit_id);
    assert_eq!(h.store.audit_bundle(receipt.audit_id).unwrap(), bytes);
    let stale = metadata(&bytes, 6, None);
    assert!(matches!(
        h.store.accept(
            validated(&root, &bytes, &stale).await,
            &h.context(Mode::Observation),
            || Some(1)
        ),
        Err(StoreError::Conflict)
    ));
    // Retained bytes traverse the actual validator again on recovery.
    let retry = h
        .store
        .accept(
            validated(
                &root,
                &h.store.audit_bundle(receipt.audit_id).unwrap(),
                &first,
            )
            .await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert!(!retry.changed);
    assert_eq!(retry.audit_id, receipt.audit_id);
}

#[tokio::test]
async fn store_replay_is_identical_only_and_never_rewinds() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let first = metadata(&bytes, 1, None);
    let r = h
        .store
        .accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    let retry = h
        .store
        .accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert_eq!(r.audit_id, retry.audit_id);
    assert!(!retry.changed);
    let altered = metadata(&bytes, 1, Some(&"bb".repeat(20)));
    assert!(matches!(
        h.store.accept(
            validated(&root, &bytes, &altered).await,
            &h.context(Mode::Observation),
            || Some(1)
        ),
        Err(StoreError::Conflict)
    ));
    let new_bytes = fixture_data(b"{\"event\":2}\n");
    let latest = metadata(&new_bytes, 3, Some(&"cc".repeat(20)));
    assert_ne!(first.new_tip(), latest.new_tip());
    h.store
        .accept(
            validated(&root, &new_bytes, &latest).await,
            &h.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert!(matches!(
        h.store.accept(
            validated(&root, &bytes, &first).await,
            &h.context(Mode::Observation),
            || Some(1)
        ),
        Err(StoreError::Conflict)
    ));
    assert_eq!(h.count("proto_ref_audit"), 2);
}

#[tokio::test]
async fn store_concurrent_authoritative_writers_have_one_cas_winner() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let bundles = [
        validated(&root, &bytes, &metadata(&bytes, 1, None)).await,
        validated(&root, &bytes, &metadata(&bytes, 2, None)).await,
    ];
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = bundles
        .into_iter()
        .map(|bundle| {
            let other = Harness::open(&root.0);
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                other
                    .store
                    .accept(bundle, &other.context(Mode::Authoritative), || Some(1))
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(StoreError::Conflict)))
            .count(),
        1
    );
    assert_eq!(h.count("proto_ref_audit"), 1);
    assert!(h.current().is_some());
}

#[tokio::test]
async fn store_concurrent_observers_cannot_overwrite_newer_completion() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let stale_metadata = metadata(&bytes, 2, None);
    assert!(stale_metadata.check_observation(None).is_ok()); // pre-credit result isn't a lease
    let stale_bundle = validated(&root, &bytes, &stale_metadata).await;
    let other = Harness::open(&root.0);
    let newer = metadata(&bytes, 3, None);
    other
        .store
        .accept(
            validated(&root, &bytes, &newer).await,
            &other.context(Mode::Observation),
            || Some(1),
        )
        .unwrap();
    assert!(matches!(
        h.store
            .accept(stale_bundle, &h.context(Mode::Observation), || Some(1)),
        Err(StoreError::Conflict)
    ));
    assert_eq!(h.current().unwrap().metadata.generation(), 3);
}

#[tokio::test]
async fn store_rechecks_revocation_after_validation_even_for_identical_retry() {
    for revoke in ["peer", "ticket", "delegation"] {
        let root = Root::new();
        let h = Harness::new(&root.0);
        let bytes = fixture();
        let m = metadata(&bytes, 1, None);
        h.store
            .accept(
                validated(&root, &bytes, &m).await,
                &h.context(Mode::Observation),
                || Some(1),
            )
            .unwrap();
        let retry = validated(&root, &bytes, &m).await;
        match revoke {
            "peer" => h.authority.revoke_peer(h.peer).unwrap(),
            "ticket" => h.authority.end_ticket(&h.scope).unwrap(),
            _ => h.authority.revoke_delegation(h.hub, &h.coglet()).unwrap(),
        }
        assert!(matches!(
            h.store
                .accept(retry, &h.context(Mode::Observation), || Some(1)),
            Err(StoreError::Authority(_))
        ));
        assert_eq!(h.count("proto_ref_audit"), 1);
    }
}

#[tokio::test]
async fn store_rechecks_generation_pins_and_exact_scope() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let m = metadata(&bytes, 1, None);
    let omitted = Scope {
        actor: h.scope.actor.clone(),
        ticket: None,
    };
    for mismatch in ["grant", "delegation", "scope", "clock"] {
        let mut context = h.context(Mode::Observation);
        match mismatch {
            "grant" => context.grant_generation = 8,
            "delegation" => context.delegation_generation = Some(10),
            "scope" => context.scope = &omitted,
            _ => (),
        }
        assert!(matches!(
            h.store
                .accept(validated(&root, &bytes, &m).await, &context, || {
                    if mismatch == "clock" { None } else { Some(1) }
                }),
            Err(StoreError::Authority(_))
        ));
    }
    let prepared = validated(&root, &bytes, &m).await;
    let key = SecretKey::from([3; 32]);
    let statement = json!({"purpose":"delegation","coglet":h.coglet(),"hub":h.hub.to_string(),"device":h.peer.to_string(),"generation":10,"expires":100,"methods":["hub.git.fetch"]});
    let signed = json!({"signature":hex::encode(key.sign(&signing::preimage("delegation", &statement).unwrap()).to_bytes()),"signer":key.public().to_string(),"statement":statement});
    h.authority
        .replace_delegation(h.hub, h.peer, &h.coglet(), &signed, || Some(1))
        .unwrap();
    assert!(matches!(
        h.store
            .accept(prepared, &h.context(Mode::Observation), || Some(1)),
        Err(StoreError::Authority(_))
    ));
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
    let mut refreshed = h.context(Mode::Observation);
    refreshed.delegation_generation = Some(10);
    h.store
        .accept(validated(&root, &bytes, &m).await, &refreshed, || Some(1))
        .unwrap();
}

#[tokio::test]
async fn store_expiry_at_final_check_rolls_back_every_write() {
    let root = Root::new();
    let h = Harness::new(&root.0);
    let bytes = fixture();
    let m = metadata(&bytes, 1, None);
    let clock = std::cell::Cell::new(0);
    let result = h.store.accept(
        validated(&root, &bytes, &m).await,
        &h.context(Mode::Observation),
        || {
            let call = clock.get();
            clock.set(call + 1);
            Some(if call == 0 { 1 } else { 100 })
        },
    );
    assert!(matches!(result, Err(StoreError::Authority(_))));
    assert!(h.current().is_none());
    assert_eq!(h.count("proto_ref_audit"), 0);
}

#[tokio::test]
async fn store_process_restart_recovers_before_and_after_commit() {
    let root = Root::new();
    let bytes = fixture();
    fs::write(root.0.join("retained.bundle"), &bytes).unwrap();
    {
        let h = Harness::new(&root.0);
        h.store
            .accept(
                validated(&root, &bytes, &metadata(&bytes, 1, None)).await,
                &h.context(Mode::Observation),
                || Some(1),
            )
            .unwrap();
    }
    for phase in ["before", "after"] {
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--ignored", "--exact", "store_crash_child", "--nocapture"])
            .env("COGLET_STORE_CRASH_ROOT", &root.0)
            .env("COGLET_STORE_CRASH_PHASE", phase)
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(73),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let h = Harness::open(&root.0);
        let expected = if phase == "before" { 1 } else { 2 };
        assert_eq!(h.current().unwrap().metadata.generation(), expected);
        assert_eq!(h.count("proto_ref_audit"), expected as i64);
        assert_eq!(
            h.store.audit_bundle(h.current().unwrap().audit_id).unwrap(),
            bytes
        );
    }
}

#[tokio::test]
#[ignore = "subprocess fixture, invoked by store_process_restart_recovers_before_and_after_commit"]
async fn store_crash_child() {
    let root = Root(PathBuf::from(
        std::env::var_os("COGLET_STORE_CRASH_ROOT").expect("fixture root"),
    ));
    let phase = std::env::var("COGLET_STORE_CRASH_PHASE").unwrap();
    let h = Harness::open(&root.0);
    let bytes = fs::read(root.0.join("retained.bundle")).unwrap();
    let calls = std::cell::Cell::new(0);
    h.store
        .accept(
            validated(&root, &bytes, &metadata(&bytes, 2, None)).await,
            &h.context(Mode::Observation),
            || {
                calls.set(calls.get() + 1);
                if phase == "before" && calls.get() == 2 {
                    std::process::exit(73);
                }
                Some(1)
            },
        )
        .unwrap();
    std::process::exit(73); // No Rust destructors / connection cleanup.
}
