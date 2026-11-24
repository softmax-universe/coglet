#![cfg(target_os = "linux")]
use coglet_proto::{
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    transfer::DurableSink,
    validator::{Limits, Validator},
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};
#[path = "support/validator_attacks.rs"]
mod attacks;

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "coglet-validator-{}",
            iroh::SecretKey::generate().public()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn git(&self, args: &[&str]) -> Vec<u8> {
        let output = Command::new("/usr/bin/git")
            .current_dir(&self.0)
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        output.stdout
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn signed(tip: &str, archive: &[u8]) -> VerifiedRef {
    signed_as(tip, archive, RefKind::Logs)
}
fn signed_as(tip: &str, archive: &[u8], kind: RefKind) -> VerifiedRef {
    let key = SigningKey::from_bytes(&[3; 32]);
    let hub = "11".repeat(32);
    let coglet = format!("coglet:{}", hex::encode(key.verifying_key().as_bytes()));
    let statement = json!({"purpose":kind.purpose(),"hub":hub,"coglet":coglet,"ref":kind.reference(),"old":null,"new":tip,"archive_sha256":hex::encode(Sha256::digest(archive)),"generation":1});
    let signature = key.sign(&coglet_proto::signing::preimage(kind.purpose(), &statement).unwrap());
    VerifiedRef::verify(&json!({"statement":statement,"signer":hex::encode(key.verifying_key().as_bytes()),"signature":hex::encode(signature.to_bytes())}), kind, &hub, &coglet).unwrap()
}
fn fixture() -> (Vec<u8>, VerifiedRef) {
    let source = Root::new();
    source.git(&["init", "--initial-branch=logs"]);
    fs::create_dir(source.0.join("logs")).unwrap();
    fs::write(source.0.join("logs/events.jsonl"), b"{}\n").unwrap();
    source.git(&["add", "."]);
    source.git(&["commit", "-m", "logs"]);
    let tip = String::from_utf8(source.git(&["rev-parse", "HEAD"])).unwrap();
    let archive = source.git(&["archive", "--format=tar", "HEAD"]);
    source.git(&["bundle", "create", "transfer.bundle", "refs/heads/logs"]);
    (
        fs::read(source.0.join("transfer.bundle")).unwrap(),
        signed(tip.trim(), &archive),
    )
}
#[tokio::test]
async fn validator_accepts_real_delta_bundle() {
    let source = Root::new();
    source.git(&["init", "--initial-branch=logs"]);
    fs::create_dir(source.0.join("logs")).unwrap();
    let mut data = (0..20000)
        .map(|n| format!("event {n}: a long repeatable log line\n"))
        .collect::<String>();
    for n in 0..3 {
        data.push_str(&format!("tail {n}\n"));
        fs::write(source.0.join(format!("logs/{n}.jsonl")), &data).unwrap();
    }
    source.git(&["add", "."]);
    source.git(&["commit", "-m", "delta logs"]);
    source.git(&["repack", "-adf", "--depth=50", "--window=50"]);
    let index = fs::read_dir(source.0.join(".git/objects/pack"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.extension().is_some_and(|e| e == "idx"))
        .unwrap();
    let listing =
        String::from_utf8(source.git(&["verify-pack", "-v", index.to_str().unwrap()])).unwrap();
    assert!(
        listing
            .lines()
            .any(|line| line.split_whitespace().count() == 7),
        "fixture must contain actual deltas: {listing}"
    );
    let tip = String::from_utf8(source.git(&["rev-parse", "HEAD"])).unwrap();
    let archive = source.git(&["archive", "--format=tar", "HEAD"]);
    source.git(&["bundle", "create", "transfer.bundle", "refs/heads/logs"]);
    let bytes = fs::read(source.0.join("transfer.bundle")).unwrap();
    let metadata = signed(tip.trim(), &archive);
    let root = Root::new();
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    for chunk in bytes.chunks(32768) {
        sink.append(chunk).await.unwrap();
    }
    let envelope = sink
        .check_envelope(&metadata, &hex::encode(Sha256::digest(&bytes)), 100000)
        .await
        .unwrap();
    let validator = Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator"));
    envelope
        .validate(&validator, Limits::default())
        .await
        .unwrap();
}

#[tokio::test]
async fn validator_accepts_real_logs_bundle_and_retains_private_cleanup_custody() {
    let (bytes, metadata) = fixture();
    let root = Root::new();
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    for chunk in bytes.chunks(32768) {
        sink.append(chunk).await.unwrap();
    }
    let envelope = sink
        .check_envelope(&metadata, &hex::encode(Sha256::digest(&bytes)), 100000)
        .await
        .unwrap();
    let validator = Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator"));
    let verified = envelope
        .validate(&validator, Limits::default())
        .await
        .unwrap();
    assert_eq!(verified.metadata().new_tip(), metadata.new_tip());
    assert_eq!(BundleSink::sweep_orphans(&root.0).await.unwrap(), 0);
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    drop(verified);
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
}

#[tokio::test]
async fn validator_cancellation_keeps_cleanup_with_worker_until_helper_stops() {
    use std::{
        future::Future,
        pin::pin,
        task::{Context, Poll, Waker},
    };
    let (bytes, metadata) = fixture();
    let root = Root::new();
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    for chunk in bytes.chunks(32768) {
        sink.append(chunk).await.unwrap();
    }
    let envelope = sink
        .check_envelope(&metadata, &hex::encode(Sha256::digest(&bytes)), 100000)
        .await
        .unwrap();
    let validator = Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator"));
    {
        let mut future = pin!(envelope.validate(&validator, Limits::default()));
        assert!(matches!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
    }
    for _ in 0..100 {
        BundleSink::sweep_orphans(&root.0).await.unwrap();
        if fs::read_dir(&root.0).unwrap().count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
}

async fn validate_bytes(
    bytes: &[u8],
    metadata: &VerifiedRef,
    limits: Limits,
) -> Result<(), coglet_proto::validator::ValidationError> {
    validate_using(
        bytes,
        metadata,
        limits,
        &Validator::new(env!("CARGO_BIN_EXE_coglet-git-validator")),
    )
    .await
}

async fn validate_using(
    bytes: &[u8],
    metadata: &VerifiedRef,
    limits: Limits,
    validator: &Validator,
) -> Result<(), coglet_proto::validator::ValidationError> {
    let root = Root::new();
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
    let result = envelope.validate(validator, limits).await;
    let result = result.map(drop);
    // Killed descendants can retain the inherited lease until kernel teardown.
    // Exercise real orphan recovery, never bypass the lease or unlink directly.
    for _ in 0..100 {
        BundleSink::sweep_orphans(&root.0).await.unwrap();
        if fs::read_dir(&root.0).unwrap().count() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        fs::read_dir(&root.0).unwrap().count(),
        0,
        "rejection must clean custody"
    );
    result
}
fn finish_fixture(source: &Root, kind: RefKind) -> (Vec<u8>, VerifiedRef) {
    source.git(&["add", "."]);
    source.git(&["commit", "-m", "fixture"]);
    source.git(&["update-ref", kind.reference(), "HEAD"]);
    let tip = String::from_utf8(source.git(&["rev-parse", "HEAD"])).unwrap();
    let archive = source.git(&["archive", "--format=tar", "HEAD"]);
    source.git(&["bundle", "create", "transfer.bundle", kind.reference()]);
    (
        fs::read(source.0.join("transfer.bundle")).unwrap(),
        signed_as(tip.trim(), &archive, kind),
    )
}

#[tokio::test]
async fn validator_enforces_expanded_object_and_disk_fences() {
    let (bytes, metadata) = fixture();
    for (limits, reason) in [
        (
            Limits {
                expanded_bytes: 100,
                ..Limits::default()
            },
            "expanded bytes",
        ),
        (
            Limits {
                objects: 3,
                ..Limits::default()
            },
            "object count",
        ),
        (
            Limits {
                disk_bytes: 32768,
                ..Limits::default()
            },
            "disk",
        ),
    ] {
        let error = validate_bytes(&bytes, &metadata, limits).await.unwrap_err();
        assert!(
            matches!(error, coglet_proto::validator::ValidationError::Resource(_)),
            "{error}"
        );
        assert!(error.to_string().ends_with(reason), "{error}");
    }
}

#[tokio::test]
async fn validator_rejects_archive_digest_and_pack_corruption() {
    let (mut bytes, metadata) = fixture();
    let wrong = signed(metadata.new_tip(), b"not the signed archive");
    assert!(
        validate_bytes(&bytes, &wrong, Limits::default())
            .await
            .unwrap_err()
            .to_string()
            .ends_with("archive digest")
    );
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    assert!(
        validate_bytes(&bytes, &metadata, Limits::default())
            .await
            .unwrap_err()
            .to_string()
            .ends_with("pack checksum")
    );
}

#[tokio::test]
async fn validator_rejects_forbidden_paths_and_logs_code_smuggling() {
    for name in [
        "data/private",
        "logs/data/secret",
        "logs/.gitattributes",
        "logs/.gitmodules",
        "src/program.rs",
    ] {
        let source = Root::new();
        source.git(&["init", "--initial-branch=logs"]);
        let path = source.0.join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"secret").unwrap();
        let (bytes, metadata) = finish_fixture(&source, RefKind::Logs);
        let error = validate_bytes(&bytes, &metadata, Limits::default())
            .await
            .unwrap_err();
        assert!(
            error.to_string().ends_with(if name.starts_with("src") {
                "logs allowlist"
            } else {
                "forbidden tree path"
            }),
            "{name}: {error}"
        );
    }
}

fn code_fixture(wasm: &[u8], provenance: Option<&[u8]>) -> (Vec<u8>, VerifiedRef) {
    let source = Root::new();
    source.git(&["init", "--initial-branch=code"]);
    fs::create_dir(source.0.join("bin")).unwrap();
    fs::write(source.0.join("bin/step.wasm"), wasm).unwrap();
    if let Some(record) = provenance {
        fs::create_dir(source.0.join("build")).unwrap();
        fs::write(source.0.join("build/provenance.cbor"), record).unwrap();
    }
    finish_fixture(&source, RefKind::Published)
}
fn provenance(wasm: &[u8]) -> Vec<u8> {
    coglet_types::HubBuildRecord {
        version: "0.1".into(),
        source_commit: "ab".repeat(20),
        recipe_sha256: "11".repeat(32),
        policy_sha256: "22".repeat(32),
        transform_sha256: "33".repeat(32),
        original_sha256: hex::encode(Sha256::digest(wasm)),
        rewritten_sha256: hex::encode(Sha256::digest(wasm)),
    }
    .to_cbor()
    .unwrap()
}
#[tokio::test]
async fn validator_rejects_component_even_with_matching_provenance() {
    let component = b"\0asm\x0d\0\x01\0";
    let record = provenance(component);
    let (bytes, metadata) = code_fixture(component, Some(&record));
    assert!(
        validate_bytes(&bytes, &metadata, Limits::default())
            .await
            .unwrap_err()
            .to_string()
            .ends_with("invalid Wasm")
    );
}

#[tokio::test]
async fn validator_code_requires_valid_wasm_and_canonical_output_bound_provenance() {
    let wasm = b"\0asm\x01\0\0\0";
    let record = provenance(wasm);
    let (bytes, metadata) = code_fixture(wasm, Some(&record));
    validate_bytes(&bytes, &metadata, Limits::default())
        .await
        .unwrap();
    for (module, record, reason) in [
        (
            b"invalid".as_slice(),
            Some(record.as_slice()),
            "invalid Wasm",
        ),
        (wasm.as_slice(), None, "missing provenance"),
        (
            wasm.as_slice(),
            Some(b"not cbor".as_slice()),
            "provenance schema/canonical encoding",
        ),
    ] {
        let (bytes, metadata) = code_fixture(module, record);
        assert!(
            validate_bytes(&bytes, &metadata, Limits::default())
                .await
                .unwrap_err()
                .to_string()
                .ends_with(reason)
        );
    }
    let wrong = provenance(b"different wasm");
    let (bytes, metadata) = code_fixture(wasm, Some(&wrong));
    assert!(
        validate_bytes(&bytes, &metadata, Limits::default())
            .await
            .unwrap_err()
            .to_string()
            .ends_with("provenance output digest")
    );
}
