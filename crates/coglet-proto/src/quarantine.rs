//! Raw bundle quarantine storage only; this is NOT Git validation or admission.
//! The owning directory must be private/trusted and on a durable filesystem.
use crate::transfer::DurableSink;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

struct State {
    directory: PathBuf,
    file: File,
    size: u64,
    limit: u64,
    failed: bool,
}
impl Drop for State {
    fn drop(&mut self) {
        // Only the randomly named directory created by us, never the parent.
        // A crash leaves an orphan for startup cleanup, not an imported ref.
        let _ = fs::remove_dir_all(&self.directory);
    }
}

/// Owns a single bounded bundle file. Each append fsyncs before it returns.
/// Blocking I/O runs off the reactor. Cancellation cannot unlink a file while
/// its worker is writing: the worker retains cleanup custody until completion.
/// No pathname is exported, so callers cannot mistake raw bytes for a verified
/// Git artifact. Full fsck/archive/provenance validation remains unimplemented.
pub struct BundleSink(Arc<Mutex<State>>);

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
    pub fn object_count(&self) -> u32 {
        self.objects
    }
}
impl BundleSink {
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
                    .lock()
                    .map_err(|_| io::Error::other("quarantine lock poisoned"))?;
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
            let directory = parent.join(format!("git-{}", iroh::SecretKey::generate().public()));
            let mut builder = fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(&directory)?;
            let initialized = (|| {
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(directory.join("bundle"))?;
                file.sync_all()?;
                File::open(&directory)?.sync_all()?;
                File::open(&parent)?.sync_all()?;
                Ok::<_, io::Error>(file)
            })();
            match initialized {
                Ok(file) => Ok(Self(Arc::new(Mutex::new(State {
                    directory,
                    file,
                    size: 0,
                    limit,
                    failed: false,
                })))),
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
        let bytes = bytes.to_owned();
        let state = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = state
                .lock()
                .map_err(|_| io::Error::other("quarantine lock poisoned"))?;
            if state.failed {
                return Err(io::Error::other("quarantine write previously failed"));
            }
            let end = state
                .size
                .checked_add(bytes.len() as u64)
                .filter(|n| *n <= state.limit)
                .ok_or_else(|| io::Error::other("quarantine byte limit"))?;
            // Poison on any I/O failure; a partial write is never retried as if
            // it were an all-or-nothing append.
            state.failed = true;
            state.file.write_all(&bytes)?;
            state.file.sync_data()?;
            state.size = end;
            state.failed = false;
            Ok(())
        })
        .await
        .map_err(io::Error::other)?
    }
}
