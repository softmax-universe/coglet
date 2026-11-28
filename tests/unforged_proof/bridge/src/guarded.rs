//! Guarded STANDARD transport: same synthetic step as `standard::execute`, but every
//! private-byte serialization and every network send first waits for an exact
//! operator approval (`{"approve":N}` on stdin). Synthetic-only; no gate authority.
//!
//! Protocol (stdout JSON lines, stdin JSON lines):
//!   {"phase":"authorize","sequence":N,"operation":"initial","endpoint":E,"recipe":{..}}
//!   {"phase":"authorize","sequence":N,"operation":"serialize","label":L}
//!   {"phase":"authorize","sequence":N,"operation":"request","method":M,"path":P,"body_sha256":H|null}
//! The reply for N must be exactly `{"approve":N}` (<= 4096 bytes, within 30s). Anything
//! else (EOF, malformed, wrong sequence, extra keys, timeout) refuses and exits.
//! Sequences start at 1 and increase by one per authorize event. Nothing is serialized
//! into wire form and no request is made before sequence 1 is approved.
use gecaos::caos::{
    CaosHttpResponse, CaosObjectKind, CaosRunResponse, CaosTreeEntry, decode_caos_tree,
    encode_caos_tree, is_caos_sha1_oid, parse_caos_object, serialize_caos_object,
    UNFORGED_CAOS_POOL, UNFORGED_CAOS_REQUIRED_POOL_ARG, CaosBaseSpec, CaosClient,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc;
use std::time::Duration;

pub const MAX_ACK: usize = 4096;
pub const ACK_TIMEOUT: Duration = Duration::from_secs(30);

type R<T> = Result<T, &'static str>;

pub fn hex(bytes: &[u8]) -> String { bytes.iter().map(|b| format!("{b:02x}")).collect() }
pub fn sha256_hex(bytes: &[u8]) -> String { hex(Sha256::digest(bytes).as_slice()) }

// ---------------------------------------------------------------- retained tree

/// Immutable bytes of the complete effective argument tree (including empty dirs).
pub enum Node { File { mode: &'static str, bytes: Vec<u8> }, Dir(BTreeMap<String, Node>) }

fn read_state(path: &Path, depth: usize, budget: &mut u64) -> R<Node> {
    if depth > 32 { return Err("synthetic state depth exceeded"); }
    let mut entries = BTreeMap::new();
    for entry in std::fs::read_dir(path).map_err(|_| "state directory unavailable")? {
        let entry = entry.map_err(|_| "state entry unavailable")?;
        let name = entry.file_name().into_string().map_err(|_| "non UTF-8 state path")?;
        if name.is_empty() || name == ".git" || name.contains(['/', '\\', '\0']) { return Err("invalid state path"); }
        let meta = entry.file_type().map_err(|_| "state metadata unavailable")?;
        let node = if meta.is_dir() {
            read_state(&entry.path(), depth + 1, budget)?
        } else if meta.is_file() {
            Node::File { mode: "100644", bytes: crate::standard::read_bounded(&entry.path(), budget)? }
        } else { return Err("nonregular state entry"); };
        entries.insert(name, node);
    }
    Ok(Node::Dir(entries))
}

/// Everything needed to run; built (local reads only) before the initial authorization.
pub struct Plan { pub args: Node, pub salt: String, pub image: String }

pub fn build_plan(image: &str, salt: &str, driver: &Path, state: &Path) -> R<Plan> {
    let executable = crate::standard::read_bounded(driver, &mut (256 * 1024 * 1024))?;
    if !executable.starts_with(b"\x7fELF") { return Err("native ELF driver required"); }
    let state = read_state(state, 0, &mut (64 * 1024 * 1024))?;
    let file = |mode, bytes: &[u8]| Node::File { mode, bytes: bytes.to_vec() };
    let mut args = BTreeMap::new();
    args.insert("worker1".to_string(), file("100755", crate::standard::WORKER.as_bytes()));
    args.insert("driver".to_string(), Node::File { mode: "100755", bytes: executable });
    args.insert("state".to_string(), state);
    args.insert("salt".to_string(), file("100644", salt.as_bytes()));
    args.insert("required-native-worker-image".to_string(), file("100644", image.as_bytes()));
    args.insert(UNFORGED_CAOS_REQUIRED_POOL_ARG.to_string(), file("100644", UNFORGED_CAOS_POOL.as_bytes()));
    Ok(Plan { args: Node::Dir(args), salt: salt.to_string(), image: image.to_string() })
}

fn collect(node: &Node, path: &str, out: &mut Vec<(String, Value)>) {
    match node {
        Node::File { mode, bytes } => out.push((path.to_string(), json!({"path":path,"mode":mode,
            "sha256":sha256_hex(bytes),"length":bytes.len()}))),
        Node::Dir(children) => {
            out.push((path.to_string(), json!({"path":path,"mode":"40000"})));
            for (name, child) in children { collect(child, &format!("{path}/{name}"), out); }
        }
    }
}

/// Recipe paths are rooted at the submitted `bundle` tree, whose only entry is `args`
/// (the effective argument tree). No separate root node: `args` is the dir node.
pub fn recipe(plan: &Plan) -> Value {
    let mut nodes = Vec::new();
    collect(&plan.args, "args", &mut nodes);
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
    let (base_type, base_value) = CaosBaseSpec::Unforged.base_type_and_value();
    json!({"version":1,"base":{"type":base_type,"value":base_value},"engine":null,
        "nodes":nodes.into_iter().map(|n| n.1).collect::<Vec<_>>(),
        "salt":plan.salt,"image":plan.image})
}

// ---------------------------------------------------------------- authority channel

pub trait Channel {
    fn emit(&mut self, event: &Value) -> R<()>;
    fn ack(&mut self) -> R<Vec<u8>>;
}

/// Real stdout/stdin. A reader thread bounds every wait with a timeout.
pub struct StdChannel { rx: mpsc::Receiver<R<Vec<u8>>>, timeout: Duration }

impl StdChannel {
    pub fn new(timeout: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                let mut line = Vec::new();
                let mut byte = [0u8; 1];
                loop {
                    match stdin.read(&mut byte) {
                        Ok(1) if byte[0] == b'\n' => break,
                        Ok(1) => {
                            line.push(byte[0]);
                            if line.len() > MAX_ACK { let _ = tx.send(Err("approval too large")); return; }
                        }
                        _ => { let _ = tx.send(Err("approval channel closed")); return; }
                    }
                }
                if tx.send(Ok(line)).is_err() { return; }
            }
        });
        Self { rx, timeout }
    }
}

