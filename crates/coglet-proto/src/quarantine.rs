//! Raw bundle quarantine storage only; this is NOT Git validation or admission.
//! The owning directory must be private/trusted and on a durable filesystem.
use crate::transfer::DurableSink;

#[cfg(all(test, target_os = "linux"))]
mod custody_tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::sync::mpsc;
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[tokio::test]
    async fn inherited_lease_defers_drop_and_sweep_until_last_process_exits() {
        let parent = std::env::temp_dir().join(format!(
            "coglet-lease-{}",
            iroh::SecretKey::generate().public()
        ));
        fs::create_dir(&parent).unwrap();
        let sink = BundleSink::create(&parent, 1).await.unwrap();
        let lease = sink.0.as_ref().unwrap().export_lease().unwrap();
        let mut child = Child(
            Command::new("/bin/sleep")
                .arg("30")
                .stdin(Stdio::from(lease))
                .spawn()
                .unwrap(),
        );
        drop(sink);
        assert_eq!(fs::read_dir(&parent).unwrap().count(), 1);
        assert_eq!(BundleSink::sweep_orphans(&parent).await.unwrap(), 0);
        child.0.kill().unwrap();
        child.0.wait().unwrap();
        assert_eq!(BundleSink::sweep_orphans(&parent).await.unwrap(), 1);
        fs::remove_dir(&parent).unwrap();
    }

    fn metadata(tip: &str) -> crate::git_metadata::VerifiedRef {
        use ed25519_dalek::{Signer, SigningKey};
        let key = SigningKey::from_bytes(&[3; 32]);
        let hub = "11".repeat(32);
        let coglet = format!("coglet:{}", hex::encode(key.verifying_key().as_bytes()));
        let statement = serde_json::json!({"purpose":"logs-ref", "hub":hub,"coglet":coglet,
            "ref":"refs/heads/logs","old":null,"new":tip,
            "archive_sha256":"cd".repeat(32),"generation":1});
        let signature = key.sign(&crate::signing::preimage("logs-ref", &statement).unwrap());
        crate::git_metadata::VerifiedRef::verify(
            &serde_json::json!({"statement":statement,
                "signer":hex::encode(key.verifying_key().as_bytes()),
                "signature":hex::encode(signature.to_bytes())}),
            crate::git_metadata::RefKind::Logs,
            &hub,
            &coglet,
        )
        .unwrap()
    }

    /// Model a concurrent fork/spawn on another thread: it shares the
    /// unexported lease's open description until its exec closes the CLOEXEC
    /// copy. No timing is involved: the copy is released only once the kernel
    /// lists a cleanup blocked on this lease in /proc/locks (correct, waiting
    /// cleanup), or once the test signals that the rejection already returned
    /// (a non-waiting cleanup, which then fails the leftover assertion).
    fn transient_holder(sink: &BundleSink) -> (mpsc::Sender<()>, std::thread::JoinHandle<()>) {
        use std::os::unix::fs::MetadataExt;
        let state = sink.0.as_ref().unwrap();
        let copy = state._lease.as_ref().unwrap().try_clone().unwrap();
        assert!(!state.exported.load(Ordering::Acquire));
        let inode = copy.metadata().unwrap().ino().to_string();
        let (release, wait) = mpsc::channel();
        let holder = std::thread::spawn(move || {
            let blocked_on_lease = || {
                fs::read_to_string("/proc/locks")
                    .unwrap()
                    .lines()
                    .any(|line| {
                        line.contains("->")
                            && line.split_whitespace().any(|field| {
                                field.matches(':').count() == 2
                                    && field.rsplit(':').next() == Some(inode.as_str())
                            })
                    })
            };
            while wait.try_recv().is_err() && !blocked_on_lease() {
                std::thread::yield_now();
            }
            drop(copy);
        });
        (release, holder)
    }

    async fn truncated_sink(parent: &Path) -> (BundleSink, String) {
        let mut bytes = format!(
            "# v2 git bundle\n{} refs/heads/logs\n\nPACK",
            "ab".repeat(20)
        )
        .into_bytes();
        bytes.extend_from_slice(&2u32.to_be_bytes());
        bytes.extend_from_slice(&1u32.to_be_bytes());
        let mut sink = BundleSink::create(parent, bytes.len() as u64)
            .await
            .unwrap();
        sink.append(&bytes).await.unwrap();
        use sha2::Digest;
        (sink, hex::encode(sha2::Sha256::digest(&bytes)))
    }

    #[tokio::test]
    async fn rejections_remove_quarantine_before_returning_despite_forked_lease_copy() {
        let parent = std::env::temp_dir().join(format!(
            "coglet-reject-{}",
            iroh::SecretKey::generate().public()
        ));
        fs::create_dir(&parent).unwrap();
        let pinned = metadata(&"ab".repeat(20));
        let mut leftovers = Vec::new();
        for (case, expected) in [
            ("truncated pack", "truncated pack"),
            ("invalid hash", "invalid transfer hash"),
            ("mismatched header", "bundle header mismatch"),
            ("dropped sink", ""),
        ] {
            let (sink, hash) = truncated_sink(&parent).await;
            let (release, holder) = transient_holder(&sink);
            let message = match case {
                "dropped sink" => {
                    drop(sink);
                    String::new()
                }
                "invalid hash" => sink
                    .check_envelope(&pinned, "zz", 100000)
                    .await
                    .err()
                    .expect("rejected")
                    .to_string(),
                "mismatched header" => sink
                    .check_envelope(&metadata(&"cd".repeat(20)), &hash, 100000)
                    .await
                    .err()
                    .expect("rejected")
                    .to_string(),
                _ => sink
                    .check_envelope(&pinned, &hash, 100000)
                    .await
                    .err()
                    .expect("rejected")
                    .to_string(),
            };
            // Observe before releasing the simulated forked copy.
            let remaining = fs::read_dir(&parent).unwrap().count();
            let _ = release.send(());
            holder.join().unwrap();
            assert_eq!(message, expected, "{case}");
            leftovers.push((case, remaining));
            let _ = BundleSink::sweep_orphans(&parent).await;
        }
        fs::remove_dir_all(&parent).unwrap();
        assert!(
            leftovers.iter().all(|(_, remaining)| *remaining == 0),
            "{leftovers:?}"
        );
    }
}
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

