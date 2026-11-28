//! Unit tests for guarded.rs (fake channel and fake server; no network, no hosted calls).
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

type Log = Rc<RefCell<Vec<String>>>;
type Events = Rc<RefCell<Vec<Value>>>;
#[test]
#[ignore = "explicit cross_language_checks.py contract consumer"]
fn cross_language_contract_fixture() {
    let output = std::env::var("COGLET_CROSS_LANGUAGE_OUTPUT").expect("explicit output required");
    let (root, _) = fixture();
    let salt = "c".repeat(64);
    let endpoint = "d".repeat(64);
    let plan = build_plan(IMAGE, &salt, &root.join("driver"), &root.join("state")).unwrap();
    let log: Log = Default::default();
    let net = FakeNet::new(&log);
    let mut transcripts = Vec::new();
    for _ in 0..2 {
        let events: Events = Default::default();
        let mut g = Guard::new(Chan::new(&log, &events, usize::MAX));
        run(&mut g, &net, &plan, &endpoint).unwrap();
        transcripts.push(events.borrow().clone());
    }
    std::fs::write(output, serde_json::to_vec(&transcripts).unwrap()).unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
const IMAGE: &str = "native@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

struct Chan { log: Log, events: Events, approvals: usize, seen: usize, raw: Option<Vec<u8>> }
impl Chan {
    fn new(log: &Log, events: &Events, approvals: usize) -> Self {
        Self { log: log.clone(), events: events.clone(), approvals, seen: 0, raw: None }
    }
}
impl Channel for Chan {
    fn emit(&mut self, event: &Value) -> R<()> {
        if event["phase"] == "authorize" { self.log.borrow_mut().push("authorize".into()); }
        self.events.borrow_mut().push(event.clone());
        Ok(())
    }
    fn ack(&mut self) -> R<Vec<u8>> {
        if let Some(raw) = &self.raw { return Ok(raw.clone()); }
        if self.seen >= self.approvals { return Err("approval channel closed"); }
        self.seen += 1;
        let seq = self.events.borrow().iter().filter(|e| e["phase"] == "authorize").count();
        self.log.borrow_mut().push("approve".into());
        Ok(format!("{{\"approve\":{seq}}}").into_bytes())
    }
}

struct FakeNet { log: Log, objects: RefCell<HashMap<String, Vec<u8>>>, out: String, sidecar: String,
    posts: RefCell<Vec<(String, Vec<u8>)>>, req: RefCell<String> }
impl FakeNet {
    fn new(log: &Log) -> Self {
        let mut objects = HashMap::new();
        let (sidecar, w) = serialize_caos_object(CaosObjectKind::Blob, b"{\"ok\":1}");
        objects.insert(sidecar.clone(), w);
        let (file, w) = serialize_caos_object(CaosObjectKind::Blob, b"result");
        objects.insert(file.clone(), w);
        let (out, w) = encode_caos_tree(&[CaosTreeEntry::new("100644", "result.txt", file)]).unwrap();
        objects.insert(out.clone(), w);
        Self { log: log.clone(), objects: RefCell::new(objects), out, sidecar,
            posts: RefCell::new(vec![]), req: RefCell::new(String::new()) }
    }
    fn resp(status: u16, body: impl Into<Vec<u8>>) -> Result<CaosHttpResponse, String> {
        Ok(CaosHttpResponse { status, headers: vec![], body: body.into() })
    }
}
impl Net for FakeNet {
    fn request(&self, method: &str, path: &str, _ct: Option<&str>, body: Option<&[u8]>)
        -> Result<CaosHttpResponse, String> {
        self.log.borrow_mut().push(format!("net:{method} {path}"));
        if method == "POST" { self.posts.borrow_mut().push((path.into(), body.unwrap().to_vec())); }
        match (method, path) {
            ("HEAD", p) => Self::resp(if self.objects.borrow().contains_key(&p[8..]) { 200 } else { 404 }, ""),
            ("POST", "/object/") => {
                let (kind, payload) = parse_caos_object(body.unwrap(), None)?;
                let oid = serialize_caos_object(kind, &payload).0;
                self.objects.borrow_mut().insert(oid.clone(), body.unwrap().to_vec());
                Self::resp(200, oid)
            }
            ("POST", "/submit") => {
                let oid = serialize_caos_object(CaosObjectKind::Blob, body.unwrap()).0;
                *self.req.borrow_mut() = oid.clone();
                Self::resp(200, format!("tree {oid}\n"))
            }
            ("GET", "/version") => Self::resp(200, "v"),
            ("GET", p) if p.starts_with("/status/") && p.ends_with("?all=1") => Self::resp(200,
                json!({"arg_tree":*self.req.borrow(),"out_trace":[self.sidecar]}).to_string()),
            ("GET", p) if p.starts_with("/status/") => Self::resp(200, "{}"),
            ("GET", p) if p.starts_with("/object/") => match self.objects.borrow().get(&p[8..]) {
                Some(w) => Self::resp(200, w.clone()),
                None => Self::resp(404, ""),
            },
            _ => Err("unexpected".into()),
        }
    }
    fn run_nocache(&self, req: &str) -> Result<CaosRunResponse, String> {
        self.log.borrow_mut().push(format!("net:RUN {req}"));
        Ok(CaosRunResponse { kind: CaosObjectKind::Tree, oid: self.out.clone(), cache_hit: false,
            worker_image: Some(IMAGE.into()), server_confirmed_nocache: true, server_confirmed_cache: true })
    }
}

fn fixture() -> (std::path::PathBuf, Plan) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static N: AtomicUsize = AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!("guarded-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let state = root.join("state");
    std::fs::create_dir_all(state.join("empty/nested")).unwrap();
    std::fs::create_dir_all(state.join("sub")).unwrap();
    std::fs::write(state.join("sub/a.txt"), b"alpha").unwrap();
    std::fs::write(state.join("b.txt"), b"").unwrap();
    std::fs::write(root.join("driver"), b"\x7fELFdriver").unwrap();
    let plan = build_plan(IMAGE, "salt1", &root.join("driver"), &state).unwrap();
    (root, plan)
}

fn drive(plan: &Plan, approvals: usize) -> (R<()>, Log, Events, FakeNet, usize, usize) {
    let log: Log = Default::default();
    let events: Events = Default::default();
    let net = FakeNet::new(&log);
    let mut g = Guard::new(Chan::new(&log, &events, approvals));
    let r = run(&mut g, &net, plan, "ee");
    let (s, q) = (g.serializations, g.requests);
    (r, log, events, net, s, q)
}

#[test]
fn recipe_has_exact_schema_sorted_nodes_and_empty_dirs() {
    let (root, plan) = fixture();
    let r = recipe(&plan);
    let mut keys: Vec<_> = r.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["base", "engine", "image", "nodes", "salt", "version"]);
    assert_eq!(r["version"], 1);
    assert_eq!(r["base"], json!({"type":"docker","value":gecaos::caos::UNFORGED_CAOS_IMAGE_REF}));
    assert!(r["engine"].is_null());
    let nodes = r["nodes"].as_array().unwrap();
    let paths: Vec<&str> = nodes.iter().map(|n| n["path"].as_str().unwrap()).collect();
    let mut sorted = paths.clone();
    sorted.sort();
    assert_eq!(paths, sorted);
    let by = |p: &str| nodes.iter().find(|n| n["path"] == p).unwrap_or_else(|| panic!("{p}"));
    assert_eq!(by("args"), &json!({"path":"args","mode":"40000"}));
    assert_eq!(by("args/state/empty/nested"), &json!({"path":"args/state/empty/nested","mode":"40000"}));
    assert_eq!(by("args/driver")["mode"], "100755");
    assert_eq!(by("args/driver")["sha256"], sha256_hex(b"\x7fELFdriver"));
    assert_eq!(by("args/worker1")["sha256"], sha256_hex(crate::standard::WORKER.as_bytes()));
    assert_eq!(by("args/required-pool")["sha256"], sha256_hex(b"unforged"));
    assert_eq!(by("args/state/b.txt")["length"], 0);
    assert_eq!(by("args/state/sub/a.txt"), &json!({"path":"args/state/sub/a.txt","mode":"100644",
        "sha256":sha256_hex(b"alpha"),"length":5}));
    assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn initial_denial_precedes_any_serialization_or_send() {
    let (root, plan) = fixture();
    let (r, log, events, net, s, q) = drive(&plan, 0);
    assert!(r.is_err());
    assert_eq!((s, q), (0, 0));
    assert!(net.objects.borrow().len() == 3 && net.posts.borrow().is_empty());
    assert!(!log.borrow().iter().any(|l| l.starts_with("net:")));
    let ev = events.borrow();
    assert_eq!(ev.len(), 1);
    assert_eq!((ev[0]["operation"].as_str(), ev[0]["sequence"].as_u64()), (Some("initial"), Some(1)));
    assert_eq!(ev[0]["recipe"], recipe(&plan));
    assert_eq!(ev[0]["endpoint"], "ee");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_approval_preserves_rows_and_binds_recipe_only_to_initial() {
    let (root, plan) = fixture();
    let (r, _, events, net, _, _) = drive(&plan, usize::MAX);
    r.unwrap();
    let ev = events.borrow();
    let auth: Vec<_> = ev.iter().filter(|e| e["phase"] == "authorize").collect();
    for (i, e) in auth.iter().enumerate() {
        assert_eq!(e["sequence"].as_u64(), Some(i as u64 + 1));
        assert_eq!(e.get("recipe").is_some(), i == 0);
    }
    let one = |p: &str| {
        let f: Vec<_> = ev.iter().filter(|e| e["phase"] == p).collect();
        assert_eq!(f.len(), 1, "{p}");
        f[0].clone()
    };
    assert_eq!(one("submitted")["expected_image"], IMAGE);
    assert_eq!(one("executed")["result_oid"], net.out);
    one("sidecar-readback");
    assert_eq!(one("abi-file")["path"], "result.txt");
    // submit body: native base, bundle containing only args.
    let posts = net.posts.borrow();
    let (_, raw_body) = posts.iter().find(|p| p.0 == "/submit").unwrap();
    let body: Value = serde_json::from_slice(raw_body).unwrap();
    assert_eq!(body["base"]["type"], "docker");
    assert_eq!(body["salt"], "salt1");
    let wire = net.objects.borrow()[body["bundle"].as_str().unwrap()].clone();
    let entries = decode_caos_tree(&parse_caos_object(&wire, None).unwrap().1).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!((entries[0].name.as_str(), entries[0].mode.as_str()), ("args", "40000"));
    let post = auth.iter().find(|e| e["method"] == "POST" && e["path"] == "/submit").unwrap();
    assert_eq!(post["body_sha256"], sha256_hex(raw_body));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn revocation_before_every_serialization_and_send_stops_all_work() {
    let (root, plan) = fixture();
    let (r, _, events, _, _, _) = drive(&plan, usize::MAX);
    r.unwrap();
    let total = events.borrow().iter().filter(|e| e["phase"] == "authorize").count();
    assert!(total > 20);
    for n in 0..total {
        let (r, log, events, net, s, q) = drive(&plan, n);
        assert!(r.is_err(), "approvals={n}");
        let ev = events.borrow();
        let auth: Vec<_> = ev.iter().filter(|e| e["phase"] == "authorize").collect();
        assert_eq!(auth.len(), n + 1, "denied event is the last emitted authorization");
        let approved = &auth[..n];
        assert_eq!(s, approved.iter().filter(|e| e["operation"] == "serialize").count());
        assert_eq!(q, approved.iter().filter(|e| e["operation"] == "request").count());
        let log = log.borrow();
        assert_eq!(log.iter().filter(|l| l.starts_with("net:")).count(), q);
        for (i, l) in log.iter().enumerate() {
            if l.starts_with("net:") { assert_eq!(log[i - 1], "approve", "send without immediate approval"); }
        }
        assert!(net.posts.borrow().len() <= q);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn only_exact_matching_acknowledgement_is_accepted() {
    let big = format!("{{\"approve\":1,\"x\":\"{}\"}}", "a".repeat(MAX_ACK));
    for (raw, ok) in [(&b"{\"approve\":1}"[..], true), (b" { \"approve\" : 1 } ", true),
        (b"{\"approve\":2}", false), (b"{\"approve\":0}", false), (b"{\"approve\":\"1\"}", false),
        (b"{\"approve\":true}", false), (b"{\"approve\":1,\"x\":1}", false), (b"[1]", false),
        (b"", false), (b"not json", false), (big.as_bytes(), false)] {
        let log: Log = Default::default();
        let events: Events = Default::default();
        let mut chan = Chan::new(&log, &events, 1);
        chan.raw = Some(raw.to_vec());
        let mut g = Guard::new(chan);
        assert_eq!(g.authorize("initial", json!({})).is_ok(), ok, "{:?}", String::from_utf8_lossy(raw));
    }
}
