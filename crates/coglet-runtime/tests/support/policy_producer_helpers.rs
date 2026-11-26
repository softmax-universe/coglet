// Included by policy_producers.rs: real guest-action, fault-shim and adapter
// helpers (no production fault switches, no fake producers).

fn spawn_child(config: &Path, log: &Path, env: &[(String, String)]) -> ChildGuard {
    let stderr = std::fs::File::create(log).unwrap();
    ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_cogletd"))
            .arg("--router-service")
            .arg(config)
            .env("TOKIO_WORKER_THREADS", "2")
            .envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdout(std::process::Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap(),
    )
}

fn operations(action: bool) -> Vec<&'static str> {
    if action {
        vec!["speech.speak_chunk", "speech.start"]
    } else {
        vec!["speech.start"]
    }
}

fn act_catalog() -> Value {
    json!([{"name":"speech.control","events":["speech.start"],"reads":[],
        "actions":["speech.speak_chunk"],"exclusive":["audio.mic","audio.speaker"],
        "schemas":{"speech.start":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-start",
        "speech.speak_chunk":"https://coglet.dev/spec/messages/speech.schema.json#/$defs/speak-chunk"}}])
}

/// Real Wasm guest: each `handle` step emits ONE schema-valid
/// `speech.speak_chunk` action whose ID ends in the step number, so every step
/// produces a distinct physical output.
fn action_wat(coglet: &str) -> String {
    let id = "0".repeat(32);
    let action = json!({"coglet":coglet,"id":id,"capability":"speech.speak_chunk",
        "args":{"stream":"a".repeat(32),"chunk":0,"text":"proposal","final":true},
        "deadline":50_000_000u64,"correlation_id":"5".repeat(32)});
    let raw = coglet_runtime::cbor(&action).unwrap();
    let offset = raw
        .windows(32)
        .position(|window| window == id.as_bytes())
        .unwrap();
    let data: String = raw.iter().map(|byte| format!("\\{byte:02x}")).collect();
    format!(
        "(module (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))
        (memory (export \"memory\") 1 2) (data (i32.const 0) \"{data}\")
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32)
          (local $i i32) (local $n i32)
          (loop $digits
            local.get 2 local.get $i i32.const 2 i32.shl i64.extend_i32_u i64.shr_u i32.wrap_i64
            i32.const 15 i32.and local.set $n
            i32.const {offset} local.get $i i32.add
            local.get $n i32.const 48 i32.add
            local.get $n i32.const 9 i32.gt_u i32.const 39 i32.mul i32.add
            i32.store8
            local.get $i i32.const 1 i32.add local.tee $i i32.const 4 i32.lt_u br_if $digits)
          i32.const 0 i32.const {len} call $emit drop i32.const 0))",
        len = raw.len()
    )
}

async fn put(link: &mut UnixStream, kind: &str, body: Value) -> bool {
    let raw = coglet_runtime::cbor(&json!({"type":kind,"body":body})).unwrap();
    link.write_u32(raw.len() as u32).await.is_ok() && link.write_all(&raw).await.is_ok()
}

/// One adapter frame, `None` on timeout or EOF.
async fn take(link: &mut UnixStream, within: Duration) -> Option<Value> {
    tokio::time::timeout(within, async {
        let n = link.read_u32().await.ok()? as usize;
        let mut bytes = vec![0; n];
        link.read_exact(&mut bytes).await.ok()?;
        coglet_types::decode("surface-message", &bytes)
            .ok()?
            .deserialized()
            .ok()
    })
    .await
    .ok()
    .flatten()
}

/// Read until a real daemon `act` frame; any other frame is skipped.
async fn wait_act(link: &mut UnixStream) -> Value {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let frame = take(link, Duration::from_secs(15))
                .await
                .expect("guest action reaches adapter");
            if frame["type"] == "act" {
                return frame;
            }
        }
    })
    .await
    .expect("no act frame from the real guest")
}

