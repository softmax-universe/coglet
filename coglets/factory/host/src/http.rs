//! Public Kanban HTTP transport. Paths and JSON are the adapter contract;
//! nothing here opens a Hermes database.
use crate::adapter::{
    AdapterError, HermesAdapter, Lease, Outcome, RemoteTask, TaskSpec, WorkerReport,
};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub struct HttpKanban {
    base: String,
}

impl HttpKanban {
    pub fn new(base: impl Into<String>) -> Self {
        Self { base: base.into() }
    }
}

impl HermesAdapter for HttpKanban {
    fn create(&mut self, spec: &TaskSpec) -> Result<RemoteTask, AdapterError> {
        let body = format!(
            "{{\"idempotency_key\":{key},\"kind\":{kind},\"payload_cbor_hex\":{payload},\"correlation_id\":{corr},\"task_id\":{task}}}",
            key = json_str(&spec.idempotency_key),
            kind = json_str(&spec.kind),
            payload = json_str(&spec.payload_cbor_hex),
            corr = json_str(&spec.correlation_id),
            task = json_str(&spec.task_id),
        );
        let (status, text) = call(&self.base, "POST", "/v1/tasks", Some(&body))?;
        if status == 409 {
            return Err(AdapterError::Conflict(text));
        }
        if status >= 500 {
            return Err(AdapterError::Uncertain);
        }
        if !(200..300).contains(&status) {
            return Err(AdapterError::Failed(text));
        }
        Ok(RemoteTask {
            remote_id: field(&text, "remote_id").ok_or(AdapterError::Uncertain)?,
            idempotency_key: field(&text, "idempotency_key")
                .unwrap_or_else(|| spec.idempotency_key.clone()),
        })
    }

    fn claim(
        &mut self,
        remote_id: &str,
        owner: &str,
        until: u64,
        attempt: u64,
    ) -> Result<Lease, AdapterError> {
        let body = format!(
            "{{\"owner\":{owner},\"until\":{until},\"attempt\":{attempt}}}",
            owner = json_str(owner),
        );
        let (status, text) = call(
            &self.base,
            "POST",
            &format!("/v1/tasks/{remote_id}/claim"),
            Some(&body),
        )?;
        map_status(status, &text)?;
        Ok(Lease {
            remote_id: remote_id.into(),
            owner: owner.into(),
            until,
            attempt,
            token: field(&text, "lease").ok_or(AdapterError::Uncertain)?,
        })
    }

    fn heartbeat(
        &mut self,
        remote_id: &str,
        until: u64,
        attempt: u64,
        lease: &str,
    ) -> Result<(), AdapterError> {
        let body = format!(
            "{{\"until\":{until},\"attempt\":{attempt},\"lease\":{lease}}}",
            lease = json_str(lease),
        );
        let (status, text) = call(
            &self.base,
            "POST",
            &format!("/v1/tasks/{remote_id}/heartbeat"),
            Some(&body),
        )?;
        map_status(status, &text)
    }

    fn retry(&mut self, remote_id: &str) -> Result<(), AdapterError> {
        let (status, text) = call(
            &self.base,
            "POST",
            &format!("/v1/tasks/{remote_id}/retry"),
            Some("{}"),
        )?;
        map_status(status, &text)
    }

    fn cancel(&mut self, remote_id: &str, attempt: u64, lease: &str) -> Result<(), AdapterError> {
        let body = format!(
            "{{\"attempt\":{attempt},\"lease\":{lease}}}",
            lease = json_str(lease),
        );
        let (status, text) = call(
            &self.base,
            "POST",
            &format!("/v1/tasks/{remote_id}/cancel"),
            Some(&body),
        )?;
        map_status(status, &text)
    }

    fn progress(
        &mut self,
        remote_id: &str,
        note: &str,
        attempt: u64,
        lease: &str,
    ) -> Result<(), AdapterError> {
        let body = format!(
            "{{\"note\":{note},\"attempt\":{attempt},\"lease\":{lease}}}",
            note = json_str(note),
            lease = json_str(lease),
        );
        let (status, text) = call(
            &self.base,
            "POST",
            &format!("/v1/tasks/{remote_id}/progress"),
            Some(&body),
        )?;
        map_status(status, &text)
    }