impl Channel for StdChannel {
    fn emit(&mut self, event: &Value) -> R<()> {
        let mut out = std::io::stdout().lock();
        writeln!(out, "{event}").map_err(|_| "stdout unavailable")?;
        out.flush().map_err(|_| "stdout unavailable")
    }
    fn ack(&mut self) -> R<Vec<u8>> {
        self.rx.recv_timeout(self.timeout).map_err(|_| "approval timeout")?
    }
}

pub struct Guard<C: Channel> { pub chan: C, sequence: u64, pub serializations: usize, pub requests: usize }

impl<C: Channel> Guard<C> {
    pub fn new(chan: C) -> Self { Self { chan, sequence: 0, serializations: 0, requests: 0 } }

    fn authorize(&mut self, operation: &str, fields: Value) -> R<()> {
        self.sequence += 1;
        let mut event = Map::new();
        event.insert("phase".into(), json!("authorize"));
        event.insert("sequence".into(), json!(self.sequence));
        event.insert("operation".into(), json!(operation));
        if let Value::Object(extra) = fields { event.extend(extra); }
        self.chan.emit(&Value::Object(event))?;
        let raw = self.chan.ack()?;
        if raw.len() > MAX_ACK { return Err("approval too large"); }
        let value: Value = serde_json::from_slice(&raw).map_err(|_| "approval malformed")?;
        if value != json!({"approve": self.sequence}) { return Err("approval refused or sequence mismatch"); }
        Ok(())
    }

    pub fn initial(&mut self, endpoint: &str, recipe: Value) -> R<()> {
        self.authorize("initial", json!({"endpoint":endpoint,"recipe":recipe}))
    }

    /// Authorize, THEN serialize a blob wire object.
    fn blob(&mut self, label: &str, bytes: &[u8]) -> R<(String, Vec<u8>)> {
        self.authorize("serialize", json!({"label":label}))?;
        self.serializations += 1;
        Ok(serialize_caos_object(CaosObjectKind::Blob, bytes))
    }

    fn tree(&mut self, label: &str, entries: &[CaosTreeEntry]) -> R<(String, Vec<u8>)> {
        self.authorize("serialize", json!({"label":label}))?;
        self.serializations += 1;
        encode_caos_tree(entries).map_err(|_| "tree encoding failed")
    }

    /// Authorize, THEN send exactly one request.
    fn send(&mut self, net: &dyn Net, method: &str, path: &str, ct: Option<&str>, body: Option<&[u8]>)
        -> R<CaosHttpResponse> {
        self.authorize("request", json!({"method":method,"path":path,
            "body_sha256":body.map(sha256_hex)}))?;
        self.requests += 1;
        net.request(method, path, ct, body).map_err(|_| "request failed")
    }

