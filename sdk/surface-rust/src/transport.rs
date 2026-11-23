//! Surface control transports. Wire validation and authenticated bindings live in `ControlPeer`.

use std::io::{self, Read, Write};

pub const MAX_FRAME: usize = 1_048_576;
pub const QUEUE_BOUND: usize = 128;

/// Core's authenticated surface state machine. Bodies are deterministic CBOR,
/// never stream prefixes. `receive` must validate profile/schema/direction and
/// lifecycle before effects; Ok signifies a valid peer message. `tick` must emit
/// health at least once per logical second. All clocks are monotonic microseconds.
/// An Err from any method permits the driver to close; disconnect must stop native
/// I/O, clear authority and reconcile pending work even after prior peer errors.
/// Binding surface/coglet keys is a supervisor/core configuration responsibility.
pub trait ControlPeer {
    fn start(&mut self) -> Result<Vec<Vec<u8>>, String>;
    fn receive(&mut self, bytes: &[u8], now: u64) -> Result<Vec<Vec<u8>>, String>;
    fn tick(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String>;
    fn disconnect(&mut self, now: u64) -> Result<(), String>;
}

fn valid_size(n: usize) -> io::Result<()> {
    if n == 0 || n > MAX_FRAME {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid control frame size",
        ))
    } else {
        Ok(())
    }
}

/// Reads one frame; clean EOF is None, partial EOF is UnexpectedEof.
/// For timeout-driven streams use FrameDecoder so partial reads survive timeouts.
pub fn read_frame<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let n = u32::from_be_bytes(header) as usize;
    valid_size(n)?;
    let mut body = vec![0; n];
    reader.read_exact(&mut body)?;
    Ok(Some(body))
}

pub fn write_frame<W: Write>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    valid_size(body.len())?;
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(body)
}

/// Incremental parser with at most one bounded partial payload and 128 results.
/// Any error poisons the parser: the link must close, never resynchronize.
#[derive(Default)]
pub struct FrameDecoder {
    header: [u8; 4],
    used: usize,
    expected: usize,
    payload: Vec<u8>,
    failed: bool,
}
impl FrameDecoder {
    pub fn push(&mut self, mut input: &[u8]) -> io::Result<Vec<Vec<u8>>> {
        if self.failed {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "decoder closed"));
        }
        let mut frames = Vec::new();
        while !input.is_empty() {
            if self.used < 4 {
                let count = (4 - self.used).min(input.len());
                self.header[self.used..self.used + count].copy_from_slice(&input[..count]);
                self.used += count;
                input = &input[count..];
                if self.used != 4 {
                    continue;
                }
                self.expected = u32::from_be_bytes(self.header) as usize;
                if let Err(e) = valid_size(self.expected) {
                    self.failed = true;
                    return Err(e);
                }
                self.payload = Vec::with_capacity(self.expected);
            }
            let count = (self.expected - self.payload.len()).min(input.len());
            self.payload.extend_from_slice(&input[..count]);
            input = &input[count..];
            if self.payload.len() == self.expected {
                if frames.len() == QUEUE_BOUND {
                    self.failed = true;
                    return Err(io::Error::new(
                        io::ErrorKind::OutOfMemory,
                        "resource_exhausted: control queue",
                    ));
                }
                frames.push(std::mem::take(&mut self.payload));
                self.used = 0;
                self.expected = 0;
            }
        }
        Ok(frames)
    }
    pub fn finish(&mut self) -> io::Result<()> {
        if self.failed || self.used != 0 {
            self.failed = true;
            Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "EOF inside control frame",
            ))
        } else {
            Ok(())
        }
    }
}

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

struct PairState<P: ControlPeer> {
    peer: P,
    incoming: VecDeque<Vec<u8>>,
    outgoing: VecDeque<Vec<u8>>,
    closed: bool,
    now: u64,
    last_valid: u64,
}
impl<P: ControlPeer> PairState<P> {
    fn close(&mut self) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.incoming.clear();
        self.outgoing.clear();
        self.peer.disconnect(self.now)
    }
    fn enqueue(&mut self, frames: Vec<Vec<u8>>) -> Result<(), String> {
        if frames.len() > QUEUE_BOUND.saturating_sub(self.outgoing.len()) {
            return Err("resource_exhausted: control queue".into());
        }
        for frame in &frames {
            valid_size(frame.len()).map_err(|e| e.to_string())?;
        }
        self.outgoing.extend(frames);
        Ok(())
    }
}