    fn poll(&mut self, remote_id: &str) -> Result<Option<WorkerReport>, AdapterError> {
        let (status, text) = call(
            &self.base,
            "GET",
            &format!("/v1/tasks/{remote_id}/result"),
            None,
        )?;
        map_status(status, &text)?;
        if field(&text, "status").as_deref() == Some("pending") {
            return Ok(None);
        }
        let succeeded = field(&text, "status").as_deref() == Some("succeeded");
        let note = field(&text, "note").unwrap_or_default();
        let outcome = if succeeded {
            Outcome::Succeeded { note }
        } else {
            Outcome::Failed { note }
        };
        Ok(Some(WorkerReport {
            attempt: number(&text, "attempt").ok_or(AdapterError::Uncertain)?,
            lease: field(&text, "lease").ok_or(AdapterError::Uncertain)?,
            outcome,
        }))
    }
}

/// In-process stand-in for the public Kanban API. Tests and local fakes use it;
/// production points [`HttpKanban`] at the real scheduler.
pub struct MockKanbanServer {
    base: String,
    state: Arc<Mutex<Board>>,
    shutdown: Arc<Mutex<bool>>,
    thread: Option<JoinHandle<()>>,
}

struct Board {
    tasks: Vec<Remote>,
}

struct Remote {
    idempotency_key: String,
    remote_id: String,
    lease: String,
    attempt: u64,
    owner: String,
    until: u64,
    cancelled: bool,
    progress: Vec<String>,
    heartbeats: u32,
    retries: u32,
    result: Option<(bool, String, u64, String)>,
}

