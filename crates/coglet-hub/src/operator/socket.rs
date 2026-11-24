//! AF_UNIX transport only. The caller retains the router state-directory lock.
//! OS identity is acquired on every connection, before reading any JSON.
use super::{PeerIdentity, Request};
use std::{
    fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
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

pub struct OperatorSocket {
    listener: UnixListener,
    path: PathBuf,
    device: u64,
    inode: u64,
    uid: u32,
}
impl OperatorSocket {
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
        })
    }
    pub async fn accept(&self) -> io::Result<(UnixStream, PeerIdentity)> {
        let (stream, _) = self.listener.accept().await?;
        let cred = stream.peer_cred()?;
        if cred.uid() != self.uid {
            return Err(denied());
        }
        Ok((
            stream,
            PeerIdentity {
                uid: cred.uid(),
                gid: Some(cred.gid()),
                pid: cred.pid().and_then(|p| u32::try_from(p).ok()),
            },
        ))
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
