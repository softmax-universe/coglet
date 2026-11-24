//! Raw bundle quarantine storage only; this is NOT Git validation or admission.
//! The owning directory must be private/trusted and on a durable filesystem.
use crate::transfer::DurableSink;
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
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
impl BundleSink {
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