    fn row(&mut self, row: Value) -> R<()> { self.chan.emit(&row) }

    fn put(&mut self, net: &dyn Net, oid: &str, wire: &[u8]) -> R<()> {
        let head = self.send(net, "HEAD", &format!("/object/{oid}"), None, None)?;
        match head.status { 200 => return Ok(()), 404 => {}, _ => return Err("unexpected HEAD status") }
        let resp = self.send(net, "POST", "/object/", Some("application/octet-stream"), Some(wire))?;
        if resp.status != 200 { return Err("object upload failed"); }
        if String::from_utf8_lossy(&resp.body).trim().to_ascii_lowercase() != oid { return Err("upload hash mismatch"); }
        Ok(())
    }

    fn get_object(&mut self, net: &dyn Net, oid: &str) -> R<(CaosObjectKind, Vec<u8>)> {
        let resp = self.send(net, "GET", &format!("/object/{oid}"), None, None)?;
        if resp.status != 200 { return Err("object unavailable"); }
        parse_caos_object(&resp.body, Some(oid)).map_err(|_| "object invalid")
    }

    fn upload(&mut self, net: &dyn Net, node: &Node, path: &str, ids: &mut BTreeMap<String, String>) -> R<String> {
        match node {
            Node::File { bytes, .. } => {
                let (oid, wire) = self.blob(&format!("blob:{path}"), bytes)?;
                self.put(net, &oid, &wire)?;
                ids.insert(path.to_string(), oid.clone());
                Ok(oid)
            }
            Node::Dir(children) => {
                let mut entries = Vec::new();
                for (name, child) in children {
                    let mode = match child { Node::File { mode, .. } => *mode, Node::Dir(_) => "40000" };
                    let oid = self.upload(net, child, &format!("{path}/{name}"), ids)?;
                    entries.push(CaosTreeEntry::new(mode, name, oid));
                }
                let (oid, wire) = self.tree(&format!("tree:{path}"), &entries)?;
                self.put(net, &oid, &wire)?;
                ids.insert(path.to_string(), oid.clone());
                Ok(oid)
            }
        }
    }
}

// ---------------------------------------------------------------- network seam

pub trait Net {
    fn request(&self, method: &str, path: &str, ct: Option<&str>, body: Option<&[u8]>)
        -> Result<CaosHttpResponse, String>;
    /// Exactly one `GET /run?req=..&nocache=1`.
    fn run_nocache(&self, req: &str) -> Result<CaosRunResponse, String>;
}

impl Net for CaosClient {
    fn request(&self, method: &str, path: &str, ct: Option<&str>, body: Option<&[u8]>)
        -> Result<CaosHttpResponse, String> { self.request_with_headers(method, path, ct, body) }
    fn run_nocache(&self, req: &str) -> Result<CaosRunResponse, String> { self.run_arg_tree_detailed(req, true) }
}

// ---------------------------------------------------------------- the step

fn read_tree<C: Channel>(g: &mut Guard<C>, net: &dyn Net, oid: &str, prefix: &str, depth: usize, budget: &mut u64) -> R<()> {
    if depth > 32 { return Err("output depth exceeded"); }
    let (kind, raw) = g.get_object(net, oid)?;
    if raw.len() as u64 > *budget { return Err("output budget exceeded"); }
    *budget -= raw.len() as u64;
    if kind != CaosObjectKind::Tree { return Err("output is not tree"); }
    g.row(json!({"phase":"abi-object","oid":oid,"kind":"tree","hex":hex(&raw)}))?;
    for entry in decode_caos_tree(&raw).map_err(|_| "output tree invalid")? {
        if entry.name.is_empty() || matches!(entry.name.as_str(), "."|".."|".git") || entry.name.contains(['/', '\\', '\0']) {
            return Err("invalid output path");
        }
        let path = format!("{prefix}{}", entry.name);
        if entry.mode == "40000" {
            read_tree(g, net, &entry.oid, &format!("{path}/"), depth + 1, budget)?;
        } else if entry.mode == "100644" {
            let (kind, raw) = g.get_object(net, &entry.oid)?;
            if kind != CaosObjectKind::Blob || raw.len() as u64 > *budget { return Err("invalid output blob"); }
            *budget -= raw.len() as u64;
            g.row(json!({"phase":"abi-object","oid":entry.oid,"kind":"blob","hex":hex(&raw)}))?;
            g.row(json!({"phase":"abi-file","path":path,"mode":entry.mode,"hex":hex(&raw)}))?;
        } else { return Err("nonregular ABI output"); }
    }
    Ok(())
}

