use super::*;
use flate2::{Compression, write::ZlibEncoder};
use sha1::Sha1;
use std::{
    io::Write,
    time::{Duration, Instant},
};

// These are real attacker-authored bundle/pack bytes, with valid zlib streams,
// object IDs and pack checksum. Git need not agree to CREATE malicious objects.
struct Pack {
    raw: Vec<u8>,
    count: u32,
}
impl Pack {
    fn new() -> Self {
        Self {
            raw: b"PACK\0\0\0\x02\0\0\0\0".to_vec(),
            count: 0,
        }
    }
    fn entry(&mut self, typ: u8, base: &[u8], bytes: &[u8]) {
        let mut size = bytes.len();
        let first = ((typ & 7) << 4) | (size as u8 & 15);
        size >>= 4;
        self.raw.push(first | if size > 0 { 128 } else { 0 });
        while size > 0 {
            let b = size as u8 & 127;
            size >>= 7;
            self.raw.push(b | if size > 0 { 128 } else { 0 });
        }
        self.raw.extend_from_slice(base);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(bytes).unwrap();
        self.raw.extend(encoder.finish().unwrap());
        self.count += 1;
    }
    fn object(&mut self, kind: &str, bytes: &[u8]) -> [u8; 20] {
        let typ = match kind {
            "commit" => 1,
            "tree" => 2,
            "blob" => 3,
            _ => panic!("test kind"),
        };
        self.entry(typ, &[], bytes);
        object_id(kind, bytes)
    }
    fn finish(mut self, tip: [u8; 20]) -> (Vec<u8>, VerifiedRef) {
        self.raw[8..12].copy_from_slice(&self.count.to_be_bytes());
        let checksum = Sha1::digest(&self.raw);
        self.raw.extend_from_slice(&checksum);
        let mut bytes =
            format!("# v2 git bundle\n{} refs/heads/logs\n\n", hex::encode(tip)).into_bytes();
        bytes.extend(self.raw);
        let metadata = signed(
            &hex::encode(tip),
            b"archive intentionally unused by early refusal tests",
        );
        (bytes, metadata)
    }
}
fn object_id(kind: &str, bytes: &[u8]) -> [u8; 20] {
    let mut hash = Sha1::new();
    hash.update(format!("{kind} {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    hash.finalize().into()
}
fn tree_entry(mode: &str, name: &str, id: [u8; 20]) -> Vec<u8> {
    let mut bytes = format!("{mode} {name}\0").into_bytes();
    bytes.extend_from_slice(&id);
    bytes
}
fn commit(pack: &mut Pack, root: [u8; 20], author: &str) -> [u8; 20] {
    pack.object(
        "commit",
        format!(
            "tree {}\nauthor {author}\ncommitter Test <test@example.invalid> 1 +0000\n\ntest\n",
            hex::encode(root)
        )
        .as_bytes(),
    )
}
fn finish_logs(mut pack: Pack, logs: [u8; 20], author: &str) -> (Vec<u8>, VerifiedRef) {
    let root = pack.object("tree", &tree_entry("40000", "logs", logs));
    let tip = commit(&mut pack, root, author);
    pack.finish(tip)
}
async fn refuse(bytes: &[u8], metadata: &VerifiedRef, limits: Limits, suffix: &str) {
    let error = validate_bytes(bytes, metadata, limits).await.unwrap_err();
    assert!(
        error.to_string().ends_with(suffix),
        "expected {suffix}, got {error}"
    );
}

#[tokio::test]
async fn validator_malicious_delta_chain_hits_depth_before_reconstruction() {
    let mut pack = Pack::new();
    let mut blob = b"a".to_vec();
    let mut base = pack.object("blob", &blob);
    let mut tree = tree_entry("100644", "000.jsonl", base);
    for n in 1..=51 {
        let program = [
            blob.len() as u8,
            blob.len() as u8 + 1,
            0x90,
            blob.len() as u8,
            1,
            b'a',
        ];
        pack.entry(7, &base, &program);
        blob.push(b'a');
        base = object_id("blob", &blob);
        tree.extend(tree_entry("100644", &format!("{n:03}.jsonl"), base));
    }
    let logs = pack.object("tree", &tree);
    let (bytes, metadata) = finish_logs(pack, logs, "Test <test@example.invalid> 1 +0000");
    refuse(&bytes, &metadata, Limits::default(), "delta depth").await;
}

#[tokio::test]
async fn validator_rejects_missing_delta_base_and_delta_result_bomb() {
    let mut pack = Pack::new();
    pack.entry(7, &[0x77; 20], &[1, 2, 1, b'a', 1, b'b']);
    let (bytes, metadata) = pack.finish([0x88; 20]);
    refuse(
        &bytes,
        &metadata,
        Limits::default(),
        "missing/cyclic delta base",
    )
    .await;
    let mut pack = Pack::new();
    let base = pack.object("blob", b"a");
    // base size 1, reconstructed size 2^28. No result buffer may be allocated.
    pack.entry(7, &base, &[1, 0x80, 0x80, 0x80, 0x80, 1]);
    let (bytes, metadata) = pack.finish([0x88; 20]);
    refuse(&bytes, &metadata, Limits::default(), "expanded bytes").await;
}

#[tokio::test]
async fn validator_rejects_dag_archive_bomb_before_running_archive() {
    let mut pack = Pack::new();
    let blob = pack.object("blob", b"x");
    let mut tree = pack.object("tree", &tree_entry("100644", "event.jsonl", blob));
    for _ in 0..20 {
        let mut entries = tree_entry("40000", "a", tree);
        entries.extend(tree_entry("40000", "b", tree));
        tree = pack.object("tree", &entries);
    }
    let (bytes, metadata) = finish_logs(pack, tree, "Test <test@example.invalid> 1 +0000");
    refuse(
        &bytes,
        &metadata,
        Limits {
            expanded_bytes: 65536,
            ..Limits::default()
        },
        "expanded archive",
    )
    .await;
}

#[tokio::test]
async fn validator_strict_fsck_rejects_invalid_commit_identity() {
    let mut pack = Pack::new();
    let blob = pack.object("blob", b"{}\n");
    let logs = pack.object("tree", &tree_entry("100644", "events.jsonl", blob));
    let (bytes, metadata) = finish_logs(pack, logs, "not-an-identity");
    refuse(&bytes, &metadata, Limits::default(), "strict fsck").await;
}

#[tokio::test]
async fn validator_rejects_symlinks_gitlinks_and_traversal_without_checkout() {
    for (mode, name, expected) in [
        ("120000", "link", "symlink/gitlink/tree mode"),
        ("160000", "module", "symlink/gitlink/tree mode"),
        ("100644", "../escape", "forbidden tree path"),
        ("100644", "/absolute", "forbidden tree path"),
        ("100644", ".git", "forbidden tree path"),
    ] {
        let mut pack = Pack::new();
        let blob = pack.object("blob", b"outside");
        let logs = pack.object("tree", &tree_entry(mode, name, blob));
        let (bytes, metadata) = finish_logs(pack, logs, "Test <test@example.invalid> 1 +0000");
        refuse(&bytes, &metadata, Limits::default(), expected).await;
    }
}

#[tokio::test]
async fn validator_compression_bomb_is_bounded_independent_of_build_speed() {
    let mut pack = Pack::new();
    let blob = pack.object("blob", &vec![b'x'; 200 * 1024 * 1024]);
    let logs = pack.object("tree", &tree_entry("100644", "large.jsonl", blob));
    let (bytes, metadata) = finish_logs(pack, logs, "Test <test@example.invalid> 1 +0000");
    assert!(
        bytes.len() < 2 * 1024 * 1024,
        "actual compression amplification required"
    );
    let start = Instant::now();
    let error = validate_bytes(
        &bytes,
        &metadata,
        Limits {
            cpu_seconds: 3,
            ..Limits::default()
        },
    )
    .await
    .unwrap_err();
    // Debug builds often exhaust the CPU share; optimized builds may safely
    // finish parsing and reject the intentionally false signed archive digest.
    // Neither performance outcome proves the other's missing fence.
    assert!(
        match &error {
            coglet_proto::validator::ValidationError::Resource(message) => matches!(
                message.as_str(),
                "helper CPU/memory/process limit"
                    | "fsck CPU"
                    | "archive CPU"
                    | "validation wall time/cancelled"
            ),
            coglet_proto::validator::ValidationError::Invalid(message) =>
                message.ends_with("archive digest"),
            _ => false,
        },
        "{error}"
    );
    assert!(start.elapsed() < Duration::from_secs(35));
}

#[tokio::test]
async fn validator_kernel_cpu_fence_terminates_deterministic_busy_helper() {
    use std::os::unix::fs::PermissionsExt;
    let root = Root::new();
    let path = root.0.join("cpu-probe");
    // Trusted test executable, never a remote-selectable helper. Unlike a fixed
    // pack workload, this cannot finish before its limit on a faster build/CPU.
    fs::write(&path, b"#!/bin/sh\nwhile :; do :; done\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    let (bytes, metadata) = fixture();
    let start = Instant::now();
    let error = validate_using(
        &bytes,
        &metadata,
        Limits {
            cpu_seconds: 3,
            ..Limits::default()
        },
        &Validator::new(path),
    )
    .await
    .unwrap_err();
    assert!(
        matches!(error, coglet_proto::validator::ValidationError::Resource(_)),
        "{error}"
    );
    assert!(
        error
            .to_string()
            .ends_with("helper CPU/memory/process limit"),
        "{error}"
    );
    assert!(start.elapsed() < Duration::from_secs(10));
}

#[tokio::test]
async fn validator_wall_deadline_cleans_real_bundle_without_accepting() {
    let mut pack = Pack::new();
    let blob = pack.object("blob", &vec![b'x'; 16 * 1024 * 1024]);
    let logs = pack.object("tree", &tree_entry("100644", "large.jsonl", blob));
    let (bytes, metadata) = finish_logs(pack, logs, "Test <test@example.invalid> 1 +0000");
    refuse(
        &bytes,
        &metadata,
        Limits {
            wall_millis: 1,
            ..Limits::default()
        },
        "validation wall time/cancelled",
    )
    .await;
}