struct State {
    directory: PathBuf,
    file: File,
    _lease: Option<File>,
    size: u64,
    limit: u64,
    failed: bool,
    /// Set once the lease is deliberately handed to a helper process.
    exported: AtomicBool,
}
impl State {
    fn export_lease(&self) -> io::Result<File> {
        let lease = self
            ._lease
            .as_ref()
            .ok_or_else(|| io::Error::other("quarantine discarded"))?;
        self.exported.store(true, Ordering::Release);
        lease.try_clone()
    }
}
impl Drop for State {
    fn drop(&mut self) {
        // Serialize with create/sweep across the metadata/open/remove sequence.
        // On gate failure leave the orphan intact for later recovery.
        let Some(parent) = self.directory.parent() else {
            return;
        };
        let Ok(gate) = File::open(parent) else { return };
        if gate.lock().is_err() {
            return;
        }
        // A helper/descendant inherits our locked open description. Never unlink
        // while one still owns it (including a killed task stuck in kernel I/O).
        // If service/helper dies first, startup sweep resumes cleanup only after
        // the final inherited descriptor closes. No accepted ref exists here.
        //
        // Before export no helper owns the lease, but a concurrent fork/spawn on
        // any thread still duplicates every descriptor until exec closes the
        // CLOEXEC copy. A non-blocking probe can lose to that transient holder
        // and leave rejected bytes behind, so wait for it: every pre-export
        // rejection returns only after removal. After export, keep deferring.
        let candidate = File::open(self.directory.join("lease"));
        drop(self._lease.take());
        let Ok(lease) = candidate else { return };
        let owned = if self.exported.load(Ordering::Acquire) {
            lease.try_lock().is_ok()
        } else {
            lease.lock().is_ok()
        };
        if owned {
            let _ = fs::remove_dir_all(&self.directory);
        }
    }
}

/// Owns a single bounded bundle file. Each append fsyncs before it returns.
/// Blocking I/O runs off the reactor. Cancellation cannot unlink a file while
/// its worker is writing: the worker retains cleanup custody until completion.
/// No pathname is exported, so callers cannot mistake raw bytes for a verified
/// Git artifact. Consume its envelope through `BundleEnvelope::validate` for
/// bounded fsck/archive/provenance validation (never authority or activation).
pub struct BundleSink(Option<State>);

