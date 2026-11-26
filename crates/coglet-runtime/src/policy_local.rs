//! ADR-0033 §1: opt-in, authenticated local producer endpoint.
//!
//! Transport only: a private Unix socket, kernel peer-UID authentication, a finite
//! configured principal -> actor/target/ticket binding, and closed bounded
//! duplicate-rejecting newline-delimited JSON frames. Policy decisions and the
//! effects themselves live behind [`LocalOps`] (implemented by `DeviceHandler`).
//! The exact schema is published in `policy-local-interface.md`, with shared
//! vectors in `tests/vectors/policy_local_frames.json`.
use crate::{Result, validate};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
};

/// At most 64 KiB per request line (excluding the newline) and per reply.
pub(crate) const MAX_FRAME: usize = 64 * 1024;
const MAX_PRINCIPALS: usize = 16;
const MAX_NAME: usize = 256;

/// Operator-configured producer profile. Absent means the endpoint does not
/// exist and legacy behavior is unchanged.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProducerProfile {
    /// Absolute path whose immediate parent is an owner-only (0700) directory.
    pub socket: PathBuf,
    pub principals: Vec<PrincipalBinding>,
}

/// One local principal. A UID is local operator authority, never proof of a
/// remote actor; the request must repeat this exact binding.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PrincipalBinding {
    pub principal: String,
    pub uid: u32,
    pub actor: String,
    pub target: String,
    #[serde(default)]
    pub ticket: Option<String>,
    pub stop: StopPlan,
}

/// The pre-configured emergency `hub.stop` that is retained in the WAL before any
/// lease is granted. `actor`/`ticket`/`params` must be exactly what the original
/// hub authority will present; they are never taken from a later reader.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopPlan {
    pub actor: String,
    #[serde(default)]
    pub ticket: Option<String>,
    pub operation_id: String,
    #[serde(default = "empty_object")]
    pub params: Value,
}
fn empty_object() -> Value {
    json!({})
}

fn name_ok(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_NAME
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-' | b'@'))
}

impl ProducerProfile {
    /// Fail startup on an ambiguous or unsafe profile.
    pub(crate) fn validate(&self, targets: &BTreeMap<String, crate::TargetPin>) -> Result<()> {
        if !self.socket.is_absolute() {
            return Err("producer socket path must be absolute".into());
        }
        if self.principals.is_empty() || self.principals.len() > MAX_PRINCIPALS {
            return Err("producer profile needs 1..=16 principals".into());
        }
        let mut names = BTreeSet::new();
        let mut stops = BTreeSet::new();
        for p in &self.principals {
            if !name_ok(&p.principal) || !names.insert(p.principal.as_str()) {
                return Err("invalid or duplicate producer principal".into());
            }
            if !targets.contains_key(&p.target) {
                return Err("producer principal targets an unpinned coglet".into());
            }
            if p.actor.is_empty()
                || p.actor.len() > MAX_NAME
                || p.stop.actor.is_empty()
                || p.stop.actor.len() > MAX_NAME
                || p.ticket
                    .as_deref()
                    .is_some_and(|t| t.is_empty() || t.len() > MAX_NAME)
                || p.stop
                    .ticket
                    .as_deref()
                    .is_some_and(|t| t.is_empty() || t.len() > MAX_NAME)
                || !p.stop.params.is_object()
            {
                return Err("invalid producer actor/ticket/stop binding".into());
            }
            validate("correlation-id", &json!(p.stop.operation_id))?;
            if !stops.insert((p.target.as_str(), p.stop.operation_id.as_str())) {
                return Err("duplicate emergency stop operation ID for a target".into());
            }
        }
        Ok(())
    }
    pub(crate) fn binding(&self, principal: &str) -> Option<&PrincipalBinding> {
        self.principals.iter().find(|p| p.principal == principal)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    AttestApplied,
    ObserveHealth,
    EnableOutputLease,
    RevokeOutputLease,
    ReconcileEmergencyStop,
}
impl Method {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::AttestApplied => "attest_applied",
            Self::ObserveHealth => "observe_health",
            Self::EnableOutputLease => "enable_output_lease",
            Self::RevokeOutputLease => "revoke_output_lease",
            Self::ReconcileEmergencyStop => "reconcile_emergency_stop",
        }
    }
    fn parse(name: &str) -> Option<Self> {
        [
            Self::AttestApplied,
            Self::ObserveHealth,
            Self::EnableOutputLease,
            Self::RevokeOutputLease,
            Self::ReconcileEmergencyStop,
        ]
        .into_iter()
        .find(|m| m.as_str() == name)
    }
}

/// Exact generation pin carried by attest/health/enable.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Params {
    pub generation: Option<u64>,
    pub commit: Option<String>,
    pub executable_sha256: Option<String>,
    pub daemon_instance: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    pub method: Method,
    pub nonce: String,
    pub principal: String,
    pub actor: String,
    pub target: String,
    pub ticket: Option<String>,
    pub params: Params,
}

/// Reject duplicate object keys at any depth BEFORE building a map (serde_json's
/// `Value` silently keeps the last). Structure-only scan; the real parse follows.
fn reject_duplicate_keys(raw: &str) -> Result<()> {
    let bytes = raw.as_bytes();
    // (is_object, keys seen, next string is a key)
    let mut stack: Vec<(bool, BTreeSet<String>, bool)> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < bytes.len() && bytes[i] != b'"' {
                    i += if bytes[i] == b'\\' { 2 } else { 1 };
                }
                if i >= bytes.len() {
                    return Err("unterminated string".into());
                }
                if let Some((true, keys, expect_key)) = stack.last_mut()
                    && *expect_key
                {
                    let key: String = serde_json::from_str(&raw[start..=i])
                        .map_err(|_| "invalid object key".to_owned())?;
                    if !keys.insert(key) {
                        return Err("duplicate object key".into());
                    }
                    *expect_key = false;
                }
            }
            b'{' => stack.push((true, BTreeSet::new(), true)),
            b'[' => stack.push((false, BTreeSet::new(), false)),
            b'}' | b']' => {
                stack.pop();
            }
            b',' => {
                if let Some((true, _, expect_key)) = stack.last_mut() {
                    *expect_key = true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    Ok(())
}

fn closed(object: &serde_json::Map<String, Value>, names: &[&str]) -> Result<()> {
    if object.len() != names.len() || names.iter().any(|n| !object.contains_key(*n)) {
        return Err("closed object field mismatch".into());
    }
    Ok(())
}
fn text(value: &Value, max: usize) -> Result<&str> {
    value
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= max)
        .ok_or_else(|| "invalid string field".into())
}
fn hex_len(value: &Value, lens: &[usize]) -> Result<String> {
    let s = value.as_str().ok_or("invalid hex field")?;
    if !lens.contains(&s.len()) || !s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err("invalid hex field".into());
    }
    Ok(s.to_owned())
}

