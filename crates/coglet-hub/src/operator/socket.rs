//! AF_UNIX transport only. The caller retains the router state-directory lock.
//! OS identity is acquired on every connection, before reading any JSON.
use super::{PeerIdentity, Request};
use std::{
    fs, io,
    os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
};

const MAX_FRAME: usize = 1_048_576;
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "local operator identity required",
    )
}

fn verified_peer(
    credential: io::Result<PeerIdentity>,
    service_uid: u32,
) -> io::Result<PeerIdentity> {
    let peer = credential?;
    if peer.uid != service_uid {
        return Err(denied());
    }
    Ok(peer)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operator_fail_closed_creds() {
        assert!(verified_peer(Err(io::Error::other("credentials unavailable")), 1000).is_err());
        for uid in [0, 1001] {
            assert_eq!(
                verified_peer(
                    Ok(PeerIdentity {
                        uid,
                        gid: None,
                        pid: None
                    }),
                    1000
                )
                .unwrap_err()
                .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        assert_eq!(
            verified_peer(
                Ok(PeerIdentity {
                    uid: 1000,
                    gid: Some(1000),
                    pid: Some(1)
                }),
                1000
            )
            .unwrap()
            .uid,
            1000
        );
    }
}

/// Exclusive process ownership of a private router state directory. The socket
/// retains this guard, so dropping a caller's copy cannot release ownership early.
#[derive(Clone)]
pub struct StateLock(Arc<LockedState>);
struct LockedState {
    _file: fs::File,
    root: PathBuf,
    bound: AtomicBool,
}
impl StateLock {
    pub fn acquire(root: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(root)?;
        // SAFETY: geteuid has no arguments and cannot invalidate Rust memory.
        let uid = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o777 != 0o700 {
            return Err(denied());
        }
        let path = root.join("hub.lock");
        let file = fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(&path)?;
        let meta = file.metadata()?;
        if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o777 != 0o600 || meta.nlink() != 1
        {
            return Err(denied());
        }
        file.try_lock()?;
        let entry = fs::symlink_metadata(&path)?;
        if entry.dev() != meta.dev() || entry.ino() != meta.ino() {
            return Err(denied());
        }
        Ok(Self(Arc::new(LockedState {
            _file: file,
            root: root.to_path_buf(),
            bound: AtomicBool::new(false),
        })))
    }
}
struct BindLease(StateLock);
impl Drop for BindLease {
    fn drop(&mut self) {
        self.0.0.bound.store(false, Ordering::Release);
    }
}

pub struct OperatorSocket {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    uid: u32,
    _lease: Option<BindLease>,
}
impl OperatorSocket {
    /// Recover only a service-owned, refused socket while holding exclusive
    /// state ownership. Ambiguous probes, live listeners and arbitrary entries
    /// are never replaced. The binding lease also serializes same-process calls.
    pub async fn bind_locked(lock: &StateLock) -> io::Result<Self> {
        if lock
            .0
            .bound
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "operator socket already bound",
            ));
        }
        let lease = BindLease(lock.clone());
        let path = lock.0.root.join("operator.sock");
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                // SAFETY: geteuid has no arguments and cannot invalidate Rust memory.
                let uid = unsafe { libc::geteuid() };
                if !meta.file_type().is_socket()
                    || meta.uid() != uid
                    || meta.mode() & 0o777 != 0o600
                {
                    return Err(denied());
                }
                match tokio::time::timeout(Duration::from_millis(250), UnixStream::connect(&path))
                    .await
                {
                    Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => (),
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::AlreadyExists,
                            "operator socket is live or ambiguous",
                        ));
                    }
                }
                let current = fs::symlink_metadata(&path)?;
                if current.dev() != meta.dev() || current.ino() != meta.ino() {
                    return Err(denied());
                }
                fs::remove_file(&path)?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let mut socket = Self::bind(&lock.0.root)?;
        socket._lease = Some(lease);
        Ok(socket)
    }
    /// Require an existing private, service-owned directory; do not chmod an
    /// arbitrary caller path, follow a symlink or replace an existing entry.
    pub fn bind(state: &Path) -> io::Result<Self> {
        let meta = fs::symlink_metadata(state)?;
        // SAFETY: geteuid has no arguments and cannot invalidate Rust memory.
        let uid = unsafe { libc::geteuid() };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o777 != 0o700 {
            return Err(denied());
        }
        let path = state.join("operator.sock");
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "operator socket entry already exists",
                ));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let meta = fs::symlink_metadata(&path)?;
        Ok(Self {
            listener,
            path,
            device: meta.dev(),
            inode: meta.ino(),
            uid,
            _lease: None,
        })
    }
    pub async fn accept(&self) -> io::Result<(UnixStream, PeerIdentity)> {
        let (stream, _) = self.listener.accept().await?;
        let peer = verified_peer(
            stream.peer_cred().map(|cred| PeerIdentity {
                uid: cred.uid(),
                gid: Some(cred.gid()),
                pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
            }),
            self.uid,
        )?;
        Ok((stream, peer))
    }
}
impl Drop for OperatorSocket {
    fn drop(&mut self) {
        // Never unlink a replacement, even if an administrative peer moved it.
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.dev() == self.device && m.ino() == self.inode)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}
/// One bounded request. Timeout is independent of the client-supplied wait
/// budget, so an idle/malformed client cannot retain a parser slot indefinitely.
pub async fn read_request(stream: &mut UnixStream) -> io::Result<Request> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let length = stream.read_u32().await? as usize;
        if length == 0 || length > MAX_FRAME {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid operator frame length",
            ));
        }
        let mut bytes = vec![0; length];
        stream.read_exact(&mut bytes).await?;
        // Parsing must be called from a driver with bounded connection capacity.
        Request::parse(&bytes)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid operator request"))
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "operator frame timeout"))?
}
pub async fn write_response(
    stream: &mut UnixStream,
    response: &serde_json::Value,
) -> io::Result<()> {
    let bytes = serde_json::to_vec(response)?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid operator response length",
        ));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        stream.shutdown().await
    })
    .await
    .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "operator reply timeout"))?
}