/// Hello/capabilities/grants handshake with the daemon's adapter listener.
async fn adapter_up(socket: &Path, surface: &str, coglet: &str) -> UnixStream {
    let mut link = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match UnixStream::connect(socket).await {
                Ok(link) => break link,
                Err(_) => tokio::time::sleep(Duration::from_millis(20)).await,
            }
        }
    })
    .await
    .expect("adapter listener");
    assert!(put(&mut link, "hello", json!({"surface":surface,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"a".repeat(32)})).await);
    let hello = take(&mut link, Duration::from_secs(10)).await.unwrap();
    assert_eq!(hello["type"], "hello_reply", "{hello}");
    assert!(put(&mut link, "capabilities", json!({"revision":1,"clock":1100,"entries":act_catalog(),"correlation_id":"b".repeat(32)})).await);
    assert!(put(&mut link, "grants", json!({"epoch":1,"entries":[{"coglet":coglet,"operations":operations(true),"resources":["audio.mic","audio.speaker"],"expires":100_000_000}],"correlation_id":"c".repeat(32)})).await);
    link
}

async fn feed(link: &mut (impl AsyncWriteExt + Unpin), surface: &str, seq: u64) -> bool {
    let raw = coglet_runtime::cbor(&json!({"type":"event","body":{"source":surface,"seq":seq,
        "logical_time":2000+seq,"kind":"speech.start",
        "payload":{"utterance":correlation(seq),"position_ms":0},"correlation_id":correlation(seq)}})).unwrap();
    link.write_u32(raw.len() as u32).await.is_ok() && link.write_all(&raw).await.is_ok()
}

/// Bytes already queued in the kernel receive buffer of `link` (FIONREAD).
fn queued_bytes(link: &impl AsRawFd) -> usize {
    let mut n: libc::c_int = 0;
    // SAFETY: valid open socket fd and a writable c_int.
    assert_eq!(
        unsafe { libc::ioctl(link.as_raw_fd(), libc::FIONREAD, &mut n) },
        0
    );
    n as usize
}

/// Parse an exact byte string of length-prefixed adapter frames; a partial
/// trailing frame is a failure (it would be a torn physical exposure).
fn whole_frames(mut bytes: &[u8]) -> Vec<Value> {
    let mut frames = vec![];
    while !bytes.is_empty() {
        let n = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        assert!(bytes.len() >= 4 + n, "partial adapter frame exposed");
        frames.push(
            coglet_types::decode("surface-message", &bytes[4..4 + n])
                .unwrap()
                .deserialized()
                .unwrap(),
        );
        bytes = &bytes[4 + n..];
    }
    frames
}

fn producer_code(reply: &Value) -> &str {
    assert_eq!(reply["ok"], false, "{reply}");
    reply["error"]["code"].as_str().unwrap()
}

impl Daemon {
    async fn start_enabled(&mut self, bundle: &Bundle) {
        applied(&self.env.simple("hub.start").await);
        self.enable(bundle).await;
    }
    /// Start, enable the lease, connect the adapter and deliver two events; the
    /// installed guest runs and its first real action is read back from the
    /// adapter (physical output BEFORE any revoke).
    async fn execute_action(&mut self, bundle: &Bundle) -> (UnixStream, Value) {
        self.start_enabled(bundle).await;
        let mut link = adapter_up(&self.socket, &self.surface, &self.env.coglet).await;
        for seq in 1..=2 {
            assert!(feed(&mut link, &self.surface, seq).await);
        }
        let act = wait_act(&mut link).await;
        assert_eq!(
            act["body"]["action"]["capability"], "speech.speak_chunk",
            "{act}"
        );
        assert_eq!(act["body"]["action"]["coglet"], self.env.coglet, "{act}");
        (link, act)
    }
    fn hold_authority(&self) -> rusqlite::Connection {
        let db = rusqlite::Connection::open(self.root.join("authority.sqlite")).unwrap();
        db.execute_batch("BEGIN EXCLUSIVE;").unwrap();
        db
    }
    async fn step(&mut self) -> u64 {
        let status = self.env.simple("hub.status").await;
        status["result"]["step"].as_u64().unwrap()
    }
    async fn health(&mut self, bundle: &Bundle) -> Value {
        self.local("observe_health", self.pins(bundle)).await
    }
}