fn read_status<C: Channel>(g: &mut Guard<C>, net: &dyn Net, req: &str) -> R<()> {
    for path in ["/version".to_string(), format!("/status/{req}"), format!("/status/{req}?all=1")] {
        let response = g.send(net, "GET", &path, None, None)?;
        g.row(json!({"phase":"readback","path":path,"http_status":response.status,
            "body":String::from_utf8_lossy(&response.body)}))?;
        if path.ends_with("?all=1") && response.status == 200 {
            let value: Value = serde_json::from_slice(&response.body).map_err(|_| "completed diagnostic status is not JSON")?;
            if value.get("arg_tree").and_then(|v| v.as_str()) == Some(req) {
                if let Some(traces) = value.get("out_trace").and_then(|v| v.as_array()) {
                    if traces.len() != 1 { return Err("readback sidecar ambiguous"); }
                    let oid = traces[0].as_str().ok_or("readback sidecar identity invalid")?;
                    if !is_caos_sha1_oid(oid) { return Err("readback sidecar identity invalid"); }
                    let (kind, payload) = g.get_object(net, oid)?;
                    if kind != CaosObjectKind::Blob { return Err("readback sidecar is not blob"); }
                    let raw = std::str::from_utf8(&payload).map_err(|_| "readback sidecar is not UTF-8")?;
                    g.row(json!({"phase":"sidecar-readback","request":req,"oid":oid,"raw_payload":raw}))?;
                }
            }
        }
    }
    Ok(())
}

pub fn run<C: Channel>(g: &mut Guard<C>, net: &dyn Net, plan: &Plan, endpoint: &str) -> R<()> {
    g.initial(endpoint, recipe(plan))?;
    let mut ids = BTreeMap::new();
    let args = g.upload(net, &plan.args, "args", &mut ids)?;
    let id = |p: &str| ids[p].clone();
    // Bundle = tree{args}; submit body mirrors CaosClient::submit_arg_tree for the native
    // Unforged base (pool is already explicit in args, so the library adds nothing).
    let (bundle, wire) = g.tree("tree:bundle", &[CaosTreeEntry::new("40000", "args", &args)])?;
    g.put(net, &bundle, &wire)?;
    let (base_type, base_val) = CaosBaseSpec::Unforged.base_type_and_value();
    let esc = |v: &str| serde_json::to_string(v).expect("string");
    let body = format!("{{\"base\":{{\"type\":{},\"value\":{}}},\"bundle\":{},\"salt\":{}}}",
        esc(base_type), esc(base_val), esc(&bundle), esc(&plan.salt));
    let resp = g.send(net, "POST", "/submit", Some("application/json"), Some(body.as_bytes()))?;
    if resp.status != 200 { return Err("submission failed"); }
    let text = String::from_utf8_lossy(&resp.body).to_string();
    let req = text.lines().next().unwrap_or("").trim().strip_prefix("tree ")
        .map(str::trim).filter(|r| is_caos_sha1_oid(r)).ok_or("malformed submit response")?.to_string();
    g.row(json!({"phase":"submitted","request":req,"salt":plan.salt,"args":args,"state":id("args/state"),
        "driver":id("args/driver"),"worker":id("args/worker1"),"expected_image":plan.image,
        "bundle":bundle,"pool":UNFORGED_CAOS_POOL}))?;
    g.authorize("request", json!({"method":"GET","path":format!("/run?req={req}&nocache=1"),"body_sha256":null}))?;
    g.requests += 1;
    let result = net.run_nocache(&req).map_err(|_| "strict recorded step failed")?;
    g.row(json!({"phase":"executed","request":req,"result_oid":result.oid,"cache_hit":result.cache_hit,
        "worker_image":result.worker_image,"server_confirmed_nocache":result.server_confirmed_nocache}))?;
    if result.cache_hit || !result.server_confirmed_nocache || result.worker_image.as_deref() != Some(plan.image.as_str()) {
        return Err("fresh execution/image mismatch");
    }
    read_status(g, net, &req)?;
    read_tree(g, net, &result.oid, "", 0, &mut (64 * 1024 * 1024))
}

pub fn execute(client: &CaosClient, endpoint: &str, image: &str, salt: &str, driver: &Path, state: &Path) -> R<()> {
    let plan = build_plan(image, salt, driver, state)?;
    let mut guard = Guard::new(StdChannel::new(ACK_TIMEOUT));
    run(&mut guard, client, &plan, endpoint)
}

#[cfg(test)]
#[path = "guarded_tests.rs"]
mod tests;
