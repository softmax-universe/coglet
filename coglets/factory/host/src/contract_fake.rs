//! Contract-conformant test double for the recorded Hermes Board wire schema.
//! Templates come from real HTTP capture, not from transport response structs.
//! No production endpoint should ever use this in-process fixture.
use crate::HttpKanban;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

const BOARD: &str = "coglet-factory-t01a20f44e61e630a";
const RECORDING: &str = include_str!("../tests/fixtures/board-contract-recording.json");

pub struct ContractKanbanServer {
    base: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
#[derive(Default)]
struct State {
    tasks: BTreeMap<String, Value>,
    runs: BTreeMap<String, Value>,
    ops: BTreeMap<String, Value>,
    lease_ops: BTreeMap<String, Value>,
    requests: Vec<Value>,
    lose_create_reply: bool,
    lose_grant_reply: bool,
    unknown_run: bool,
}
fn template(label: &str, pointer: &str) -> Value {
    let capture: Value = serde_json::from_str(RECORDING).unwrap();
    capture["records"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["label"] == label && r["response"]["status"] == 200)
        .unwrap()["response"]["body"]
        .pointer(pointer)
        .unwrap()
        .clone()
}
impl ContractKanbanServer {
    pub fn spawn() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = Arc::new(Mutex::new(State::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (shared, shutdown) = (state.clone(), stop.clone());
        let thread = thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let _ = handle(stream, &shared);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2))
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base,
            state,
            stop,
            thread: Some(thread),
        }
    }
    pub fn base(&self) -> &str {
        &self.base
    }
    pub fn adapter(&self) -> HttpKanban {
        HttpKanban::authenticated(
            &self.base,
            BOARD,
            "fixture-bearer",
            "worker-a1",
            "factory-contract-fixture",
        )
        .unwrap()
    }
    pub fn len(&self) -> usize {
        self.state.lock().unwrap().tasks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn lose_next_create_reply(&self) {
        self.state.lock().unwrap().lose_create_reply = true;
    }
    pub fn requests(&self) -> Vec<Value> {
        self.state.lock().unwrap().requests.clone()
    }
    pub fn lose_next_grant_reply(&self) {
        self.state.lock().unwrap().lose_grant_reply = true;
    }
    pub fn expire_lease(&self, remote: &str) {
        self.state.lock().unwrap().tasks.get_mut(remote).unwrap()["lease_expires"] = json!(1);
    }
    pub fn unknown_run_evidence(&self, unknown: bool) {
        self.state.lock().unwrap().unknown_run = unknown;
    }
    pub fn change_run_holder(&self, remote: &str) {
        let mut state = self.state.lock().unwrap();
        let run = state.tasks[remote]["current_run_uid"]
            .as_str()
            .unwrap()
            .to_owned();
        state.runs.get_mut(&run).unwrap()["origin_gateway"] = json!("another-worker");
    }
    pub fn post_result(&self, remote: &str, succeeded: bool, note: &str) {
        let mut state = self.state.lock().unwrap();
        let task = state.tasks.get_mut(remote).expect("remote");
        let run = task["current_run_uid"].as_str().unwrap().to_owned();
        task["status"] = json!(if succeeded { "done" } else { "ready" });
        task["current_run_uid"] = Value::Null;
        task["lease_run_uid"] = Value::Null;
        task["lease_holder"] = Value::Null;
        task["lease_expires"] = Value::Null;
        let row = state.runs.get_mut(&run).unwrap();
        row["status"] = json!(if succeeded { "done" } else { "crashed" });
        row["outcome"] = json!(if succeeded { "completed" } else { "crashed" });
        row["ended_at"] = json!(1795557397);
        row["summary"] = json!(note);
    }
}
impl Drop for ContractKanbanServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}
fn handle(mut stream: TcpStream, shared: &Mutex<State>) -> std::io::Result<()> {
    // The listener is non-blocking and macOS hands that flag to the accepted
    // socket: a request not yet in the buffer would fail the first read and the
    // connection would be dropped unanswered.
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(3)))?;
    let mut raw = Vec::new();
    let end;
    loop {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&chunk[..n]);
        if let Some(i) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
            end = i + 4;
            break;
        }
        if raw.len() > 65536 {
            return Ok(());
        }
    }
    let headers = String::from_utf8_lossy(&raw[..end]).to_string();
    let size = headers
        .lines()
        .find_map(|l| {
            l.to_ascii_lowercase()
                .strip_prefix("content-length:")?
                .trim()
                .parse::<usize>()
                .ok()
        })
        .unwrap_or(0);
    if size > 65536 {
        return Ok(());
    }
    while raw.len() < end + size {
        let mut chunk = [0; 4096];
        let n = stream.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        raw.extend_from_slice(&chunk[..n]);
    }
    let mut request = headers.lines().next().unwrap().split_whitespace();
    let method = request.next().unwrap();
    let path = request.next().unwrap();
    let body: Value = if size == 0 {
        Value::Null
    } else {
        serde_json::from_slice(&raw[end..end + size]).unwrap()
    };
    let authorized = headers
        .lines()
        .any(|l| l.eq_ignore_ascii_case("authorization: Bearer fixture-bearer"));
    let mut state = shared.lock().unwrap();
    state
        .requests
        .push(json!({"method":method,"path":path,"body":body}));
    let response = if authorized {
        route(&mut state, method, path, &body)
    } else {
        json!({"error":"unauthorized"})
    };
    if state.lose_create_reply && body["ops"][0]["op"] == "create" {
        state.lose_create_reply = false;
        return Ok(());
    }
    if state.lose_grant_reply && body["action"] == "grant" {
        state.lose_grant_reply = false;
        return Ok(());
    }
    let body = response.to_string();
    write!(
        stream,
        "HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        if authorized { 200 } else { 401 },
        body.len()
    )
}
fn rejected(reason: &str) -> Value {
    json!({"status":"rejected","reason":reason})
}
fn route(state: &mut State, method: &str, path: &str, body: &Value) -> Value {
    let prefix = format!("/v2/boards/{BOARD}");
    let Some(suffix) = path.strip_prefix(&prefix) else {
        return json!({"error":"unknown route"});
    };
    if method == "GET" && suffix.starts_with("/evidence?") {
        let url = reqwest::Url::parse(&format!("http://fixture{suffix}")).unwrap();
        let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        let table = params.get("table").unwrap();
        let uid = params.get("uid").unwrap();
        if table == "task_runs" && state.unknown_run {
            return json!({"board":BOARD,"table":table,"uid":uid,"state":"unknown","row":null});
        }
        let row = match table.as_str() {
            "tasks" => state.tasks.get(uid),
            "task_runs" => state.runs.get(uid),
            _ => None,
        };
        return json!({"board":BOARD,"table":table,"uid":uid,"state":if row.is_some() {"live"} else {"never_existed"},
            "row":row,"tombstone":null,"floor":0,"feed_floor":0,"head":1});
    }
    if method == "POST" && suffix == "/ops" {
        let ops = body["ops"].as_array().expect("ops array per Board schema");
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        let id = op["client_op_id"].as_str().unwrap();
        if let Some(prior) = state.ops.get(id) {
            return prior.clone();
        }
        let remote = op["task"].as_str().unwrap();
        let mut result = match op["op"].as_str().unwrap() {
            "create" => {
                assert!(op["fields"]["title"].is_string());
                assert!(
                    op.get("table").is_none(),
                    "semantic create, not a table upsert"
                );
                if state.tasks.contains_key(remote) {
                    rejected("id_taken")
                } else {
                    let mut row = template("create", "/results/0/row");
                    row["uid"] = json!(remote);
                    row["id"] = json!(remote);
                    for (key, value) in op["fields"].as_object().unwrap() {
                        row[key] = value.clone();
                    }
                    row["status"] = json!("ready");
                    state.tasks.insert(remote.into(), row.clone());
                    json!({"status":"applied","row":row})
                }
            }
            "comment" if state.tasks.contains_key(remote) => {
                assert!(op["body"].is_string() && op["author"].is_string());
                json!({"status":"applied","row":state.tasks[remote]})
            }
            _ => rejected("unsupported_operation"),
        };
        result["client_op_id"] = json!(id);
        let response = json!({"results":[result],"head":1});
        state.ops.insert(id.into(), response.clone());
        return response;
    }
    if method == "POST" && suffix == "/leases" {
        let remote = body["task"].as_str().unwrap();
        let run = body["run_uid"].as_str().unwrap();
        assert!(body["ttl_s"].is_number() && body["client_op_id"].is_string());
        let Some(row) = state.tasks.get_mut(remote) else {
            return rejected("not_found");
        };
        let action = body["action"].as_str().unwrap();
        let id = body["client_op_id"].as_str().unwrap();
        // Pinned board.ts:887-914: replay the cached verdict/row but
        // attach the CURRENT lease, including null after completion.
        if let Some(prior) = state.lease_ops.get(id) {
            if prior["task"] != remote || prior["action"] != action {
                return rejected("op_id_taken");
            }
            let mut response = prior.clone();
            response["lease"] = current_lease(row);
            return response;
        }
        if (action == "grant" && row["status"] != "ready" && row["current_run_uid"] != run)
            || (action == "renew" && row["current_run_uid"] != run)
        {
            return rejected("lease_lost");
        }
        let mut lease = template("claim", "/lease");
        lease["task_id"] = json!(remote);
        lease["run_uid"] = json!(run);
        lease["expires"] = json!(9_000_000_000_000u64);
        row["current_run_uid"] = json!(run);
        row["lease_holder"] = json!("worker-a1");
        row["lease_run_uid"] = json!(run);
        row["lease_expires"] = lease["expires"].clone();
        row["status"] = json!("running");
        if !state.runs.contains_key(run) {
            let mut r = template("run-readback", "/row");
            r["uid"] = json!(run);
            r["task_id"] = json!(remote);
            r["status"] = json!("running");
            r["outcome"] = Value::Null;
            r["ended_at"] = Value::Null;
            state.runs.insert(run.into(), r);
        }
        let response = json!({"status":"applied","client_op_id":body["client_op_id"],"task":remote,"action":action,"row":row,"lease":lease});
        state.lease_ops.insert(id.into(), response.clone());
        return response;
    }
    json!({"error":"unknown route"})
}

fn current_lease(row: &Value) -> Value {
    if row["lease_holder"].is_null() {
        return Value::Null;
    }
    let mut lease = template("claim", "/lease");
    lease["task_id"] = row["uid"].clone();
    lease["run_uid"] = row["lease_run_uid"].clone();
    lease["holder"] = row["lease_holder"].clone();
    lease["expires"] = row["lease_expires"].clone();
    lease
}