impl MockKanbanServer {
    pub fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock kanban");
        let port = listener.local_addr().expect("addr").port();
        let state = Arc::new(Mutex::new(Board { tasks: vec![] }));
        let shutdown = Arc::new(Mutex::new(false));
        let shared = Arc::clone(&state);
        let stop = Arc::clone(&shutdown);
        listener.set_nonblocking(true).expect("nonblocking");
        let thread = thread::spawn(move || {
            while !*stop.lock().expect("stop") {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle(stream, &shared);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base: format!("http://127.0.0.1:{port}"),
            state,
            shutdown,
            thread: Some(thread),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn post_result(&self, remote_id: &str, succeeded: bool, note: &str) {
        let mut board = self.state.lock().expect("board");
        let task = board
            .tasks
            .iter_mut()
            .find(|t| t.remote_id == remote_id)
            .expect("remote");
        task.result = Some((succeeded, note.into(), task.attempt, task.lease.clone()));
    }

    pub fn len(&self) -> usize {
        self.state.lock().expect("board").tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for MockKanbanServer {
    fn drop(&mut self) {
        *self.shutdown.lock().expect("stop") = true;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn handle(mut stream: TcpStream, state: &Mutex<Board>) -> std::io::Result<()> {
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    let mut buf = [0u8; 8192];
    let n = stream.read(&mut buf)?;
    let text = String::from_utf8_lossy(&buf[..n]);
    let mut lines = text.lines();
    let request = lines.next().unwrap_or("");
    let mut parts = request.split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("");
    let body = text.split("\r\n\r\n").nth(1).unwrap_or("");
    let response = route(method, path, body, state);
    stream.write_all(response.as_bytes())?;
    Ok(())
}

fn route(method: &str, path: &str, body: &str, state: &Mutex<Board>) -> String {
    let mut board = state.lock().expect("board");
    if method == "POST" && path == "/v1/tasks" {
        let key = field(body, "idempotency_key").unwrap_or_default();
        if let Some(existing) = board.tasks.iter().find(|t| t.idempotency_key == key) {
            return ok(&format!(
                "{{\"remote_id\":{id},\"idempotency_key\":{key}}}",
                id = json_str(&existing.remote_id),
                key = json_str(&existing.idempotency_key),
            ));
        }
        let remote_id = format!("k{key}");
        board.tasks.push(Remote {
            idempotency_key: key.clone(),
            remote_id: remote_id.clone(),
            lease: String::new(),
            attempt: 0,
            owner: String::new(),
            until: 0,
            cancelled: false,
            progress: vec![],
            heartbeats: 0,
            retries: 0,
            result: None,
        });
        return created(&format!(
            "{{\"remote_id\":{id},\"idempotency_key\":{key}}}",
            id = json_str(&remote_id),
            key = json_str(&key),
        ));
    }
    let Some(remote_id) = path
        .strip_prefix("/v1/tasks/")
        .and_then(|rest| rest.split('/').next())
    else {
        return status(404, "{\"error\":\"missing\"}");
    };
    let Some(task) = board.tasks.iter_mut().find(|t| t.remote_id == remote_id) else {
        return status(404, "{\"error\":\"unknown\"}");
    };
    let action = path.rsplit('/').next().unwrap_or("");
    match (method, action) {
        ("POST", "claim") => {
            if task.cancelled {
                return status(409, "{\"error\":\"cancelled\"}");
            }
            let attempt = number(body, "attempt").unwrap_or(0);
            let owner = field(body, "owner").unwrap_or_default();
            let until = number(body, "until").unwrap_or(0);
            task.attempt = attempt;
            task.owner = owner;
            task.until = until;
            task.lease = format!("lease:{attempt}:{remote_id}");
            ok(&format!(
                "{{\"remote_id\":{id},\"lease\":{lease},\"until\":{until}}}",
                id = json_str(remote_id),
                lease = json_str(&task.lease),
            ))
        }
        ("POST", "heartbeat") => {
            if task.lease != field(body, "lease").unwrap_or_default()
                || task.attempt != number(body, "attempt").unwrap_or(0)
            {
                return status(409, "{\"error\":\"stale\"}");
            }
            task.until = number(body, "until").unwrap_or(task.until);
            task.heartbeats += 1;
            ok("{}")
        }
        ("POST", "retry") => {
            task.lease.clear();
            task.retries += 1;
            ok("{}")
        }
        ("POST", "cancel") => {
            if task.lease != field(body, "lease").unwrap_or_default() {
                return status(409, "{\"error\":\"stale\"}");
            }
            task.cancelled = true;
            ok("{}")
        }
        ("POST", "progress") => {
            if task.lease != field(body, "lease").unwrap_or_default() {
                return status(409, "{\"error\":\"stale\"}");
            }
            task.progress.push(field(body, "note").unwrap_or_default());
            ok("{}")
        }
        ("GET", "result") => {
            if let Some((succeeded, note, attempt, lease)) = &task.result {
                let status_name = if *succeeded { "succeeded" } else { "failed" };
                ok(&format!(
                    "{{\"status\":{status},\"note\":{note},\"attempt\":{attempt},\"lease\":{lease}}}",
                    status = json_str(status_name),
                    note = json_str(note),
                    lease = json_str(lease),
                ))
            } else {
                ok("{\"status\":\"pending\"}")
            }
        }
        _ => status(404, "{\"error\":\"route\"}"),
    }
}

fn call(
    base: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<(u16, String), AdapterError> {
    let host = base.trim_start_matches("http://");
    let mut stream = TcpStream::connect(host).map_err(|_| AdapterError::Uncertain)?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|_| AdapterError::Uncertain)?;
    let body = body.unwrap_or("");
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|_| AdapterError::Uncertain)?;
    let mut raw = Vec::new();
    stream
        .read_to_end(&mut raw)
        .map_err(|_| AdapterError::Uncertain)?;
    let text = String::from_utf8_lossy(&raw);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let payload = text.split("\r\n\r\n").nth(1).unwrap_or("").to_owned();
    Ok((status, payload))
}

fn map_status(status: u16, text: &str) -> Result<(), AdapterError> {
    if status == 409 {
        return Err(AdapterError::Conflict(text.into()));
    }
    if status >= 500 || status == 0 {
        return Err(AdapterError::Uncertain);
    }
    if !(200..300).contains(&status) {
        return Err(AdapterError::Failed(text.into()));
    }
    Ok(())
}

fn ok(body: &str) -> String {
    status(200, body)
}
fn created(body: &str) -> String {
    status(201, body)
}
fn status(code: u16, body: &str) -> String {
    format!(
        "HTTP/1.1 {code} OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}
fn json_str(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn field(raw: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\":\"");
    let start = raw.find(&needle)? + needle.len();
    let rest = &raw[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_owned())
}
fn number(raw: &str, key: &str) -> Option<u64> {
    let needle = format!("\"{key}\":");
    let start = raw.find(&needle)? + needle.len();
    let rest = raw[start..].trim_start();
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok()
}
