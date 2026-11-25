#![cfg(target_os = "linux")]
use coglet_hub::{
    Error, Store,
    build::{Artifact, BuildRequest, Builder, Candidate, Delegation, Profile, SignRequest, Target},
};
use coglet_proto::{authority::Authority, validator::Validator};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .current_dir(path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
const WAT: &str = "(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))";
struct Fixture {
    dir: TempDir,
    builder: Builder,
    request: BuildRequest,
}
fn fixture() -> Fixture {
    fixture_with(WAT, None)
}
fn fixture_with(wat: &str, output_pin: Option<&str>) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let coglet = format!("coglet:{}", iroh::SecretKey::from_bytes(&[7; 32]).public());
    let manifest = json!({"version":"0.1","id":coglet,"subscriptions":[],"required_capabilities":[],"exclusive_outputs":[],"budgets":{"compute":10000,"compute_mode":"wamr.interpreter","memory_pages":2,"wall_time_us":100000,"max_events":16,"host_bytes":65536}});
    fs::write(dir.path().join("guest.wat"), wat).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    git(dir.path(), &["init", "--initial-branch=source"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "pinned source"]);
    let source_commit = git(dir.path(), &["rev-parse", "HEAD"]);
    let output_hash = hash(&wat::parse_str(wat).unwrap());
    let profile = Profile::integer_wat(
        "integer-v1",
        hash(wat.as_bytes()),
        hash(&serde_json::to_vec(&manifest).unwrap()),
        output_pin.unwrap_or(&output_hash).into(),
    );
    let target = Target {
        repository: dir.path().into(),
        coglet,
        device: iroh::SecretKey::from_bytes(&[9; 32]).public().to_string(),
    };
    let builder = Builder::new(target, profile).unwrap();
    Fixture {
        dir,
        builder,
        request: BuildRequest {
            source_commit,
            profile_id: "integer-v1".into(),
        },
    }
}
struct Signing {
    root: TempDir,
    store: Store,
    authority: Authority,
    hub: iroh::SecretKey,
    provision: Delegation,
    request: SignRequest,
    validator: Validator,
}
impl Signing {
    fn new() -> Self {
        let hub = iroh::SecretKey::from_bytes(&[8; 32]);
        // Coglet key exists ONLY in fixture provisioning; never passed to builder.
        let key = iroh::SecretKey::from_bytes(&[7; 32]);
        let statement = json!({"purpose":"delegation","hub":hub.public().to_string(),"coglet":format!("coglet:{}",key.public()),"device":iroh::SecretKey::from_bytes(&[9;32]).public().to_string(),"generation":1,"expires":1000,"methods":["hub.git.push","hub.install","hub.start"]});
        let signature =
            key.sign(&coglet_proto::signing::preimage("delegation", &statement).unwrap());
        let provision = Delegation {
            id: "fixture-delegation".into(),
            signed: json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())}),
        };
        drop(key);
        let request = SignRequest {
            uid: 42,
            operation_id: "ab".repeat(16),
            delegation_id: provision.id.clone(),
            old_tip: None,
            generation: 1,
        };
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("kernel.db");
        let store = Store::open(&database).unwrap();
        let authority = Authority::open(&database).unwrap();
        authority
            .accept_delegation(
                hub.public(),
                iroh::SecretKey::from_bytes(&[9; 32]).public(),
                provision.signed["statement"]["coglet"].as_str().unwrap(),
                &provision.signed,
                || Some(100),
            )
            .unwrap();
        let validator = Validator::new(env!("CARGO_BIN_EXE_coglet-hub-build-validator"));
        Self {
            root,
            store,
            authority,
            hub,
            provision,
            request,
            validator,
        }
    }
    fn sign(
        &mut self,
        candidate: Candidate,
        clock: impl Fn() -> Option<u64>,
    ) -> coglet_hub::Result<Artifact> {
        candidate.sign_and_retain(
            &mut self.store,
            &self.request,
            &self.provision,
            &self.hub,
            &self.authority,
            &self.validator,
            self.root.path(),
            clock,
        )
    }
    fn absent(&self) {
        assert!(
            self.store
                .build_artifact(self.request.uid, &self.request.operation_id)
                .unwrap()
                .is_none()
        );
    }
}
#[test]
fn digest_addressed_build_evidence_roundtrip() {
    let f = fixture();
    let mut s = Signing::new();
    let candidate = f.builder.compile(&f.request).unwrap();
    let a = s.sign(candidate, || Some(100)).unwrap();
    let bytes = s
        .store
        .build_evidence(a.provenance["original_sha256"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(bytes, wat::parse_str(WAT).unwrap());

    assert_eq!(s.store.build_evidence(&"00".repeat(32)).unwrap(), None);
}
#[test]
fn real_pinned_integer_fixture_compiles_with_exact_provenance() {
    let f = fixture();
    let artifact = f.builder.compile(&f.request).unwrap();
    assert_eq!(artifact.wasm(), wat::parse_str(WAT).unwrap());
    assert_eq!(
        artifact.provenance()["source_commit"],
        f.request.source_commit
    );
    assert_eq!(
        artifact.provenance()["original_sha256"],
        hash(artifact.wasm())
    );
    assert_eq!(
        artifact.provenance()["rewritten_sha256"],
        hash(artifact.wasm())
    );
    assert!(
        !f.dir.path().join("bin/step.wasm").exists(),
        "source repository stays untouched"
    );
}
#[test]
fn verified_fixture_is_signed_and_restart_returns_identical_artifact() {
    if let Some(root) = std::env::var_os("COGLET_BUILD_REOPEN_FIXTURE") {
        let root = std::path::PathBuf::from(root);
        let artifact = Store::open(root.join("kernel.db"))
            .unwrap()
            .build_artifact(42, &"ab".repeat(16))
            .unwrap()
            .unwrap();
        fs::write(root.join("reopened.bundle"), &artifact.bundle).unwrap();
        fs::write(
            root.join("reopened.metadata"),
            serde_json::to_vec(&artifact).unwrap(),
        )
        .unwrap();
        return;
    }
    let f = fixture();
    let mut s = Signing::new();
    let retained = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    coglet_proto::signing::verify(&retained.signed, "hub-ref", &s.hub.public().to_string())
        .unwrap();
    assert!(!retained.bundle.is_empty());
    let database = s.root.path().join("kernel.db");
    drop(s.store);
    let child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "verified_fixture_is_signed_and_restart_returns_identical_artifact",
            "--nocapture",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", s.root.path())
        .env("COGLET_BUILD_REOPEN_FIXTURE", s.root.path())
        .output()
        .unwrap();
    assert!(
        child.status.success(),
        "{} {}",
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr)
    );
    assert_eq!(
        retained.bundle,
        fs::read(s.root.path().join("reopened.bundle")).unwrap()
    );
    assert_eq!(
        serde_json::to_vec(&retained).unwrap(),
        fs::read(s.root.path().join("reopened.metadata")).unwrap()
    );
    s.store = Store::open(&database).unwrap();
    let recovered = Store::open(database)
        .unwrap()
        .build_artifact(42, &s.request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(retained, recovered);
    for (field, input) in [
        ("recipe_sha256", "recipe"),
        ("policy_sha256", "policy"),
        ("transform_sha256", "transform"),
        ("original_sha256", "original"),
        ("rewritten_sha256", "rewritten"),
    ] {
        let bytes = hex::decode(retained.reproduction[input].as_str().unwrap()).unwrap();
        assert_eq!(retained.provenance[field], hash(&bytes), "{field}");
    }
    // Identical retry returns the retained bytes even after expiry; this is
    // retrieval, never fresh signing or dispatch authority.
    assert_eq!(
        retained,
        s.sign(f.builder.compile(&f.request).unwrap(), || None)
            .unwrap()
    );
    assert!(
        s.store
            .build_artifact(43, &s.request.operation_id)
            .unwrap()
            .is_none()
    );
    s.request.generation += 1;
    assert!(matches!(
        s.sign(f.builder.compile(&f.request).unwrap(), || Some(100)),
        Err(Error::Conflict(_))
    ));
}
#[test]
fn tampered_pinned_source_and_manifest_are_denied_before_compilation() {
    for name in ["guest.wat", "manifest.json"] {
        let mut f = fixture();
        let mut bytes = fs::read(f.dir.path().join(name)).unwrap();
        bytes.push(b' ');
        fs::write(f.dir.path().join(name), bytes).unwrap();
        git(f.dir.path(), &["add", name]);
        git(f.dir.path(), &["commit", "-m", "tamper"]);
        f.request.source_commit = git(f.dir.path(), &["rev-parse", "HEAD"]);
        assert!(
            matches!(f.builder.compile(&f.request), Err(Error::Denied)),
            "{name}"
        );
    }
}
#[test]
fn unqualified_profile_and_wrong_output_gate_pin_are_denied() {
    let mut f = fixture();
    f.request.profile_id = "caller-selected-unqualified".into();
    assert!(matches!(f.builder.compile(&f.request), Err(Error::Denied)));
    let f = fixture_with(WAT, Some(&"00".repeat(32)));
    assert!(matches!(f.builder.compile(&f.request), Err(Error::Denied)));
}
#[test]
fn pinned_float_simd_and_ambient_import_sources_remain_unqualified() {
    for addition in [
        "(func (result f32) f32.const 0)",
        "(func (result v128) v128.const i32x4 0 0 0 0)",
    ] {
        let wat = format!("{} {addition})", &WAT[..WAT.len() - 1]);
        let f = fixture_with(&wat, None);
        assert!(matches!(f.builder.compile(&f.request), Err(Error::Denied)));
    }
    let wat=WAT.replacen("(module", "(module (import \"wasi_snapshot_preview1\" \"clock_time_get\" (func (param i32 i64 i32) (result i32)))",1);
    let f = fixture_with(&wat, None);
    assert!(matches!(f.builder.compile(&f.request), Err(Error::Denied)));
}
#[test]
fn expired_unknown_and_revoked_delegations_never_retain_signed_output() {
    for variant in [
        "expiry",
        "clock-loss",
        "revoked",
        "wrong-ref",
        "tampered-delegation",
        "missing-helper",
    ] {
        let f = fixture();
        let mut s = Signing::new();
        match variant {
            "revoked" => s
                .authority
                .revoke_delegation(
                    s.hub.public(),
                    s.provision.signed["statement"]["coglet"].as_str().unwrap(),
                )
                .unwrap(),
            "wrong-ref" => s.request.delegation_id = "unknown".into(),
            "tampered-delegation" => s.provision.signed["statement"]["expires"] = json!(2000),
            "missing-helper" => {
                s.validator = Validator::new(s.root.path().join("missing-validator"))
            }
            _ => {}
        }
        let now = match variant {
            "expiry" => Some(1000),
            "clock-loss" => None,
            _ => Some(100),
        };
        assert!(
            s.sign(f.builder.compile(&f.request).unwrap(), || now)
                .is_err(),
            "{variant}"
        );
        s.absent();
    }
}
#[test]
fn delegation_revocation_during_validation_is_rechecked_before_router_signing() {
    let f = fixture();
    let mut s = Signing::new();
    let authority = s.authority.clone();
    let hub = s.hub.public();
    let coglet = s.provision.signed["statement"]["coglet"]
        .as_str()
        .unwrap()
        .to_owned();
    let calls = std::cell::Cell::new(0);
    assert!(
        s.sign(f.builder.compile(&f.request).unwrap(), || {
            let call = calls.get() + 1;
            calls.set(call);
            if call == 3 {
                authority.revoke_delegation(hub, &coglet).unwrap();
            }
            Some(100)
        })
        .is_err()
    );
    assert_eq!(calls.get(), 3);
    s.absent();
}
#[test]
fn final_publication_authority_is_checked_inside_retention_transaction() {
    let f = fixture();
    let mut s = Signing::new();
    let database = s.root.path().join("kernel.db");
    let calls = std::cell::Cell::new(0);
    let fenced = std::cell::Cell::new(false);
    s.sign(f.builder.compile(&f.request).unwrap(), || {
        let call = calls.get() + 1;
        calls.set(call);
        if call >= 3 {
            let probe = rusqlite::Connection::open(&database).unwrap();
            probe.busy_timeout(std::time::Duration::ZERO).unwrap();
            let blocked = probe.execute_batch("BEGIN IMMEDIATE").is_err();
            if !blocked {
                probe.execute_batch("ROLLBACK").unwrap();
            }
            fenced.set(blocked);
        }
        Some(100)
    })
    .unwrap();
    assert!(
        fenced.get(),
        "final authority sampling must hold the Store write fence"
    );
}
#[test]
fn key_isolation_source_scripts_and_repository_hooks_cannot_run() {
    let mut f = fixture();
    let mut s = Signing::new();
    let marker = s.root.path().join("leaked-key");
    let script = format!("#!/bin/sh\nprintf leaked > '{}'\n", marker.display());
    // This executable is malicious fixture source, not a trusted build recipe.
    fs::write(f.dir.path().join("build.sh"), &script).unwrap();
    fs::write(f.dir.path().join(".git/hooks/post-commit"), &script).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(
        f.dir.path().join("build.sh"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    // Use a different hook location only after pinning the malicious source.
    git(f.dir.path(), &["add", "build.sh"]);
    git(f.dir.path(), &["commit", "-m", "untrusted build script"]);
    f.request.source_commit = git(f.dir.path(), &["rev-parse", "HEAD"]);
    fs::set_permissions(
        f.dir.path().join(".git/hooks/post-commit"),
        fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    let artifact = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    assert!(
        !marker.exists(),
        "no source scripts or repository hooks execute"
    );
    // Publication holds only public delegation bytes, never the coglet secret.
    assert!(artifact.reproduction["delegation"]["signed"]["signature"].is_string());
    let coglet_secret = hex::encode([7u8; 32]);
    assert!(
        !serde_json::to_string(&artifact.reproduction)
            .unwrap()
            .contains(&coglet_secret)
    );
    assert!(
        !artifact
            .bundle
            .windows(coglet_secret.len())
            .any(|w| w == coglet_secret.as_bytes())
    );
}
#[test]
fn retained_artifact_cannot_be_replaced_with_sql_replace() {
    let f = fixture();
    let mut s = Signing::new();
    let artifact = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    let db = rusqlite::Connection::open(s.root.path().join("kernel.db")).unwrap();
    let result=db.execute("INSERT OR REPLACE INTO build_artifacts SELECT uid,id,intent,metadata,bundle,bundle_sha256 FROM build_artifacts",[]);
    assert!(
        result.is_err(),
        "REPLACE must not delete and replace retained custody"
    );
    assert_eq!(
        artifact,
        s.store
            .build_artifact(42, &s.request.operation_id)
            .unwrap()
            .unwrap()
    );
}
#[test]
fn artifact_capacity_refuses_new_custody_without_evicting_retained_bytes() {
    let f = fixture();
    let mut s = Signing::new();
    let first = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    let first_id = s.request.operation_id.clone();
    for index in 0..63 {
        s.request.operation_id = format!("{index:032x}");
        s.sign(f.builder.compile(&f.request).unwrap(), || Some(100))
            .unwrap();
    }
    s.request.operation_id = "cd".repeat(16);
    assert!(matches!(
        s.sign(f.builder.compile(&f.request).unwrap(), || Some(100)),
        Err(Error::Full)
    ));
    s.absent();
    assert_eq!(
        first,
        s.store.build_artifact(42, &first_id).unwrap().unwrap()
    );
}
#[test]
fn artifact_and_audit_rollback_together_then_exact_retry_succeeds() {
    let f = fixture();
    let mut s = Signing::new();
    let db = rusqlite::Connection::open(s.root.path().join("kernel.db")).unwrap();
    db.execute_batch("CREATE TRIGGER injected_build_audit_failure BEFORE INSERT ON audit BEGIN SELECT RAISE(ABORT,'test'); END;").unwrap();
    assert!(matches!(
        s.sign(f.builder.compile(&f.request).unwrap(), || Some(100)),
        Err(Error::Storage(_))
    ));
    s.absent();
    db.execute_batch("DROP TRIGGER injected_build_audit_failure;")
        .unwrap();
    let artifact = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    assert_eq!(
        artifact,
        s.store
            .build_artifact(42, &s.request.operation_id)
            .unwrap()
            .unwrap()
    );
}