/// A complete, hash-bound bundle envelope, NOT a validated Git artifact.
/// The pack remains untrusted: connectivity, decompression limits, fsck, tree,
/// archive and provenance checks are still required before durable acceptance.
/// No pathname or write API is exposed. Drop retains the sink's cleanup custody.
pub struct BundleEnvelope {
    _sink: BundleSink,
    _metadata: crate::git_metadata::VerifiedRef,
    objects: u32,
}
impl BundleEnvelope {
    pub(crate) fn directory(&self) -> io::Result<&Path> {
        self._sink
            .0
            .as_ref()
            .map(|s| s.directory.as_path())
            .ok_or_else(|| io::Error::other("quarantine discarded"))
    }
    pub(crate) fn metadata(&self) -> &crate::git_metadata::VerifiedRef {
        &self._metadata
    }
    pub(crate) fn lease(&self) -> io::Result<File> {
        self._sink
            .0
            .as_ref()
            .ok_or_else(|| io::Error::other("quarantine discarded"))?
            .export_lease()
    }

    /// Consume raw custody. Success proves bounded Git/tree/archive validation,
    /// not authority, anti-replay persistence, policy qualification or activation.
    pub async fn validate(
        self,
        validator: &crate::validator::Validator,
        limits: crate::validator::Limits,
    ) -> Result<crate::validator::VerifiedBundle, crate::validator::ValidationError> {
        crate::validator::validate(self, validator, limits).await
    }