/// Parse one request line (newline already stripped).
pub(crate) fn parse_request(line: &[u8]) -> Result<Request> {
    if line.is_empty() || line.len() > MAX_FRAME {
        return Err("invalid frame size".into());
    }
    let raw = std::str::from_utf8(line).map_err(|_| "frame is not UTF-8")?;
    reject_duplicate_keys(raw)?;
    let value: Value = serde_json::from_str(raw).map_err(|_| "malformed frame")?;
    let object = value.as_object().ok_or("frame must be an object")?;
    closed(
        object,
        &[
            "version",
            "method",
            "nonce",
            "principal",
            "actor",
            "target",
            "ticket",
            "params",
        ],
    )?;
    if object["version"].as_u64() != Some(1) {
        return Err("unsupported version".into());
    }
    let method = Method::parse(text(&object["method"], 64)?).ok_or("unknown method")?;
    let nonce = text(&object["nonce"], 128)?;
    if !nonce
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b':' | b'-'))
    {
        return Err("invalid nonce".into());
    }
    let ticket = match &object["ticket"] {
        Value::Null => None,
        other => Some(text(other, MAX_NAME)?.to_owned()),
    };
    let params = object["params"]
        .as_object()
        .ok_or("params must be object")?;
    let pin = |p: &serde_json::Map<String, Value>, with_instance: bool| -> Result<Params> {
        let mut names = vec!["generation", "commit", "executable_sha256"];
        if with_instance {
            names.push("daemon_instance");
        }
        closed(p, &names)?;
        Ok(Params {
            generation: Some(p["generation"].as_u64().ok_or("invalid generation")?),
            commit: Some(hex_len(&p["commit"], &[40, 64])?),
            executable_sha256: Some(hex_len(&p["executable_sha256"], &[64])?),
            daemon_instance: if with_instance {
                Some(hex_len(&p["daemon_instance"], &[64])?)
            } else {
                None
            },
        })
    };
    let params = match method {
        Method::AttestApplied | Method::ObserveHealth => pin(params, false)?,
        Method::EnableOutputLease => pin(params, true)?,
        Method::RevokeOutputLease | Method::ReconcileEmergencyStop => {
            closed(params, &[])?;
            Params::default()
        }
    };
    Ok(Request {
        method,
        nonce: nonce.to_owned(),
        principal: text(&object["principal"], MAX_NAME)?.to_owned(),
        actor: text(&object["actor"], MAX_NAME)?.to_owned(),
        target: text(&object["target"], MAX_NAME)?.to_owned(),
        ticket,
        params,
    })
}

pub(crate) fn reply_ok(request: &Request, daemon_instance: &str, result: Value) -> Value {
    json!({"version":1,"ok":true,"method":request.method.as_str(),"nonce":request.nonce,
        "principal":request.principal,"actor":request.actor,"target":request.target,
        "ticket":request.ticket,"daemon_instance":daemon_instance,"result":result})
}
pub(crate) fn reply_err(nonce: Option<&str>, code: &str) -> Value {
    json!({"version":1,"ok":false,"nonce":nonce,
        "error":{"code":code,"retryable":matches!(code,"unavailable"|"resource_exhausted")}})
}

/// Authenticated request to effect mapping, implemented by the device handler.
pub(crate) trait LocalOps: Send + Sync + 'static {
    fn daemon_instance(&self) -> &str;
    /// `binding` is already proven to match the peer UID and every request
    /// identity field. Errors are wire codes only (no internal detail).
    fn call(
        &self,
        binding: &PrincipalBinding,
        request: &Request,
    ) -> std::result::Result<Value, &'static str>;
}

/// Authenticate a parsed request against the profile and the kernel peer UID.
/// Everything is refused as one undisclosing `permission_denied`.
pub(crate) fn authenticate<'a>(
    profile: &'a ProducerProfile,
    peer_uid: u32,
    request: &Request,
) -> std::result::Result<&'a PrincipalBinding, &'static str> {
    let binding = profile
        .binding(&request.principal)
        .ok_or("permission_denied")?;
    if binding.uid != peer_uid
        || binding.actor != request.actor
        || binding.target != request.target
        || binding.ticket != request.ticket
    {
        return Err("permission_denied");
    }
    Ok(binding)
}

/// One frame in, one frame out. `true` means close the connection afterward.
pub(crate) fn handle_frame(
    profile: &ProducerProfile,
    ops: &dyn LocalOps,
    peer_uid: u32,
    seen: &mut BTreeSet<String>,
    line: &[u8],
) -> (Value, bool) {
    let request = match parse_request(line) {
        Ok(request) => request,
        Err(_) => return (reply_err(None, "invalid_argument"), true),
    };
    let binding = match authenticate(profile, peer_uid, &request) {
        Ok(binding) => binding,
        Err(code) => return (reply_err(None, code), true),
    };
    if seen.len() >= 256 || !seen.insert(request.nonce.clone()) {
        return (reply_err(Some(&request.nonce), "invalid_argument"), true);
    }
    match ops.call(binding, &request) {
        Ok(result) => (reply_ok(&request, ops.daemon_instance(), result), false),
        Err(code) => (reply_err(Some(&request.nonce), code), false),
    }
}

