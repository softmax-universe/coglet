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
    const IDLE: Duration = Duration::from_secs(10);
    const FRAME: Duration = Duration::from_secs(3);

    pub(crate) struct LocalListener {
        listener: UnixListener,
        path: PathBuf,
        inode: u64,
        device: u64,
        profile: Arc<ProducerProfile>,
        ops: Arc<dyn LocalOps>,
    }
    impl LocalListener {
        /// Bind (mode 0600) inside an owned 0700 parent. A live listener is never
        /// displaced; a refused-connection stale socket of ours (after SIGKILL) is
        /// unlinked, since boot-closed custody makes that safe.
        pub(crate) fn bind(profile: Arc<ProducerProfile>, ops: Arc<dyn LocalOps>) -> Result<Self> {
            let path = profile.socket.clone();
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
            if let Ok(existing) = std::fs::symlink_metadata(&path) {
                if !existing.file_type().is_socket() || existing.uid() != euid {
                    return Err("refusing to replace a non-socket or foreign path".into());
                }
                match UnixStream::connect(&path) {
                    Ok(_) => return Err("producer socket already has a live listener".into()),
                    Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => {
                        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
                    }
                    Err(e) => return Err(e.to_string()),
                }
            }
            let listener = UnixListener::bind(&path).map_err(|e| e.to_string())?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
            let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
            Ok(Self {
                listener,
                path,
                inode: meta.ino(),
                device: meta.dev(),
                profile,
                ops,
            })
        }
        /// Accept forever. Each connection has its own thread so a revoke is never
        /// queued behind another request that is waiting on a guest or SQL.
        pub(crate) fn serve_forever(self) {
            let active = Arc::new(AtomicUsize::new(0));
            loop {
                let stream = match self.listener.accept() {
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
                let (profile, ops, slots) =
                    (self.profile.clone(), self.ops.clone(), active.clone());
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
    }
    impl Drop for LocalListener {
        fn drop(&mut self) {
            if std::fs::symlink_metadata(&self.path)
                .is_ok_and(|m| m.ino() == self.inode && m.dev() == self.device)
            {
                let _ = std::fs::remove_file(&self.path);
            }
        }
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
            let dir = tempfile::tempdir().unwrap();
            let socket = dir.path().join("p").join("producer.sock");
            let mut profile = profile(uid);
            profile.socket = socket.clone();
            let listener = unix::LocalListener::bind(Arc::new(profile), Arc::new(Echo)).unwrap();
            std::thread::spawn(move || listener.serve_forever());
            (dir, socket)
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