    pub fn object_count(&self) -> u32 {
        self.objects
    }
}
impl BundleSink {
    /// Run once at service startup (also safe alongside live sinks). The private
    /// parent directory's advisory lock serializes creation and sweeping; each
    /// live artifact retains its own lease through finalization. Unrelated files
    /// and symlinks are never followed or removed.
    pub async fn sweep_orphans(parent: impl AsRef<Path>) -> io::Result<usize> {
        let parent = parent.as_ref().to_owned();
        tokio::task::spawn_blocking(move || {
            let gate = File::open(&parent)?;
            gate.lock()?;
            let mut removed = 0;
            for entry in fs::read_dir(&parent)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(id) = name.strip_prefix("git-") else {
                    continue;
                };
                if id.len() != 64
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    || !entry.file_type()?.is_dir()
                {
                    continue;
                }
                let lease = entry.path().join("lease");
                match fs::symlink_metadata(&lease) {
                    Ok(meta) if meta.file_type().is_file() => {
                        let lease = File::open(&lease)?;
                        match lease.try_lock() {
                            Ok(()) => {}
                            Err(std::fs::TryLockError::WouldBlock) => continue,
                            Err(std::fs::TryLockError::Error(error)) => return Err(error),
                        }
                        fs::remove_dir_all(entry.path())?;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        fs::remove_dir_all(entry.path())?;
                    }
                    Ok(_) => continue,
                    Err(error) => return Err(error),
                }
                removed += 1;
            }
            gate.sync_all()?;
            Ok(removed)
        })
        .await
        .map_err(io::Error::other)?
    }

    /// Consume write custody, bind the entire file hash and exact v2 header to
    /// pinned metadata, and reject the declared pack object count before import.
    /// This is only preflight; forged pack headers cannot establish Git validity.
    pub async fn check_envelope(
        self,
        metadata: &crate::git_metadata::VerifiedRef,
        transfer_sha256: &str,
        max_objects: u32,
    ) -> io::Result<BundleEnvelope> {
        use sha2::{Digest, Sha256};
        let mut expected_hash = [0; 32];
        if transfer_sha256.len() != 64
            || hex::decode_to_slice(transfer_sha256, &mut expected_hash).is_err()
            || hex::encode(expected_hash) != transfer_sha256
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid transfer hash",
            ));
        }
        if max_objects == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid object limit",
            ));
        }
        let metadata = metadata.clone();
        tokio::task::spawn_blocking(move || {
            let objects = {
                let state = self
                    .0
                    .as_ref()
                    .ok_or_else(|| io::Error::other("quarantine discarded"))?;
                if state.failed || state.size != state.limit {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "incomplete bundle",
                    ));
                }
                // An exact bounded header rejects prerequisites, extra refs,
                // capabilities, comments, alternate spellings and version 3.
                let header = format!(
                    "# v2 git bundle\n{} {}\n\n",
                    metadata.new_tip(),
                    metadata.signed()["statement"]["ref"]
                        .as_str()
                        .expect("validated ref")
                );
                let mut file = File::open(state.directory.join("bundle"))?;
                let mut prefix = vec![0; header.len() + 12];
                file.read_exact(&mut prefix)?;
                if prefix[..header.len()] != *header.as_bytes() {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bundle header mismatch",
                    ));
                }
                if state.limit <= (prefix.len() + 20) as u64 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "truncated pack"));
                }
                let pack = &prefix[header.len()..];
                let version = u32::from_be_bytes(pack[4..8].try_into().expect("four bytes"));
                if &pack[..4] != b"PACK" || !matches!(version, 2 | 3) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid pack header",
                    ));
                }
                let objects = u32::from_be_bytes(pack[8..12].try_into().expect("four bytes"));
                if objects == 0 {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "empty pack"));
                }
                if objects > max_objects {
                    return Err(io::Error::other("quarantine object limit"));
                }
                let mut hash = Sha256::new();
                hash.update(&prefix);
                let mut length = prefix.len() as u64;
                let mut buffer = [0; 32768];
                loop {
                    let n = file.read(&mut buffer)?;
                    if n == 0 {
                        break;
                    }
                    length += n as u64;
                    if length > state.limit {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "bundle size mismatch",
                        ));
                    }
                    hash.update(&buffer[..n]);
                }
                if length != state.limit || hash.finalize().as_slice() != expected_hash {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "bundle hash mismatch",
                    ));
                }
                objects
            };
            Ok(BundleEnvelope {
                _sink: self,
                _metadata: metadata,
                objects,
            })
        })
        .await
        .map_err(io::Error::other)?
    }

    pub async fn create(parent: impl AsRef<Path>, limit: u64) -> io::Result<Self> {
        if !(1..=67108864).contains(&limit) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid bundle limit",
            ));
        }
        let parent = parent.as_ref().to_owned();
        tokio::task::spawn_blocking(move || {
            let gate = File::open(&parent)?;
            gate.lock()?;
            let directory = parent.join(format!("git-{}", iroh::SecretKey::generate().public()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory)?;
            let initialized = (|| {
                let lease = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(directory.join("lease"))?;
                lease.lock()?;
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join("bundle"))?;
                file.sync_all()?;
                File::open(&directory)?.sync_all()?;
                File::open(&parent)?.sync_all()?;
                Ok::<_, io::Error>((file, lease))
            })();
            match initialized {
                Ok((file, lease)) => Ok(Self(Some(State {
                    directory,
                    file,
                    _lease: Some(lease),
                    size: 0,
                    limit,
                    failed: false,
                    exported: AtomicBool::new(false),
                }))),
                Err(error) => {
                    let _ = fs::remove_dir_all(directory);
                    Err(error)
                }
            }
        })
        .await
        .map_err(io::Error::other)?
    }
}
impl DurableSink for BundleSink {
    async fn append(&mut self, bytes: &[u8]) -> io::Result<()> {
        if bytes.is_empty() || bytes.len() > 32768 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid chunk size",
            ));
        }
        let state = self
            .0
            .as_ref()
            .ok_or_else(|| io::Error::other("quarantine discarded"))?;
        let end = state
            .size
            .checked_add(bytes.len() as u64)
            .filter(|n| *n <= state.limit)
            .ok_or_else(|| io::Error::other("quarantine byte limit"))?;
        let bytes = bytes.to_owned();
        // Move exclusive custody into the worker. A cancelled future never
        // restores the sink: the worker/result drops State after I/O finishes.
        let mut state = self.0.take().expect("checked state");
        let state = tokio::task::spawn_blocking(move || {
            if state.failed {
                return Err(io::Error::other("quarantine write previously failed"));
            }

            // Poison on any I/O failure; a partial write is never retried as if
            // it were an all-or-nothing append.
            state.failed = true;
            state.file.write_all(&bytes)?;
            state.file.sync_data()?;
            state.size = end;
            state.failed = false;
            Ok::<_, io::Error>(state)
        })
        .await
        .map_err(io::Error::other)??;
        self.0 = Some(state);
        Ok(())
    }
}