/// ADR-0043 reserved lane: exactly one `revoke_output_lease` per connection.
/// Parsing and authentication are the ordinary ones; any other authenticated
/// method is refused with its nonce WITHOUT reaching [`LocalOps::call`], so the
/// lane can never wait on SQL or runtime custody. The caller always closes.
pub(crate) fn handle_emergency_frame(
    profile: &ProducerProfile,
    ops: &dyn LocalOps,
    peer_uid: u32,
    line: &[u8],
) -> Value {
    let request = match parse_request(line) {
        Ok(request) => request,
        Err(_) => return reply_err(None, "invalid_argument"),
    };
    let binding = match authenticate(profile, peer_uid, &request) {
        Ok(binding) => binding,
        Err(code) => return reply_err(None, code),
    };
    if request.method != Method::RevokeOutputLease {
        return reply_err(Some(&request.nonce), "invalid_argument");
    }
    match ops.call(binding, &request) {
        Ok(result) => reply_ok(&request, ops.daemon_instance(), result),
        Err(code) => reply_err(Some(&request.nonce), code),
    }
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
pub(crate) use unix::LocalListener;

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
mod unix {
    use super::*;
    use std::{
        io::{Read, Write},
        os::unix::{
            fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
            net::{UnixListener, UnixStream},
        },
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::{Duration, Instant},
    };

    const MAX_CONNECTIONS: usize = 16;
    /// ADR-0043: reserved revoke-only admission, never shared with ordinary load.
    const MAX_EMERGENCY: usize = 4;
    const EMERGENCY_SUFFIX: &str = ".revoke";
    const IDLE: Duration = Duration::from_secs(10);
    const FRAME: Duration = Duration::from_secs(3);
    /// Whole emergency request, measured from acceptance (no idle allowance).
    const EMERGENCY_REQUEST: Duration = Duration::from_secs(3);

    /// The revoke-only sibling: the literal suffix appended to the COMPLETE
    /// socket path (never an extension replacement).
    fn emergency_path(socket: &std::path::Path) -> PathBuf {
        let mut path = socket.as_os_str().to_owned();
        path.push(EMERGENCY_SUFFIX);
        PathBuf::from(path)
    }

    /// One bound socket created by THIS startup; dropping it unlinks only that
    /// exact inode, never a live or foreign replacement.
    pub(super) struct Bound {
        listener: UnixListener,
        path: PathBuf,
        inode: u64,
        device: u64,
    }
    impl Drop for Bound {
        fn drop(&mut self) {
            if std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| m.ino() == self.inode && m.dev() == self.device)
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }

    /// Read-only admission of one path. `Ok(true)`: a refused-connection stale
    /// socket of ours (after SIGKILL) that may be unlinked, since boot-closed
    /// custody makes that safe. A live listener is never displaced.
    fn preflight(path: &std::path::Path, euid: u32) -> Result<bool> {
        use std::os::unix::ffi::OsStrExt;
        // SAFETY: sockaddr_un is plain old data; only its field size is read.
        let bound = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
            .sun_path
            .len();
        // Room for the terminating NUL; never truncate or omit a listener.
        if path.as_os_str().as_bytes().len() >= bound {
            return Err("producer socket path exceeds the Unix socket bound".into());
        }
        let Ok(existing) = std::fs::symlink_metadata(path) else {
            return Ok(false);
        };
        if !existing.file_type().is_socket() || existing.uid() != euid {
            return Err("refusing to replace a non-socket or foreign path".into());
        }
        match UnixStream::connect(path) {
            Ok(_) => Err("producer socket already has a live listener".into()),
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => Ok(true),
            Err(e) => Err(e.to_string()),
        }
    }

    pub(super) fn bind_one(path: PathBuf, stale: bool) -> Result<Bound> {
        if stale {
            std::fs::remove_file(&path).map_err(|e| e.to_string())?;
        }
        let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
        // Own the inode before anything else can fail, so cleanup is exact.
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        let bound = Bound {
            listener,
            path,
            inode: meta.ino(),
            device: meta.dev(),
        };
        std::fs::set_permissions(&bound.path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| e.to_string())?;
        Ok(bound)
    }

    /// Releases one admission slot on completion, error or panic.
    struct Slot(Arc<AtomicUsize>);
    impl Drop for Slot {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    type Go = (std::sync::Mutex<Option<bool>>, std::sync::Condvar);
    fn wait_go(go: &Go) -> bool {
        let mut state = go.0.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(run) = *state {
                return run;
            }
            state = go.1.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }

    /// The ordinary listener and its revoke-only sibling (ADR-0043). Both exist
    /// or neither does.
    pub(crate) struct LocalListener {
        ordinary: Bound,
        emergency: Bound,
        profile: Arc<ProducerProfile>,
        ops: Arc<dyn LocalOps>,
    }
    impl LocalListener {
        /// Bind both sockets (mode 0600) inside an owned 0700 parent. BOTH paths
        /// are preflighted before either is unlinked or bound; a failure of the
        /// second bind drops (and unlinks) only the first socket made here.
        pub(crate) fn bind(profile: Arc<ProducerProfile>, ops: Arc<dyn LocalOps>) -> Result<Self> {
            let path = profile.socket.clone();
            let sibling = emergency_path(&path);
            let parent = path.parent().ok_or("socket has no parent")?;
            match std::fs::DirBuilder::new().mode(0o700).create(parent) {
                Ok(()) => (),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.to_string()),
            }
            // SAFETY: geteuid has no preconditions.
            let euid = unsafe { libc::geteuid() };
            let meta = std::fs::symlink_metadata(parent).map_err(|e| e.to_string())?;
            if !meta.is_dir() || meta.uid() != euid || meta.mode() & 0o7777 != 0o700 {
                return Err("socket parent must be owned directory mode 0700".into());
            }
            let stale = preflight(&path, euid)?;
            let sibling_stale = preflight(&sibling, euid)?;
            let ordinary = bind_one(path, stale)?;
            let emergency = bind_one(sibling, sibling_stale)?;
            Ok(Self {
                ordinary,
                emergency,
                profile,
                ops,
            })
        }

        /// Start both accept workers, or neither: they wait for a shared go
        /// signal, so a failed spawn drops (and unlinks) both sockets before any
        /// connection is served.
        pub(crate) fn start(self) -> Result<()> {
            let Self {
                ordinary,
                emergency,
                profile,
                ops,
            } = self;
            let go: Arc<Go> = Arc::new((std::sync::Mutex::new(None), std::sync::Condvar::new()));
            let signal = |run: bool| {
                *go.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
                go.1.notify_all();
            };
            let (wait, lane_profile, lane_ops) = (go.clone(), profile.clone(), ops.clone());
            std::thread::Builder::new()
                .name("cogletd-emerg-accept".into())
                .spawn(move || {
                    if wait_go(&wait) {
                        serve_emergency(emergency, lane_profile, lane_ops);
                    }
                })
                .map_err(|e| e.to_string())?;
            let wait = go.clone();
            let spawned = std::thread::Builder::new()
                .name("cogletd-producer".into())
                .spawn(move || {
                    if wait_go(&wait) {
                        serve_ordinary(ordinary, profile, ops);
                    }
                });
            signal(spawned.is_ok());
            spawned.map(drop).map_err(|e| e.to_string())
        }
    }

    /// Accept forever. Each connection has its own thread, bounded at 16. This
    /// pool alone gives revoke NO liveness under ordinary saturation (ADR-0043).
    fn serve_ordinary(bound: Bound, profile: Arc<ProducerProfile>, ops: Arc<dyn LocalOps>) {
        let active = Arc::new(AtomicUsize::new(0));
        loop {
            let stream = match bound.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) => {
                    eprintln!("cogletd producer accept: {error}");
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_CONNECTIONS {
                active.fetch_sub(1, Ordering::SeqCst);
                continue; // drop: too many local connections
            }
            let (profile, ops, slots) = (profile.clone(), ops.clone(), active.clone());
            let spawned = std::thread::Builder::new()
                .name("cogletd-producer".into())
                .spawn(move || {
                    if let Err(error) = serve_connection(stream, &profile, ops.as_ref()) {
                        eprintln!("cogletd producer connection: {error}");
                    }
                    slots.fetch_sub(1, Ordering::SeqCst);
                });
            if spawned.is_err() {
                active.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }

    /// The reserved lane: its own accept loop, counter and four workers. A full
    /// pool closes the new connection; it never borrows ordinary capacity.
    fn serve_emergency(bound: Bound, profile: Arc<ProducerProfile>, ops: Arc<dyn LocalOps>) {
        let active = Arc::new(AtomicUsize::new(0));
        loop {
            let stream = match bound.listener.accept() {
                Ok((stream, _)) => stream,
                Err(error) => {
                    eprintln!("cogletd emergency accept: {error}");
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
            };
            let accepted = Instant::now();
            // Kernel UID first: an unconfigured peer never occupies a worker.
            let uid = match crate::socket::peer_uid(&stream) {
                Ok(uid) if profile.principals.iter().any(|p| p.uid == uid) => uid,
                _ => continue,
            };
            if active.fetch_add(1, Ordering::SeqCst) >= MAX_EMERGENCY {
                active.fetch_sub(1, Ordering::SeqCst);
                continue; // drop: reserved pool exhausted
            }
            let slot = Slot(active.clone());
            let (profile, ops) = (profile.clone(), ops.clone());
            // A failed spawn drops the closure, which releases the slot.
            let _ = std::thread::Builder::new()
                .name("cogletd-emerg".into())
                .spawn(move || {
                    let _slot = slot;
                    if let Err(error) =
                        serve_emergency_connection(stream, uid, accepted, &profile, ops.as_ref())
                    {
                        eprintln!("cogletd emergency connection: {error}");
                    }
                });
        }
    }

    /// One request, one reply attempt, immediate close (no lingering drain).
    fn serve_emergency_connection(
        mut stream: UnixStream,
        uid: u32,
        accepted: Instant,
        profile: &ProducerProfile,
        ops: &dyn LocalOps,
    ) -> Result<()> {
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        let deadline = accepted + EMERGENCY_REQUEST;
        let mut buffer: Vec<u8> = Vec::new();
        let reply = loop {
            if let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                break if pos > MAX_FRAME {
                    reply_err(None, "resource_exhausted")
                } else {
                    handle_emergency_frame(profile, ops, uid, &buffer[..pos])
                };
            }
            if buffer.len() > MAX_FRAME {
                break reply_err(None, "resource_exhausted");
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero())
                .ok_or("emergency request deadline")?;
            stream
                .set_read_timeout(Some(remaining.min(Duration::from_millis(100))))
                .map_err(|e| e.to_string())?;
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => return Err("EOF before a complete emergency request".into()),
                Ok(n) => buffer.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted
                            | std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        };
        let mut bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        if bytes.len() >= MAX_FRAME {
            return Err("reply exceeds frame bound".into());
        }
        bytes.push(b'\n');
        stream.write_all(&bytes).map_err(|e| e.to_string())
    }

    pub(super) fn serve_connection(
        mut stream: UnixStream,
        profile: &ProducerProfile,
        ops: &dyn LocalOps,
    ) -> Result<()> {
        let uid = crate::socket::peer_uid(&stream)?;
        // Not any configured principal's UID: close without reading a byte.
        if !profile.principals.iter().any(|p| p.uid == uid) {
            return Err("unauthenticated peer UID".into());
        }
        stream
            .set_write_timeout(Some(Duration::from_secs(3)))
            .map_err(|e| e.to_string())?;
        let mut buffer: Vec<u8> = Vec::new();
        let mut seen = BTreeSet::new();
        loop {
            let Some(line) = next_line(&mut stream, &mut buffer)? else {
                return Ok(());
            };
            let (reply, close) = match line {
                Line::Frame(line) => handle_frame(profile, ops, uid, &mut seen, &line),
                Line::Oversize => (reply_err(None, "resource_exhausted"), true),
            };
            let mut bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
            if bytes.len() >= MAX_FRAME {
                return Err("reply exceeds frame bound".into());
            }
            bytes.push(b'\n');
            stream.write_all(&bytes).map_err(|e| e.to_string())?;
            if close {
                // Let the peer read the refusal instead of resetting it over
                // unread input.
                let _ = stream.shutdown(std::net::Shutdown::Write);
                let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
                let mut sink = [0u8; 4096];
                for _ in 0..32 {
                    if !matches!(stream.read(&mut sink), Ok(n) if n > 0) {
                        break;
                    }
                }
                return Ok(());
            }
        }
    }

    enum Line {
        Frame(Vec<u8>),
        Oversize,
    }
    /// `Ok(None)`: clean EOF or idle timeout between frames.
    fn next_line(stream: &mut UnixStream, buffer: &mut Vec<u8>) -> Result<Option<Line>> {
        let mut deadline = Instant::now() + if buffer.is_empty() { IDLE } else { FRAME };
        let mut started = !buffer.is_empty();
        loop {
            if let Some(pos) = buffer.iter().position(|b| *b == b'\n') {
                let mut line: Vec<u8> = buffer.drain(..=pos).collect();
                line.pop();
                return Ok(Some(if line.len() > MAX_FRAME {
                    Line::Oversize
                } else {
                    Line::Frame(line)
                }));
            }
            if buffer.len() > MAX_FRAME {
                return Ok(Some(Line::Oversize));
            }
            let remaining = deadline
                .checked_duration_since(Instant::now())
                .filter(|d| !d.is_zero());
            let Some(remaining) = remaining else {
                return if started {
                    Err("frame timeout".into())
                } else {
                    Ok(None)
                };
            };
            stream
                .set_read_timeout(Some(remaining.min(Duration::from_millis(100))))
                .map_err(|e| e.to_string())?;
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) if buffer.is_empty() => return Ok(None),
                Ok(0) => return Err("EOF mid-frame".into()),
                Ok(n) => {
                    if !started {
                        started = true;
                        deadline = Instant::now() + FRAME;
                    }
                    buffer.extend_from_slice(&chunk[..n]);
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const VECTORS: &str = include_str!("../tests/vectors/policy_local_frames.json");
    const COMMIT: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    #[test]
    fn shared_frame_vectors() {
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let mut valid = 0;
        let mut invalid = 0;
        for vector in vectors["vectors"].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            let frame = vector["frame"].as_str().unwrap();
            let outcome = parse_request(frame.as_bytes());
            if vector["valid"].as_bool().unwrap() {
                valid += 1;
                let request = outcome.unwrap_or_else(|e| panic!("{name}: {e}"));
                assert_eq!(request.method.as_str(), vector["method"], "{name}");
            } else {
                invalid += 1;
                assert!(outcome.is_err(), "{name} must be rejected");
            }
        }
        assert!(valid >= 5 && invalid >= 14, "{valid}/{invalid}");
        // Over-size frames are generated, not stored.
        let pad = "x".repeat(MAX_FRAME);
        assert!(parse_request(format!("{{\"a\":\"{pad}\"}}").as_bytes()).is_err());
    }

    /// ADR-0043 shared vectors: the reserved endpoint dispatches only revoke.
    #[test]
    fn shared_reserved_endpoint_vectors() {
        struct Count(std::sync::atomic::AtomicUsize);
        impl LocalOps for Count {
            fn daemon_instance(&self) -> &str {
                "inst"
            }
            fn call(
                &self,
                _: &PrincipalBinding,
                request: &Request,
            ) -> std::result::Result<Value, &'static str> {
                assert_eq!(request.method, Method::RevokeOutputLease);
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(json!({}))
            }
        }
        let vectors: Value = serde_json::from_str(VECTORS).unwrap();
        let (mut dispatched, mut refused) = (0, 0);
        for vector in vectors["reserved_endpoint"].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            let frame = vector["frame"].as_str().unwrap();
            // The peer holds exactly the binding the frame names.
            let mut profile = profile(7);
            if let Ok(Value::Object(sent)) = serde_json::from_str::<Value>(frame) {
                let binding = &mut profile.principals[0];
                binding.ticket = sent
                    .get("ticket")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                if let Some(actor) = sent.get("actor").and_then(Value::as_str) {
                    binding.actor = actor.into();
                }
                if let Some(target) = sent.get("target").and_then(Value::as_str) {
                    binding.target = target.into();
                }
            }
            let ops = Count(Default::default());
            let reply = handle_emergency_frame(&profile, &ops, 7, frame.as_bytes());
            let calls = ops.0.load(std::sync::atomic::Ordering::SeqCst);
            if vector["expect"] == "dispatch" {
                dispatched += 1;
                assert_eq!(reply["ok"], true, "{name}: {reply}");
                assert_eq!(calls, 1, "{name}");
            } else {
                refused += 1;
                assert_eq!(reply["ok"], false, "{name}: {reply}");
                assert_eq!(reply["error"]["code"], vector["expect"], "{name}");
                assert_eq!(calls, 0, "{name}: dispatched on the reserved endpoint");
            }
            assert_eq!(
                reply["nonce"].is_string(),
                vector["nonce_echoed"].as_bool().unwrap(),
                "{name}: {reply}"
            );
        }
        assert!(dispatched >= 1 && refused >= 12, "{dispatched}/{refused}");
    }

    fn profile(uid: u32) -> ProducerProfile {
        ProducerProfile {
            socket: PathBuf::from("/tmp/unused"),
            principals: vec![PrincipalBinding {
                principal: "operator".into(),
                uid,
                actor: "actor-1".into(),
                target: "coglet.one".into(),
                ticket: Some("ticket-1".into()),
                stop: StopPlan {
                    actor: "stop-actor".into(),
                    ticket: None,
                    operation_id: "a".repeat(32),
                    params: json!({}),
                },
            }],
        }
    }
    fn frame(mutate: impl FnOnce(&mut Value)) -> Vec<u8> {
        let mut value = json!({"version":1,"method":"revoke_output_lease","nonce":"n-1",
            "principal":"operator","actor":"actor-1","target":"coglet.one",
            "ticket":"ticket-1","params":{}});
        mutate(&mut value);
        serde_json::to_vec(&value).unwrap()
    }
    struct Echo;
    impl LocalOps for Echo {
        fn daemon_instance(&self) -> &str {
            "inst"
        }
        fn call(
            &self,
            b: &PrincipalBinding,
            r: &Request,
        ) -> std::result::Result<Value, &'static str> {
            assert_eq!(b.principal, r.principal);
            Ok(json!({"seen":r.method.as_str(),"commit":COMMIT}))
        }
    }

    #[test]
    fn binding_mismatch_is_refused_before_effect_or_disclosure() {
        let profile = profile(1000);
        for (label, line, uid) in [
            ("wrong uid", frame(|_| {}), 1001),
            (
                "unknown principal",
                frame(|v| v["principal"] = json!("root")),
                1000,
            ),
            (
                "wrong actor",
                frame(|v| v["actor"] = json!("actor-2")),
                1000,
            ),
            (
                "wrong target",
                frame(|v| v["target"] = json!("coglet.two")),
                1000,
            ),
            (
                "wrong ticket",
                frame(|v| v["ticket"] = json!("ticket-2")),
                1000,
            ),
            ("absent ticket", frame(|v| v["ticket"] = Value::Null), 1000),
        ] {
            let (reply, close) = handle_frame(&profile, &Echo, uid, &mut BTreeSet::new(), &line);
            assert_eq!(reply["ok"], false, "{label}");
            assert_eq!(reply["error"]["code"], "permission_denied", "{label}");
            assert_eq!(reply["nonce"], Value::Null, "{label}: no echo before auth");
            assert!(reply.get("actor").is_none() && reply.get("result").is_none());
            assert!(close, "{label}");
        }
        let (reply, close) =
            handle_frame(&profile, &Echo, 1000, &mut BTreeSet::new(), &frame(|_| {}));
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["nonce"], "n-1");
        assert_eq!(reply["daemon_instance"], "inst");
        assert_eq!(reply["actor"], "actor-1");
        assert!(!close);
    }

    #[test]
    fn repeated_nonce_on_one_connection_is_rejected() {
        let profile = profile(1000);
        let mut seen = BTreeSet::new();
        assert_eq!(
            handle_frame(&profile, &Echo, 1000, &mut seen, &frame(|_| {})).0["ok"],
            true
        );
        let (reply, close) = handle_frame(&profile, &Echo, 1000, &mut seen, &frame(|_| {}));
        assert_eq!(reply["error"]["code"], "invalid_argument");
        assert!(close);
    }

    #[test]
    fn profile_validation_rejects_ambiguity() {
        let mut targets = BTreeMap::new();
        targets.insert(
            "coglet.one".to_owned(),
            serde_json::from_value::<crate::TargetPin>(
                json!({"grant_generation":1,"delegation_generation":1}),
            )
            .unwrap(),
        );
        let ok = profile(1000);
        ok.validate(&targets).unwrap();
        let mut bad = ok.clone();
        bad.socket = PathBuf::from("relative.sock");
        assert!(bad.validate(&targets).is_err());
        let mut bad = ok.clone();
        bad.principals.push(bad.principals[0].clone());
        assert!(bad.validate(&targets).is_err(), "duplicate principal");
        let mut bad = ok.clone();
        bad.principals[0].target = "unpinned".into();
        assert!(bad.validate(&targets).is_err());
        let mut bad = ok.clone();
        bad.principals[0].stop.operation_id = "not valid!".into();
        assert!(bad.validate(&targets).is_err());
        let mut bad = ok;
        bad.principals.clear();
        assert!(bad.validate(&targets).is_err());
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    mod socket {
        use super::*;
        use std::io::{BufRead, BufReader, Write};
        use std::os::unix::net::UnixStream;
        use std::sync::Arc;

        fn euid() -> u32 {
            // SAFETY: geteuid has no preconditions.
            unsafe { libc::geteuid() }
        }
        fn start(uid: u32) -> (tempfile::TempDir, PathBuf) {
            start_with(uid, Arc::new(Echo))
        }
        fn start_with(uid: u32, ops: Arc<dyn LocalOps>) -> (tempfile::TempDir, PathBuf) {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("p").join("producer.sock");
            let mut profile = profile(uid);
            profile.socket = socket.clone();
            unix::LocalListener::bind(Arc::new(profile), ops)
                .unwrap()
                .start()
                .unwrap();
            (dir, socket)
        }
        fn sibling(socket: &std::path::Path) -> PathBuf {
            PathBuf::from(format!("{}.revoke", socket.display()))
        }
        /// Counts dispatched calls; `hold` keeps every call blocked until released.
        #[derive(Default)]
        struct Counting {
            calls: std::sync::atomic::AtomicUsize,
            hold: std::sync::Mutex<()>,
        }
        impl LocalOps for Counting {
            fn daemon_instance(&self) -> &str {
                "inst"
            }
            fn call(
                &self,
                _: &PrincipalBinding,
                r: &Request,
            ) -> std::result::Result<Value, &'static str> {
                self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if r.method != Method::RevokeOutputLease {
                    drop(self.hold.lock().unwrap());
                }
                Ok(json!({"seen":r.method.as_str()}))
            }
        }
        /// The server closed without any reply (dropped, refused or timed out).
        fn closed_without_reply(stream: UnixStream) -> bool {
            let mut line = String::new();
            matches!(BufReader::new(stream).read_line(&mut line), Ok(0) | Err(_)) && line.is_empty()
        }

        /// Unit/transport control for ADR-0043 (mock `LocalOps`); the real-daemon
        /// proof is `emergency_output_revoke_survives_ordinary_admission_saturation`.
        #[test]
        fn reserved_lane_admits_revoke_while_ordinary_pool_is_saturated() {
            let ops = Arc::new(Counting::default());
            let (_dir, socket) = start_with(euid(), ops.clone());
            let held = ops.hold.lock().unwrap();
            // 8 authenticated operations blocked inside LocalOps, 4 idle and 4
            // partial-frame connections: all 16 ordinary slots.
            let mut blockers = vec![];
            for i in 0..8 {
                let mut stream = UnixStream::connect(&socket).unwrap();
                let mut line = frame(|v| {
                    v["method"] = json!("reconcile_emergency_stop");
                    v["nonce"] = json!(format!("b{i}"));
                });
                line.push(b'\n');
                stream.write_all(&line).unwrap();
                blockers.push(stream);
            }
            for i in 0..8 {
                let mut stream = UnixStream::connect(&socket).unwrap();
                if i % 2 == 0 {
                    stream.write_all(b"{\"version\":1,").unwrap();
                }
                blockers.push(stream);
            }
            while ops.calls.load(std::sync::atomic::Ordering::SeqCst) < 8 {
                std::thread::yield_now();
            }
            // Occupancy is proven by the 17th ordinary connection being dropped.
            let dropped = |socket: &PathBuf| {
                let mut stream = UnixStream::connect(socket).unwrap();
                let _ = stream.write_all(&frame(|v| v["nonce"] = json!("x")));
                let _ = stream.write_all(b"\n");
                closed_without_reply(stream)
            };
            assert!(dropped(&socket), "ordinary pool was not saturated");
            let reply = exchange(&sibling(&socket), &frame(|_| {})).unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            assert_eq!(reply["result"]["seen"], "revoke_output_lease");
            assert!(
                dropped(&socket),
                "ordinary pool freed before revoke returned"
            );
            assert_eq!(ops.calls.load(std::sync::atomic::Ordering::SeqCst), 9);
            drop(held);
        }

        #[test]
        fn reserved_lane_is_revoke_only_bounded_and_reclaims_slots() {
            let ops = Arc::new(Counting::default());
            let (_dir, socket) = start_with(euid(), ops.clone());
            let lane = sibling(&socket);
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&lane).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
            for method in [
                "attest_applied",
                "observe_health",
                "enable_output_lease",
                "reconcile_emergency_stop",
            ] {
                let pins =
                    json!({"generation":1,"commit":COMMIT,"executable_sha256":"b".repeat(64)});
                let reply = exchange(
                    &lane,
                    &frame(|v| {
                        v["method"] = json!(method);
                        v["params"] = match method {
                            "reconcile_emergency_stop" => json!({}),
                            "enable_output_lease" => {
                                let mut p = pins.clone();
                                p["daemon_instance"] = json!("c".repeat(64));
                                p
                            }
                            _ => pins.clone(),
                        };
                    }),
                )
                .unwrap();
                assert_eq!(reply["error"]["code"], "invalid_argument", "{method}");
                assert_eq!(reply["nonce"], "n-1", "{method}: authenticated refusal");
            }
            for forged in [
                frame(|v| v["actor"] = json!("actor-2")),
                frame(|v| v["ticket"] = Value::Null),
                frame(|v| v["principal"] = json!("root")),
            ] {
                let reply = exchange(&lane, &forged).unwrap();
                assert_eq!(reply["error"]["code"], "permission_denied");
                assert_eq!(reply["nonce"], Value::Null);
            }
            let reply = exchange(&lane, &frame(|v| v["extra"] = json!(1))).unwrap();
            assert_eq!(reply["error"]["code"], "invalid_argument");
            let reply = exchange(&lane, &vec![b'x'; MAX_FRAME + 10]).unwrap();
            assert_eq!(reply["error"]["code"], "resource_exhausted");
            assert_eq!(
                ops.calls.load(std::sync::atomic::Ordering::SeqCst),
                0,
                "nothing above may reach LocalOps"
            );
            // One request per connection: a second frame is never answered.
            let mut stream = UnixStream::connect(&lane).unwrap();
            let mut two = frame(|_| {});
            two.push(b'\n');
            two.extend(frame(|v| v["nonce"] = json!("n-2")));
            two.push(b'\n');
            let _ = stream.write_all(&two);
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["nonce"],
                "n-1"
            );
            line.clear();
            assert!(matches!(reader.read_line(&mut line), Ok(0) | Err(_)) && line.is_empty());
            assert_eq!(ops.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            // Four idle/partial connections exhaust the reserved pool: the fifth
            // is closed, never queued and never moved to the ordinary pool.
            let started = std::time::Instant::now();
            let mut idle = vec![];
            for i in 0..4 {
                let mut stream = UnixStream::connect(&lane).unwrap();
                if i % 2 == 0 {
                    stream.write_all(b"{\"version\":1,").unwrap();
                }
                idle.push(stream);
            }
            let mut fifth = UnixStream::connect(&lane).unwrap();
            let _ = fifth.write_all(&frame(|_| {}));
            let _ = fifth.write_all(b"\n");
            assert!(closed_without_reply(fifth), "reserved pool is unbounded");
            assert_eq!(ops.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            // Each holder is closed at the 3 s request deadline without a reply,
            // which reclaims its slot for a subsequent valid revoke.
            for stream in idle {
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(8)))
                    .unwrap();
                assert!(closed_without_reply(stream));
            }
            assert!(started.elapsed() >= std::time::Duration::from_millis(2900));
            let reply = (0..50)
                .find_map(|_| {
                    let reply = exchange(&lane, &frame(|_| {}));
                    if reply.is_none() {
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    reply
                })
                .expect("slots reclaimed after the deadline");
            assert_eq!(reply["ok"], true, "{reply}");
        }

        #[test]
        fn listener_pair_binds_both_or_neither() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("p").join("producer.sock");
            let lane = sibling(&socket);
            let mut p = profile(euid());
            p.socket = socket.clone();
            let p = Arc::new(p);
            let bind = || unix::LocalListener::bind(p.clone(), Arc::new(Echo));
            // Live sibling: refused in preflight, nothing of ours is created and
            // the live listener is untouched.
            std::fs::create_dir(socket.parent().unwrap()).unwrap();
            std::fs::set_permissions(
                socket.parent().unwrap(),
                std::os::unix::fs::PermissionsExt::from_mode(0o700),
            )
            .unwrap();
            let live = std::os::unix::net::UnixListener::bind(&lane).unwrap();
            assert!(bind().is_err());
            assert!(!socket.exists() && lane.exists());
            UnixStream::connect(&lane).unwrap();
            drop(live);
            // Non-socket sibling: refused, left byte-identical.
            std::fs::remove_file(&lane).unwrap();
            std::fs::write(&lane, b"keep").unwrap();
            assert!(bind().is_err());
            assert!(!socket.exists());
            assert_eq!(std::fs::read(&lane).unwrap(), b"keep");
            std::fs::remove_file(&lane).unwrap();
            // A stale pair (SIGKILL leftovers) is replaced; a live pair never is.
            drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
            drop(std::os::unix::net::UnixListener::bind(&lane).unwrap());
            let pair = bind().unwrap();
            assert!(socket.exists() && lane.exists());
            assert!(bind().is_err(), "a live pair is never displaced");
            drop(pair);
            assert!(
                !socket.exists() && !lane.exists(),
                "own sockets are unlinked"
            );
            // Second-bind failure after a passed preflight (the sibling appears
            // in between): exactly the two `bind_one` steps of `bind`. The first
            // socket made here is unlinked; the foreign path is untouched.
            let first = unix::bind_one(socket.clone(), false).unwrap();
            std::fs::write(&lane, b"raced").unwrap();
            assert!(unix::bind_one(lane.clone(), false).is_err());
            drop(first);
            assert!(!socket.exists());
            assert_eq!(std::fs::read(&lane).unwrap(), b"raced");
            std::fs::remove_file(&lane).unwrap();
            // Suffix-length overflow: the ordinary path alone fits, the sibling
            // does not. Startup fails; no listener is created or truncated.
            // SAFETY: plain old data; only the field size is read.
            let bound = unsafe { std::mem::zeroed::<libc::sockaddr_un>() }
                .sun_path
                .len();
            let base = dir.path().join("p");
            let fill = bound - 1 - base.as_os_str().len() - 1;
            let long = base.join("s".repeat(fill));
            assert_eq!(long.as_os_str().len(), bound - 1);
            let mut q = profile(euid());
            q.socket = long.clone();
            assert!(unix::LocalListener::bind(Arc::new(q), Arc::new(Echo)).is_err());
            assert!(!long.exists());
            assert_eq!(std::fs::read_dir(&base).unwrap().count(), 0);
        }
        fn exchange(socket: &PathBuf, raw: &[u8]) -> Option<Value> {
            let mut stream = UnixStream::connect(socket).unwrap();
            // The server may legitimately close before reading everything.
            let _ = stream.write_all(raw);
            let _ = stream.write_all(b"\n");
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).ok()?;
            serde_json::from_str(&line).ok()
        }

        #[test]
        fn real_socket_authenticates_peer_uid_and_closes_frames() {
            let (_dir, socket) = start(euid());
            let reply = exchange(&socket, &frame(|_| {})).unwrap();
            assert_eq!(reply["ok"], true);
            assert_eq!(reply["result"]["seen"], "revoke_output_lease");
            // Duplicate key, unknown field, wrong version: refused, no effect.
            for raw in [
                br#"{"version":1,"version":1,"method":"revoke_output_lease","nonce":"n","principal":"operator","actor":"actor-1","target":"coglet.one","ticket":"ticket-1","params":{}}"#.to_vec(),
                frame(|v| v["extra"] = json!(1)),
                frame(|v| v["version"] = json!(2)),
            ] {
                let reply = exchange(&socket, &raw).unwrap();
                assert_eq!(reply["error"]["code"], "invalid_argument");
            }
            // Oversized line.
            let reply = exchange(&socket, &vec![b'x'; MAX_FRAME + 10]).unwrap();
            assert_eq!(reply["error"]["code"], "resource_exhausted");
            // Socket is owner-only.
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&socket).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        #[test]
        fn real_socket_refuses_unconfigured_peer_uid_without_reply() {
            let (_dir, socket) = start(euid().wrapping_add(1));
            // The peer (this process) is not a configured UID: nothing is read.
            assert!(exchange(&socket, &frame(|_| {})).is_none());
        }

        #[test]
        fn stale_socket_is_replaced_but_live_listener_is_not() {
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("p").join("producer.sock");
            let mut p = profile(euid());
            p.socket = socket.clone();
            let p = Arc::new(p);
            let first = unix::LocalListener::bind(p.clone(), Arc::new(Echo)).unwrap();
            assert!(unix::LocalListener::bind(p.clone(), Arc::new(Echo)).is_err());
            drop(first);
            // Simulate SIGKILL: a bound-then-closed socket file remains.
            drop(std::os::unix::net::UnixListener::bind(&socket).unwrap());
            assert!(socket.exists());
            unix::LocalListener::bind(p, Arc::new(Echo)).unwrap();
        }
    }
}

#[cfg(test)]
#[path = "policy_local_reply_tests.rs"]
pub(crate) mod reply_tests;