/// Surface half: all received bodies go through the real configured core peer.
/// No parsed-message shortcut or simulated grant/dispatch path exists here.
pub struct InProcess<P: ControlPeer>(Rc<RefCell<PairState<P>>>);
/// Runtime half of a caller-serialized paired queue; frames have no stream prefix.
pub struct RuntimeQueue<P: ControlPeer>(Rc<RefCell<PairState<P>>>);

pub fn in_process_pair<P: ControlPeer>(
    mut peer: P,
    now: u64,
) -> Result<(InProcess<P>, RuntimeQueue<P>), String> {
    let initial = match peer.start() {
        Ok(frames) => frames,
        Err(e) => {
            let _ = peer.disconnect(now);
            return Err(e);
        }
    };
    let mut state = PairState {
        peer,
        incoming: VecDeque::new(),
        outgoing: VecDeque::new(),
        closed: false,
        now,
        last_valid: now,
    };
    if let Err(e) = state.enqueue(initial) {
        let _ = state.close();
        return Err(e);
    }
    let shared = Rc::new(RefCell::new(state));
    Ok((InProcess(shared.clone()), RuntimeQueue(shared)))
}
impl<P: ControlPeer> InProcess<P> {
    /// `now` is the surface's monotonic logical microsecond clock. Core tick owns
    /// periodic health serialization; this driver additionally enforces idle loss.
    pub fn poll(&mut self, now: u64) -> Result<(), String> {
        let mut state = self.0.borrow_mut();
        if state.closed {
            return Err("closed".into());
        }
        let result = (|| {
            if now < state.now {
                return Err("clock rollback".into());
            }
            state.now = now;
            if now - state.last_valid >= 3_000_000 {
                return Err("health timeout".into());
            }
            while let Some(body) = state.incoming.pop_front() {
                let frames = state.peer.receive(&body, now)?;
                state.last_valid = now;
                state.enqueue(frames)?;
            }
            let frames = state.peer.tick(now)?;
            state.enqueue(frames)
        })();
        if result.is_err() {
            let _ = state.close();
        }
        result
    }
    pub fn disconnect(&mut self, now: u64) -> Result<(), String> {
        let mut state = self.0.borrow_mut();
        state.now = state.now.max(now);
        state.close()
    }
}
impl<P: ControlPeer> RuntimeQueue<P> {
    /// Copies raw deterministic CBOR, never retaining the caller's buffer.
    /// Invalid profile/schema/lifecycle bodies are rejected by core on poll.
    pub fn send(&mut self, body: &[u8]) -> Result<(), String> {
        let mut state = self.0.borrow_mut();
        if state.closed {
            return Err("closed".into());
        }
        if let Err(e) = valid_size(body.len()) {
            let _ = state.close();
            return Err(e.to_string());
        }
        if state.incoming.len() == QUEUE_BOUND {
            let _ = state.close();
            return Err("resource_exhausted: control queue".into());
        }
        state.incoming.push_back(body.to_vec());
        Ok(())
    }
    pub fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        let mut state = self.0.borrow_mut();
        if state.closed {
            return Err("closed".into());
        }
        Ok(state.outgoing.pop_front())
    }
}
impl<P: ControlPeer> Drop for InProcess<P> {
    fn drop(&mut self) {
        let _ = self.0.borrow_mut().close();
    }
}
impl<P: ControlPeer> Drop for RuntimeQueue<P> {
    fn drop(&mut self) {
        let _ = self.0.borrow_mut().close();
    }
}

#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::path::Path;

/// Authenticated surface-side client. Owns its peer so EOF, fatal errors and Drop
/// always apply core disconnect. Call poll frequently (<=1s); tick serializes health.
/// `now` values are monotonic microseconds, not wall-clock timestamps.
#[cfg(unix)]
pub struct SocketConnection<P: ControlPeer> {
    stream: UnixStream,
    peer: P,
    decoder: FrameDecoder,
    now: u64,
    last_valid: u64,
    closed: bool,
}

#[cfg(unix)]
fn private_socket(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
    let sock = std::fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    // SAFETY: geteuid has no pointer arguments or preconditions.
    let uid = unsafe { libc::geteuid() };
    if !dir.is_dir() || dir.uid() != uid || dir.mode() & 0o7777 != 0o700 {
        return Err("unauthenticated: socket parent must be owned directory 0700".into());
    }
    if !sock.file_type().is_socket() || sock.uid() != uid || sock.mode() & 0o7777 != 0o600 {
        return Err("unauthenticated: socket must be owned socket 0600".into());
    }
    Ok(())
}

