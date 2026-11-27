use crate::{Snapshot, digest, unhex};
use fs2::FileExt;
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

/// Every operation before/after which a process crash can change durable state.
/// Repeated once per new blob. Hooks run *after* the named operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushPoint {
    BlobWritten,
    BlobSynced,
    BlobPublished,
    BlobsSynced,
    TreeWritten,
    TreeSynced,
    TreePublished,
    TreesSynced,
    PointerWritten,
    PointerSynced,
    PointerPublished,
    RootSynced,
}
/// An exclusive, private runtime-owned directory, not a guest-visible mount.
/// No hostile host filesystem/symlink writers are supported. Locks serialize
/// cooperating processes; expected-current CAS prevents stale snapshot commits.
pub struct DiskStore {
    root: PathBuf,
}
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}
impl DiskStore {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let root = path.as_ref().to_path_buf();
        // Parent must exist: establish durable directory entries bottom-up.
        if !root.exists() {
            fs::create_dir(&root)?;
            sync_dir(
                root.parent()
                    .filter(|p| !p.as_os_str().is_empty())
                    .unwrap_or(Path::new(".")),
            )?;
        }
        for name in ["blobs", "trees"] {
            match fs::create_dir(root.join(name)) {
                Ok(()) => (),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e),
            }
        }
        sync_dir(&root)?;
        Ok(Self { root })
    }
    fn lock(&self) -> io::Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("LOCK"))?;
        file.lock_exclusive()?;
        Ok(file)
    }
    fn current(&self) -> io::Result<Option<String>> {
        match fs::read_to_string(self.root.join("CURRENT")) {
            Ok(s) if s.len() == 64 && unhex(&s).is_some() => Ok(Some(s)),
            Ok(_) => Err(invalid("invalid CURRENT")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn recover(&self) -> io::Result<Option<Snapshot>> {
        let _lock = self.lock()?;
        let Some(hash) = self.current()? else {
            return Ok(None);
        };
        let bytes = fs::read(self.root.join("trees").join(&hash))?;
        let index: BTreeMap<String, String> = serde_json::from_slice(&bytes)?;
        // Canonical encoding also rejects duplicate keys / alternate encodings.
        if serde_json::to_vec(&index)? != bytes {
            return Err(invalid("noncanonical tree"));
        }
        let mut files = BTreeMap::new();
        for (path, blob) in index {
            if blob.len() != 64 || unhex(&blob).is_none() {
                return Err(invalid("invalid blob id"));
            }
            let data = fs::read(self.root.join("blobs").join(&blob))?;
            if digest(&data) != blob {
                return Err(invalid("blob digest mismatch"));
            }
            files.insert(path, data);
        }
        let snapshot = Snapshot::from_files(files).map_err(|_| invalid("invalid path/tree"))?;
        let new_hash = snapshot.tree_hash();
        if new_hash != hash {
            // Only the verified historical identity is eligible for migration;
            // never turn an arbitrary misnamed/corrupt tree into a valid one.
            if digest(&snapshot.cbor()) != hash {
                return Err(invalid("tree digest mismatch"));
            }
            // Reuse the durable publication sequence while retaining this lock.
            // The old tree remains valid until CURRENT is atomically replaced;
            // a crash after publishing the new tree can safely retry migration.
            self.flush_locked(&snapshot, Some(&hash), |_| Ok(()))?;
        }
        Ok(Some(snapshot))
    }
    pub fn flush(&self, snapshot: &Snapshot, expected: Option<&str>) -> io::Result<String> {
        self.flush_with(snapshot, expected, |_| Ok(()))
    }
    /// Failure after PointerPublished is an uncertain commit; recover identity,
    /// never assume rollback or rerun a guest step. Does not reclaim any WAL.
    pub fn flush_with(
        &self,
        snapshot: &Snapshot,
        expected: Option<&str>,
        hook: impl FnMut(FlushPoint) -> io::Result<()>,
    ) -> io::Result<String> {
        let _lock = self.lock()?;
        self.flush_locked(snapshot, expected, hook)
    }
    // Caller must hold LOCK; recovery must not recursively acquire an OS lock.
    fn flush_locked(
        &self,
        snapshot: &Snapshot,
        expected: Option<&str>,
        mut hook: impl FnMut(FlushPoint) -> io::Result<()>,
    ) -> io::Result<String> {
        if self.current()?.as_deref() != expected {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "checkpoint conflict",
            ));
        }
        let blobs = self.root.join("blobs");
        for blob in snapshot.files.values() {
            let target = blobs.join(&blob.hash);
            if target.exists() {
                if fs::read(&target)? != blob.bytes.as_ref() {
                    return Err(invalid("existing blob corrupt"));
                }
                continue;
            }
            let mut temp = tempfile::NamedTempFile::new_in(&blobs)?;
            temp.write_all(&blob.bytes)?;
            hook(FlushPoint::BlobWritten)?;
            temp.as_file().sync_all()?;
            hook(FlushPoint::BlobSynced)?;
            temp.persist(&target).map_err(|e| e.error)?;
            hook(FlushPoint::BlobPublished)?;
        }
        sync_dir(&blobs)?;
        hook(FlushPoint::BlobsSynced)?;
        let hash = snapshot.tree_hash();
        let index: BTreeMap<_, _> = snapshot.files.iter().map(|(p, b)| (p, &b.hash)).collect();
        let bytes = serde_json::to_vec(&index)?;
        let trees = self.root.join("trees");
        // A prior interrupted publication may already have left this object.
        // Validate it rather than concealing on-disk corruption by overwriting.
        match fs::read(trees.join(&hash)) {
            Ok(existing) if existing != bytes => return Err(invalid("existing tree corrupt")),
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let mut tree = tempfile::NamedTempFile::new_in(&trees)?;
        tree.write_all(&bytes)?;
        hook(FlushPoint::TreeWritten)?;
        tree.as_file().sync_all()?;
        hook(FlushPoint::TreeSynced)?;
        tree.persist(trees.join(&hash)).map_err(|e| e.error)?;
        hook(FlushPoint::TreePublished)?;
        sync_dir(&trees)?;
        hook(FlushPoint::TreesSynced)?;
        let mut pointer = tempfile::NamedTempFile::new_in(&self.root)?;
        pointer.write_all(hash.as_bytes())?;
        hook(FlushPoint::PointerWritten)?;
        pointer.as_file().sync_all()?;
        hook(FlushPoint::PointerSynced)?;
        pointer
            .persist(self.root.join("CURRENT"))
            .map_err(|e| e.error)?;
        hook(FlushPoint::PointerPublished)?;
        sync_dir(&self.root)?;
        hook(FlushPoint::RootSynced)?;
        Ok(hash)
    }
}
