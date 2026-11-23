//! Linux authenticated local transport. Operational runtime integration is still
//! unavailable; the same negotiation-only state machine backs the embedded ABI.
use crate::{Result, SURFACE_MAX_FRAME, SurfaceBinding, SurfaceSession};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{DirBuilderExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
    },
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

pub struct SurfaceListener {
    listener: UnixListener,
    path: PathBuf,
    inode: u64,
    device: u64,
    peer_uid: u32,
    binding: SurfaceBinding,
}
impl SurfaceListener {
    /// The supervisor must provide a path with a private owned immediate parent,
    /// under trusted ancestors. Refuse stale sockets rather than unlinking a live
    /// listener or following a symlink. Peer UID is explicit supervisor policy.
    pub fn bind(path: &Path, peer_uid: u32, binding: SurfaceBinding) -> Result<Self> {
        if !path.is_absolute() {
            return Err("socket path must be absolute".into());
        }
        let parent = path.parent().ok_or("socket has no parent")?;
        match fs::DirBuilder::new().mode(0o700).create(parent) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(e) => return Err(e.to_string()),
        }
        let meta = fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
        // SAFETY: geteuid has no preconditions.
        if !meta.is_dir()
            || meta.uid() != unsafe { libc::geteuid() }
            || meta.mode() & 0o7777 != 0o700
        {
            return Err("socket parent must be owned directory mode 0700".into());
        }
        let listener = UnixListener::bind(path).map_err(|e| e.to_string())?;
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        let result = Self {
            listener,
            path: path.to_owned(),
            inode: meta.ino(),
            device: meta.dev(),
            peer_uid,
            binding,
        };
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(|e| e.to_string())?;
        Ok(result)
    }
    pub fn serve_one(&self) -> Result<()> {
        let (stream, _) = self.listener.accept().map_err(|e| e.to_string())?;
        serve(stream, self.peer_uid, self.binding.clone())
    }
}
impl Drop for SurfaceListener {
    fn drop(&mut self) {
        if fs::symlink_metadata(&self.path)
            .is_ok_and(|m| m.ino() == self.inode && m.dev() == self.device)
        {
            let _ = fs::remove_file(&self.path);
        }
    }
}

pub fn serve(mut stream: UnixStream, peer_uid: u32, binding: SurfaceBinding) -> Result<()> {
    // Authenticate kernel-supplied credentials, not a claimed hello identity.
    let mut cred = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: the kernel writes at most len bytes to aligned credential storage.
    let status = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            cred.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 || len as usize != std::mem::size_of::<libc::ucred>() {
        return Err("peer credential lookup failed".into());
    }
    // SAFETY: successful getsockopt returned the complete credential object.
    if unsafe { cred.assume_init() }.uid != peer_uid {
        return Err("unauthenticated peer UID".into());
    }
    stream
        .set_write_timeout(Some(Duration::from_secs(3)))
        .map_err(|e| e.to_string())?;
    let clock = Instant::now();
    let mut session = SurfaceSession::new(binding);
    loop {
        // One absolute deadline for the entire frame, not a renewed timeout for
        // each fragmented byte (a slow sender cannot hold this channel forever).
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut header = [0; 4];
        if !read_exact_until(
            &mut stream,
            &mut header,
            deadline,
            true,
            &mut session,
            clock,
        )? {
            session.disconnect();
            return Ok(());
        }
        let n = u32::from_be_bytes(header) as usize;
        if n == 0 || n > SURFACE_MAX_FRAME {
            session.disconnect();
            return Err("invalid surface frame length".into());
        }
        let mut raw = vec![0; n];
        read_exact_until(&mut stream, &mut raw, deadline, false, &mut session, clock)?;
        let now =
            u64::try_from(clock.elapsed().as_micros()).map_err(|_| "runtime clock overflow")?;
        if session.send(&raw, now) != 0 {
            return Err("fatal surface message".into());
        }
        while let Some(response) = session.pop() {
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .and_then(|_| stream.write_all(&response))
                .map_err(|e| e.to_string())?;
        }
        if session.is_closed() {
            return Ok(());
        }
    }
}
fn read_exact_until(
    stream: &mut UnixStream,
    mut out: &mut [u8],
    deadline: Instant,
    clean_eof: bool,
    session: &mut SurfaceSession,
    clock: Instant,
) -> Result<bool> {
    let mut first = true;
    while !out.is_empty() {
        let now =
            u64::try_from(clock.elapsed().as_micros()).map_err(|_| "runtime clock overflow")?;
        session.tick(now);
        if session.is_closed() {
            return Err("surface inactivity timeout".into());
        }
        while let Some(response) = session.pop() {
            stream
                .write_all(&(response.len() as u32).to_be_bytes())
                .and_then(|_| stream.write_all(&response))
                .map_err(|e| e.to_string())?;
        }
        let remaining = deadline
            .checked_duration_since(Instant::now())
            .ok_or("frame timeout")?;
        if remaining.is_zero() {
            return Err("frame timeout".into());
        }
        stream
            .set_read_timeout(Some(remaining.min(Duration::from_millis(50))))
            .map_err(|e| e.to_string())?;
        match stream.read(out) {
            Ok(0) if clean_eof && first => return Ok(false),
            Ok(0) => return Err("EOF mid-frame".into()),
            Ok(n) => {
                first = false;
                out = &mut out[n..];
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(true)
}