#[cfg(unix)]
fn runtime_uid(stream: &UnixStream) -> io::Result<u32> {
    use std::os::fd::AsRawFd;
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        // SAFETY: output points to correctly sized writable ucred storage; len is
        // initialized and live; getsockopt initializes storage on success.
        let rc = unsafe {
            libc::getsockopt(
                stream.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                credentials.as_mut_ptr().cast(),
                &mut len,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        if len as usize != std::mem::size_of::<libc::ucred>() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad peer credential size",
            ));
        }
        // SAFETY: successful getsockopt with exact ucred length initialized it.
        Ok(unsafe { credentials.assume_init() }.uid)
    }
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        let mut uid = 0;
        let mut gid = 0;
        // SAFETY: both output pointers reference live uid_t/gid_t storage.
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(uid)
    }
    #[cfg(not(any(
        target_os = "linux",
        target_os = "android",
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    )))]
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "peer credentials unavailable",
        ))
    }
}

#[cfg(unix)]
impl<P: ControlPeer> SocketConnection<P> {
    /// The supervisor-configured runtime UID is not inferred from hello. Core
    /// configuration separately authenticates surface/coglet key bindings.
    pub fn connect(
        path: &Path,
        configured_runtime_uid: u32,
        mut peer: P,
        now: u64,
    ) -> Result<Self, String> {
        let result = (|| {
            private_socket(path)?;
            let stream = UnixStream::connect(path).map_err(|e| e.to_string())?;
            if runtime_uid(&stream).map_err(|e| e.to_string())? != configured_runtime_uid {
                return Err("unauthenticated: runtime UID mismatch".into());
            }
            // Recheck filesystem permissions after connect as defense in depth.
            // Same-UID hostile processes are explicitly outside this boundary.
            private_socket(path)?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_millis(50)))
                .map_err(|e| e.to_string())?;
            stream
                .set_write_timeout(Some(std::time::Duration::from_millis(100)))
                .map_err(|e| e.to_string())?;
            Ok(stream)
        })();
        let stream = match result {
            Ok(stream) => stream,
            Err(e) => {
                let _ = peer.disconnect(now);
                return Err(e);
            }
        };
        let mut link = Self {
            stream,
            peer,
            decoder: FrameDecoder::default(),
            now,
            last_valid: now,
            closed: false,
        };
        let result = link
            .peer
            .start()
            .and_then(|frames| link.write_batch(frames));
        if let Err(e) = result {
            let _ = link.disconnect(now);
            return Err(e);
        }
        Ok(link)
    }
    fn write_batch(&mut self, frames: Vec<Vec<u8>>) -> Result<(), String> {
        if frames.len() > QUEUE_BOUND {
            return Err("resource_exhausted: control queue".into());
        }
        // Check entire batch before writing any of it.
        for frame in &frames {
            valid_size(frame.len()).map_err(|e| e.to_string())?;
        }
        for frame in frames {
            write_frame(&mut self.stream, &frame).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    pub fn poll(&mut self, now: u64) -> Result<(), String> {
        if self.closed {
            return Err("closed".into());
        }
        let result = (|| {
            if now < self.now {
                return Err("clock rollback".into());
            }
            self.now = now;
            if now - self.last_valid >= 3_000_000 {
                return Err("health timeout".into());
            }
            let mut buffer = [0; 8192];
            match self.stream.read(&mut buffer) {
                Ok(0) => {
                    self.decoder.finish().map_err(|e| e.to_string())?;
                    return Err("closed: EOF".into());
                }
                Ok(n) => {
                    for body in self.decoder.push(&buffer[..n]).map_err(|e| e.to_string())? {
                        let frames = self.peer.receive(&body, now)?;
                        self.last_valid = now;
                        self.write_batch(frames)?;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
            let frames = self.peer.tick(now)?;
            self.write_batch(frames)
        })();
        if result.is_err() {
            let _ = self.disconnect(now);
        }
        result
    }
    pub fn disconnect(&mut self, now: u64) -> Result<(), String> {
        if self.closed {
            return Ok(());
        }
        self.closed = true;
        self.now = self.now.max(now);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        self.peer.disconnect(self.now)
    }
    /// Blocking std-IO service loop, ending on stop, EOF or fatal peer error.
    /// The stop callback must be fast; physical off-switches are independent.
    pub fn serve<F: FnMut() -> bool>(&mut self, mut stop: F) -> Result<(), String> {
        let epoch = self.now;
        let started = std::time::Instant::now();
        while !stop() {
            let elapsed = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            let now = epoch.saturating_add(elapsed);
            self.poll(now)?;
        }
        self.disconnect(self.now)
    }
}
#[cfg(unix)]
impl<P: ControlPeer> Drop for SocketConnection<P> {
    fn drop(&mut self) {
        let _ = self.disconnect(self.now);
    }
}

/// Function pointers for the runtime library's surface ABI v1. This is a caller
/// adapter only: it exports no runtime symbols and confers no authenticated identity.
#[derive(Clone, Copy)]
pub struct NativeApi {
    pub open: unsafe extern "C" fn(u32, *mut *mut std::ffi::c_void) -> i32,
    pub send: unsafe extern "C" fn(*mut std::ffi::c_void, *const u8, usize) -> i32,
    pub recv: unsafe extern "C" fn(*mut std::ffi::c_void, *mut u8, usize, *mut usize) -> i32,
    pub close: unsafe extern "C" fn(*mut std::ffi::c_void),
}

#[derive(Debug, PartialEq, Eq)]
pub enum NativeReceive {
    Data(usize),
    WouldBlock,
    Need(usize),
}

/// Exclusive caller-serialized handle, intentionally !Send and !Sync. No borrowed
/// input/output pointer survives a callback. Never use from an audio callback.
/// Trusted callbacks MUST obey the documented ABI: checks cannot prevent a native
/// callback from writing outside its output buffer or retaining forbidden pointers.
pub struct NativeLink {
    api: NativeApi,
    handle: Option<std::ptr::NonNull<std::ffi::c_void>>,
    _serialized: std::marker::PhantomData<Rc<()>>,
    pending_size: Option<usize>,
}
impl NativeLink {
    /// # Safety
    /// Callbacks must implement the specified ABI, stay loaded until close, never
    /// unwind, retain buffer pointers, reenter this link, or alias its handle.
    /// The runtime must already have its supervisor-authenticated bindings installed.
    pub unsafe fn open(api: NativeApi, version: u32) -> Result<Self, String> {
        let mut raw = std::ptr::null_mut();
        // SAFETY: caller warrants callback contract; raw is a live writable slot.
        let rc = unsafe { (api.open)(version, &mut raw) };
        if rc != 0 {
            // A conforming failure creates no handle. A broken callback returning
            // a live handle is closed defensively, without exposing it to callers.
            if !raw.is_null() {
                unsafe {
                    (api.close)(raw);
                }
            }
            return Err(match rc {
                -1 => "invalid argument/unsupported ABI",
                -2 => "closed/fatal protocol error",
                _ => "invalid open return code",
            }
            .into());
        }
        let handle = std::ptr::NonNull::new(raw).ok_or("ABI contract: open returned null")?;
        if version != 1 {
            // SAFETY: successful open transfers a live owned handle to caller.
            unsafe {
                (api.close)(handle.as_ptr());
            }
            return Err("ABI contract: unsupported ABI accepted".into());
        }
        Ok(Self {
            api,
            handle: Some(handle),
            _serialized: std::marker::PhantomData,
            pending_size: None,
        })
    }
    fn raw(&self) -> Result<*mut std::ffi::c_void, String> {
        self.handle
            .map(|h| h.as_ptr())
            .ok_or_else(|| "closed".into())
    }
    fn fail<T>(&mut self, error: &str) -> Result<T, String> {
        self.close();
        Err(error.into())
    }
    /// true means copied, false means would-block and copied nothing. On false
    /// retry identical bytes; success is not an action accept/terminal outcome.
    pub fn send(&mut self, cbor: &[u8]) -> Result<bool, String> {
        valid_size(cbor.len()).map_err(|e| e.to_string())?;
        let raw = self.raw()?;
        // SAFETY: exclusive live handle and valid borrowed slice for this call only.
        match unsafe { (self.api.send)(raw, cbor.as_ptr(), cbor.len()) } {
            0 => Ok(true),
            1 => Ok(false),
            -1 => Err("invalid argument".into()),
            -2 => self.fail("closed/fatal protocol error"),
            _ => self.fail("ABI contract: invalid send return code"),
        }
    }
    /// Null/zero size query. Repeated queries and short-buffer attempts must keep
    /// returning the same required size until the item is successfully consumed.
    pub fn query(&mut self) -> Result<Option<usize>, String> {
        match self.recv_into(&mut [])? {
            NativeReceive::WouldBlock => Ok(None),
            NativeReceive::Need(n) => Ok(Some(n)),
            NativeReceive::Data(_) => self.fail("ABI contract: size query dequeued data"),
        }
    }
    pub fn recv_into(&mut self, output: &mut [u8]) -> Result<NativeReceive, String> {
        let raw = self.raw()?;
        let mut n = usize::MAX;
        let out = if output.is_empty() {
            std::ptr::null_mut()
        } else {
            output.as_mut_ptr()
        };
        // SAFETY: exclusive live handle, initialized output slot, writable slice
        // or the ABI-permitted null/zero query. Callback retains no pointers.
        let rc = unsafe { (self.api.recv)(raw, out, output.len(), &mut n) };
        match rc {
            0 => {
                if valid_size(n).is_err()
                    || n > output.len()
                    || self.pending_size.is_some_and(|size| size != n)
                {
                    return self.fail("ABI contract: invalid recv size");
                }
                self.pending_size = None;
                Ok(NativeReceive::Data(n))
            }
            1 => {
                if n != 0 || self.pending_size.is_some() {
                    return self.fail(
                        "ABI contract: would-block n != 0 or item dequeued by query/short buffer",
                    );
                }
                Ok(NativeReceive::WouldBlock)
            }
            2 => {
                if valid_size(n).is_err()
                    || n <= output.len()
                    || self.pending_size.is_some_and(|size| size != n)
                {
                    return self.fail("ABI contract: invalid required size/nondequeue violation");
                }
                self.pending_size = Some(n);
                Ok(NativeReceive::Need(n))
            }
            -1 => Err("invalid argument".into()),
            -2 => self.fail("closed/fatal protocol error"),
            _ => self.fail("ABI contract: invalid recv return code"),
        }
    }
    pub fn recv(&mut self) -> Result<Option<Vec<u8>>, String> {
        let Some(n) = self.query()? else {
            return Ok(None);
        };
        let mut body = vec![0; n];
        match self.recv_into(&mut body)? {
            NativeReceive::Data(size) if size == n => Ok(Some(body)),
            _ => self.fail("ABI contract: query/exact-buffer inconsistency"),
        }
    }
    /// Applies native disconnect and frees handle once, including pending work.
    pub fn close(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: uniquely owned handle is consumed once; no calls after close.
            unsafe {
                (self.api.close)(handle.as_ptr());
            }
        }
        self.pending_size = None;
    }
}
impl Drop for NativeLink {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[derive(Default)]
    struct Probe {
        calls: usize,
        stopped: bool,
    }
    impl ControlPeer for Probe {
        fn start(&mut self) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![vec![0xa0]])
        }
        fn receive(&mut self, bytes: &[u8], _: u64) -> Result<Vec<Vec<u8>>, String> {
            if bytes != [0xa0] {
                return Err("invalid CBOR/profile".into());
            }
            self.calls += 1;
            Ok(vec![bytes.to_vec()])
        }
        fn tick(&mut self, _: u64) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![])
        }
        fn disconnect(&mut self, _: u64) -> Result<(), String> {
            self.stopped = true;
            Ok(())
        }
    }
    #[test]
    fn paired_bytes_are_validated_and_bounded() {
        let (mut surface, mut runtime) = in_process_pair(Probe::default(), 0).unwrap();
        assert_eq!(runtime.recv().unwrap(), Some(vec![0xa0]));
        runtime.send(&[0xa0]).unwrap();
        surface.poll(1).unwrap();
        assert_eq!(runtime.recv().unwrap(), Some(vec![0xa0]));
        runtime.send(&[0xff]).unwrap();
        assert!(surface.poll(2).is_err());
        assert!(runtime.send(&[0xa0]).is_err());
        let (_surface, mut runtime) = in_process_pair(Probe::default(), 0).unwrap();
        for _ in 0..QUEUE_BOUND {
            runtime.send(&[0xa0]).unwrap();
        }
        assert!(
            runtime
                .send(&[0xa0])
                .unwrap_err()
                .contains("resource_exhausted")
        );
    }
    #[cfg(unix)]
    #[test]
    fn unix_authenticated_roundtrip_and_timeout() {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;
        let root = std::env::var_os("TMPDIR")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let dir = root.join(format!(
            "surface-transport-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("control.sock");
        let listener = UnixListener::bind(&path).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // SAFETY: geteuid has no pointer arguments or preconditions.
        let uid = unsafe { libc::geteuid() };
        assert!(
            SocketConnection::connect(&path, uid.wrapping_add(1), Probe::default(), 0).is_err()
        );
        let (rejected, _) = listener.accept().unwrap();
        drop(rejected);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert!(SocketConnection::connect(&path, uid, Probe::default(), 0).is_err());
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut surface = SocketConnection::connect(&path, uid, Probe::default(), 0).unwrap();
        let (mut runtime, _) = listener.accept().unwrap();
        assert_eq!(read_frame(&mut runtime).unwrap(), Some(vec![0xa0]));
        runtime.write_all(&[0, 0]).unwrap();
        surface.poll(1).unwrap();
        runtime.write_all(&[0, 1, 0xa0]).unwrap();
        surface.poll(2).unwrap();
        assert_eq!(read_frame(&mut runtime).unwrap(), Some(vec![0xa0]));
        assert!(
            surface
                .poll(3_000_002)
                .unwrap_err()
                .contains("health timeout")
        );
        assert!(surface.peer.stopped);
        drop(surface);
        drop(runtime);
        drop(listener);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn unix_partial_eof_and_service_stop_disconnect() {
        let (stream, mut runtime) = UnixStream::pair().unwrap();
        let mut surface = SocketConnection {
            stream,
            peer: Probe::default(),
            decoder: FrameDecoder::default(),
            now: 0,
            last_valid: 0,
            closed: false,
        };
        runtime.write_all(&[0, 0, 0, 2, 0xa0]).unwrap();
        runtime.shutdown(std::net::Shutdown::Write).unwrap();
        surface.poll(1).unwrap();
        assert!(surface.poll(2).unwrap_err().contains("EOF inside"));
        assert!(surface.peer.stopped);
        let (stream, _runtime) = UnixStream::pair().unwrap();
        let mut surface = SocketConnection {
            stream,
            peer: Probe::default(),
            decoder: FrameDecoder::default(),
            now: 0,
            last_valid: 0,
            closed: false,
        };
        surface.serve(|| true).unwrap();
        assert!(surface.peer.stopped);
    }
    thread_local! {
        static NATIVE_CLOSE: RefCell<Option<(usize, usize)>> = const { RefCell::new(None) };
    }
    #[derive(Default)]
    struct FakeNative {
        queue: VecDeque<Vec<u8>>,
        blocked: bool,
        effects: usize,
    }
    // These callbacks are adapter-owned ABI probes, NOT a runtime ABI implementation.
    unsafe extern "C" fn fake_open(version: u32, out: *mut *mut std::ffi::c_void) -> i32 {
        if version != 1 {
            return -1;
        }
        // SAFETY: adapter supplies a valid output slot. The box is owned until close.
        unsafe {
            *out = Box::into_raw(Box::new(FakeNative::default())).cast();
        }
        0
    }
    unsafe extern "C" fn fake_send(
        handle: *mut std::ffi::c_void,
        bytes: *const u8,
        n: usize,
    ) -> i32 {
        // SAFETY: test adapter holds a live exclusive handle and valid borrowed input.
        let state = unsafe { &mut *handle.cast::<FakeNative>() };
        if state.blocked {
            return 1;
        }
        // SAFETY: adapter passes a valid slice of n bytes for this callback only.
        let input = unsafe { std::slice::from_raw_parts(bytes, n) };
        if input != [0xa0] {
            return -2;
        }
        state.effects += 1;
        state.queue.push_back(input.to_vec());
        0
    }
    unsafe extern "C" fn fake_recv(
        handle: *mut std::ffi::c_void,
        out: *mut u8,
        cap: usize,
        n: *mut usize,
    ) -> i32 {
        // SAFETY: test adapter supplies its live handle and valid n output slot.
        let state = unsafe { &mut *handle.cast::<FakeNative>() };
        let Some(front) = state.queue.front() else {
            // SAFETY: n is the adapter's live output slot.
            unsafe {
                *n = 0;
            }
            return 1;
        };
        // SAFETY: n is the adapter's live output slot.
        unsafe {
            *n = front.len();
        }
        if cap < front.len() {
            return 2;
        }
        // SAFETY: cap >= front.len(), adapter supplies writable cap-sized storage.
        unsafe {
            std::ptr::copy_nonoverlapping(front.as_ptr(), out, front.len());
        }
        state.queue.pop_front();
        0
    }
    unsafe extern "C" fn fake_close(handle: *mut std::ffi::c_void) {
        // SAFETY: exactly one close owns this allocation, with no borrowed buffers.
        let state = unsafe { Box::from_raw(handle.cast::<FakeNative>()) };
        NATIVE_CLOSE.with(|last| *last.borrow_mut() = Some((state.effects, state.queue.len())));
        drop(state);
    }
    fn fake_api() -> NativeApi {
        NativeApi {
            open: fake_open,
            send: fake_send,
            recv: fake_recv,
            close: fake_close,
        }
    }
    #[test]
    fn native_adapter_buffer_and_return_code_probes() {
        // SAFETY: all fake callbacks obey ABI ownership and lifetime requirements.
        assert!(unsafe { NativeLink::open(fake_api(), 2) }.is_err());
        // SAFETY: callbacks and handle remain valid until this link closes.
        let mut link = unsafe { NativeLink::open(fake_api(), 1) }.unwrap();
        assert_eq!(link.query().unwrap(), None);
        // SAFETY: adapter-owned fixture exposes live exclusive handle for probe setup.
        unsafe {
            (*link.handle.unwrap().as_ptr().cast::<FakeNative>()).blocked = true;
        }
        assert!(!link.send(&[0xa0]).unwrap());
        assert_eq!(link.query().unwrap(), None);
        // SAFETY: same live fixture handle, no outstanding references.
        unsafe {
            (*link.handle.unwrap().as_ptr().cast::<FakeNative>()).blocked = false;
        }
        assert!(link.send(&[0xa0]).unwrap());
        assert_eq!(link.query().unwrap(), Some(1));
        assert_eq!(link.recv_into(&mut []).unwrap(), NativeReceive::Need(1));
        assert_eq!(link.query().unwrap(), Some(1));
        let mut exact = [0];
        assert_eq!(link.recv_into(&mut exact).unwrap(), NativeReceive::Data(1));
        assert_eq!(exact, [0xa0]);
        assert_eq!(
            link.recv_into(&mut exact).unwrap(),
            NativeReceive::WouldBlock
        );
        assert!(link.send(&[0xff]).is_err());
        assert!(link.send(&[0xa0]).is_err());
        // Only the valid send had an effect; malformed/would-block sends did not.
        NATIVE_CLOSE.with(|last| assert_eq!(*last.borrow(), Some((1, 0))));
    }
    #[test]
    fn native_nonempty_short_buffer_and_close_pending_probe() {
        // SAFETY: adapter-owned callbacks obey the specified ABI.
        let mut link = unsafe { NativeLink::open(fake_api(), 1) }.unwrap();
        // SAFETY: exclusively owned fixture, no calls or references outstanding.
        unsafe {
            (*link.handle.unwrap().as_ptr().cast::<FakeNative>())
                .queue
                .push_back(vec![0x82, 0, 1]);
        }
        let mut short = [0xff; 2];
        assert_eq!(link.query().unwrap(), Some(3));
        assert_eq!(link.recv_into(&mut short).unwrap(), NativeReceive::Need(3));
        assert_eq!(short, [0xff; 2]);
        assert_eq!(link.recv().unwrap(), Some(vec![0x82, 0, 1]));
        assert_eq!(link.recv().unwrap(), None);
        assert!(link.send(&[0xa0]).unwrap());
        link.close();
        link.close();
        NATIVE_CLOSE.with(|last| assert_eq!(*last.borrow(), Some((1, 1))));
        assert!(link.query().is_err());
    }
    #[test]
    fn frame_limits_eof_coalescing_and_poison() {
        for n in [0u32, MAX_FRAME as u32 + 1, u32::MAX] {
            let mut decoder = FrameDecoder::default();
            assert!(decoder.push(&n.to_be_bytes()).is_err());
            assert_eq!(decoder.payload.capacity(), 0);
            assert!(decoder.push(&[0, 0, 0, 1, 0xa0]).is_err());
            assert!(read_frame(&mut n.to_be_bytes().as_slice()).is_err());
        }
        let body = vec![0; MAX_FRAME];
        let mut wire = Vec::new();
        write_frame(&mut wire, &body).unwrap();
        assert_eq!(read_frame(&mut wire.as_slice()).unwrap(), Some(body));
        assert!(write_frame(&mut wire, &[]).is_err());
        assert!(write_frame(&mut wire, &vec![0; MAX_FRAME + 1]).is_err());
        assert_eq!(read_frame(&mut [].as_slice()).unwrap(), None);
        for partial in [&[0u8][..], &[0, 0, 0, 2, 0xa0][..]] {
            assert!(read_frame(&mut &*partial).is_err());
            let mut decoder = FrameDecoder::default();
            decoder.push(partial).unwrap();
            assert!(decoder.finish().is_err());
        }
        let mut decoder = FrameDecoder::default();
        let bytes = [0, 0, 0, 1, 0xa0].repeat(128);
        assert_eq!(decoder.push(&bytes).unwrap().len(), 128);
        assert!(decoder.push(&[0, 0, 0, 1, 0xa0].repeat(129)).is_err());
    }
    #[test]
    fn pair_health_drop_and_outgoing_overflow_stop_core() {
        let (mut surface, mut runtime) = in_process_pair(Probe::default(), 0).unwrap();
        runtime.send(&[0xa0]).unwrap();
        assert!(surface.poll(3_000_000).is_err());
        assert!(surface.0.borrow().peer.stopped);
        assert!(runtime.recv().is_err());
        let (surface, runtime) = in_process_pair(Probe::default(), 0).unwrap();
        drop(runtime);
        assert!(surface.0.borrow().peer.stopped);
        let (mut surface, mut runtime) = in_process_pair(Probe::default(), 0).unwrap();
        for _ in 0..128 {
            runtime.send(&[0xa0]).unwrap();
        }
        assert!(surface.poll(1).unwrap_err().contains("resource_exhausted"));
        assert!(surface.0.borrow().peer.stopped);
    }
    unsafe extern "C" fn bad_recv_n(
        _: *mut std::ffi::c_void,
        _: *mut u8,
        _: usize,
        n: *mut usize,
    ) -> i32 {
        // SAFETY: adapter supplies a writable size slot; deliberately wrong value.
        unsafe {
            *n = 7;
        }
        1
    }
    unsafe extern "C" fn bad_query_dequeues(
        handle: *mut std::ffi::c_void,
        out: *mut u8,
        cap: usize,
        n: *mut usize,
    ) -> i32 {
        // SAFETY: forwarding the caller's valid pointers unchanged to fixture.
        let rc = unsafe { fake_recv(handle, out, cap, n) };
        if rc == 2 {
            // SAFETY: live exclusive fixture handle; deliberately violates nondequeue.
            unsafe {
                (*handle.cast::<FakeNative>()).queue.pop_front();
            }
        }
        rc
    }
    #[test]
    fn native_adapter_rejects_broken_n_and_query_dequeue() {
        let mut api = fake_api();
        api.recv = bad_recv_n;
        // SAFETY: fixture memory ownership is valid; return-code contract is broken on purpose.
        let mut link = unsafe { NativeLink::open(api, 1) }.unwrap();
        assert!(link.query().unwrap_err().contains("would-block"));
        assert!(link.handle.is_none());
        let mut api = fake_api();
        api.recv = bad_query_dequeues;
        // SAFETY: fixture memory ownership remains valid despite intentional dequeue defect.
        let mut link = unsafe { NativeLink::open(api, 1) }.unwrap();
        assert!(link.send(&[0xa0]).unwrap());
        assert_eq!(link.query().unwrap(), Some(1));
        assert!(link.recv().unwrap_err().contains("dequeued"));
        assert!(link.handle.is_none());
    }
    #[test]
    fn fragmented_frame_round_trip() {
        let mut wire = Vec::new();
        write_frame(&mut wire, &[0xa0]).unwrap();
        let mut decoder = FrameDecoder::default();
        let mut frames = Vec::new();
        for byte in wire {
            frames.extend(decoder.push(&[byte]).unwrap());
        }
        assert_eq!(frames, vec![vec![0xa0]]);
        decoder.finish().unwrap();
    }
}
