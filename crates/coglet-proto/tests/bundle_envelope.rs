use coglet_proto::{
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    transfer::DurableSink,
};
use ed25519_dalek::{Signer, SigningKey};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, process::Command};

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "coglet-envelope-{}",
            iroh::SecretKey::generate().public()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn git(root: &Root, args: &[&str]) -> Vec<u8> {
    let output = Command::new("git")
        .current_dir(&root.0)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap())
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
fn metadata(tip: &str) -> VerifiedRef {
    let key = SigningKey::from_bytes(&[3; 32]);
    let hub = "11".repeat(32);
    let coglet = format!("coglet:{}", hex::encode(key.verifying_key().as_bytes()));
    let statement = json!({"purpose":"logs-ref", "hub":hub,"coglet":coglet,
        "ref":"refs/heads/logs","old":null,"new":tip,
        "archive_sha256":"cd".repeat(32),"generation":1});
    let signature = key.sign(&coglet_proto::signing::preimage("logs-ref", &statement).unwrap());
    VerifiedRef::verify(
        &json!({"statement":statement,
        "signer":hex::encode(key.verifying_key().as_bytes()),
        "signature":hex::encode(signature.to_bytes())}),
        RefKind::Logs,
        &hub,
        &coglet,
    )
    .unwrap()
}
async fn check(bytes: &[u8], hash: &str, limit: u32) -> std::io::Result<u32> {
    let root = Root::new();
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    for chunk in bytes.chunks(32768) {
        sink.append(chunk).await.unwrap();
    }
    let result = sink
        .check_envelope(&metadata(&"ab".repeat(20)), hash, limit)
        .await;
    let count = result.map(|envelope| envelope.object_count());
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    count
}
fn raw_pack(count: u32) -> Vec<u8> {
    let mut bytes = format!(
        "# v2 git bundle\n{} refs/heads/logs\n\nPACK",
        "ab".repeat(20)
    )
    .into_bytes();
    bytes.extend_from_slice(&2u32.to_be_bytes());
    bytes.extend_from_slice(&count.to_be_bytes());
    bytes
}
#[tokio::test]
async fn truncated_pack_cannot_pass_envelope_preflight() {
    let bytes = raw_pack(1);
    let error = check(&bytes, &hex::encode(Sha256::digest(&bytes)), 100000)
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(error.to_string(), "truncated pack");
}

#[tokio::test]
async fn envelope_rejections_are_bounded_atomic_and_specific() {
    let mut bytes = raw_pack(1);
    bytes.extend_from_slice(&[0; 32]); // deliberately invalid pack body; preflight never claims fsck
    let header_len = bytes.len() - 44;
    let mut cases = Vec::new();
    let mut version = bytes.clone();
    version[2] = b'3';
    cases.push((version, 100000, "bundle header mismatch"));
    let mut tip = bytes.clone();
    tip[16] = b'f';
    cases.push((tip, 100000, "bundle header mismatch"));
    let mut reference = bytes.clone();
    reference[header_len - 3] = b'x';
    cases.push((reference, 100000, "bundle header mismatch"));
    let mut extra_ref = bytes.clone();
    extra_ref.splice(
        header_len - 1..header_len - 1,
        b"extra ref\n".iter().copied(),
    );
    cases.push((extra_ref, 100000, "bundle header mismatch"));
    let mut prereq = bytes.clone();
    prereq.splice(16..16, b"-prerequisite\n".iter().copied());
    cases.push((prereq, 100000, "bundle header mismatch"));
    let mut magic = bytes.clone();
    magic[header_len] = b'X';
    cases.push((magic, 100000, "invalid pack header"));
    let mut pack_version = bytes.clone();
    pack_version[header_len + 7] = 4;
    cases.push((pack_version, 100000, "invalid pack header"));
    let mut empty = bytes.clone();
    empty[header_len + 11] = 0;
    cases.push((empty, 100000, "empty pack"));
    let mut excess = bytes.clone();
    excess[header_len + 8..header_len + 12].copy_from_slice(&100001u32.to_be_bytes());
    cases.push((excess, 100000, "quarantine object limit"));
    for (bytes, limit, expected) in cases {
        let error = check(&bytes, &hex::encode(Sha256::digest(&bytes)), limit)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), expected);
    }
    assert_eq!(
        check(&bytes, &"00".repeat(32), 100000)
            .await
            .unwrap_err()
            .to_string(),
        "bundle hash mismatch"
    );
    for bad_hash in ["AB".repeat(32), "00".repeat(33), "gg".repeat(32)] {
        assert_eq!(
            check(&bytes, &bad_hash, 100000)
                .await
                .unwrap_err()
                .to_string(),
            "invalid transfer hash"
        );
    }
    assert_eq!(
        check(&bytes, &hex::encode(Sha256::digest(&bytes)), 0)
            .await
            .unwrap_err()
            .to_string(),
        "invalid object limit"
    );
}

#[tokio::test]
async fn real_iroh_git_bundle_envelope_is_bound_to_signed_ref_and_retains_cleanup_custody() {
    let source = Root::new();
    git(&source, &["init", "--initial-branch=logs"]);
    fs::create_dir(source.0.join("logs")).unwrap();
    fs::write(source.0.join("logs/events.jsonl"), b"{}\n").unwrap();
    git(&source, &["add", "."]);
    git(&source, &["commit", "-m", "logs"]);
    let tip = String::from_utf8(git(&source, &["rev-parse", "HEAD"])).unwrap();
    git(
        &source,
        &["bundle", "create", "transfer.bundle", "refs/heads/logs"],
    );
    let bytes = fs::read(source.0.join("transfer.bundle")).unwrap();
    let root = Root::new();
    use coglet_proto::{
        transfer::{receive_chunks, send_chunks},
        transport::ALPN,
    };
    use iroh::{Endpoint, endpoint::presets};
    use tokio::io::AsyncReadExt;
    let listener = Endpoint::builder(presets::Minimal)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .unwrap();
    let dialer = Endpoint::builder(presets::Minimal).bind().await.unwrap();
    let accepting = listener.clone();
    let accepted = tokio::spawn(async move { accepting.accept().await.unwrap().await.unwrap() });
    let client = dialer.connect(listener.addr(), ALPN).await.unwrap();
    let server = accepted.await.unwrap();
    let connection = client.clone();
    let transferred = bytes.clone();
    let sender = tokio::spawn(async move {
        let (mut send, mut recv) = connection.open_bi().await.unwrap();
        send.write_all(&[42]).await.unwrap();
        send_chunks(
            &mut send,
            &mut recv,
            &"ab".repeat(16),
            transferred.len() as u64,
            &mut transferred.as_slice(),
        )
        .await
        .unwrap();
        send.finish().unwrap();
    });
    let (mut send, mut recv) = server.accept_bi().await.unwrap();
    assert_eq!(recv.read_u8().await.unwrap(), 42);
    let mut sink = BundleSink::create(&root.0, bytes.len() as u64)
        .await
        .unwrap();
    receive_chunks(
        &mut send,
        &mut recv,
        &"ab".repeat(16),
        bytes.len() as u64,
        &hex::encode(Sha256::digest(&bytes)),
        &mut sink,
    )
    .await
    .unwrap();
    assert!(recv.read(&mut [0]).await.unwrap().is_none());
    sender.await.unwrap();
    let envelope = sink
        .check_envelope(
            &metadata(tip.trim()),
            &hex::encode(Sha256::digest(&bytes)),
            100000,
        )
        .await
        .unwrap();
    assert_eq!(envelope.object_count(), 4);
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 1);
    drop(envelope);
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    dialer.close().await;
    listener.close().await;
}
