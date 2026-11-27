//! D1 discriminator controls only: queued != consumed != accepted. Synthetic
//! authority samples do not reproduce a historical failure or measure latency.
use super::*;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
    mpsc,
};
use std::thread::JoinHandle;

const LAST: u64 = 100;
const BELOW: u64 = LAST + 2_999_999;
const EXACT: u64 = LAST + 3_000_000;
const BOOT_BASE: u64 = 40_000_000;
const WAIT: Duration = Duration::from_secs(5);
const SURFACE: &str = "surface:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn queued(stream: &UnixStream) -> usize {
    let mut count: libc::c_int = -1;
    // SAFETY: valid live socket fd and writable c_int storage.
    assert_eq!(
        unsafe { libc::ioctl(stream.as_raw_fd(), libc::FIONREAD, &mut count) },
        0
    );
    usize::try_from(count).unwrap()
}

fn frame(kind: &str, body: Value) -> Vec<u8> {
    crate::cbor(&json!({"type":kind,"body":body})).unwrap()
}

fn marker() -> Vec<u8> {
    frame(
        "health",
        json!({"state":"stopped","clock":1000,"pending":0,
        "correlation_id":"2".repeat(32)}),
    )
}

#[derive(Debug, PartialEq, Eq)]
struct Observation {
    queued: usize,
    last: u64,
    sampled: u64,
}

struct Outcome {
    result: Result<&'static str>,
    last: u64,
    remaining: usize,
}

// On any assertion failure, unblock both the finite gate and socket, then reap
// the worker. Neither watchdog nor cleanup advances/renews runtime authority.
struct Worker {
    handle: Option<JoinHandle<Outcome>>,
    release: Option<mpsc::Sender<()>>,
    cleanup: UnixStream,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.release.take();
        let _ = self.cleanup.shutdown(std::net::Shutdown::Both);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn run(
    bytes: &[u8],
    initial: u64,
    jump_at_pump: Option<usize>,
) -> (Outcome, Vec<Observation>, Vec<Value>) {
    let dir = tempfile::tempdir().unwrap();
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    let mut service = crate::RuntimeService::open(&dir.path().join("d1.wal"), &setup, 8).unwrap();
    let sample = Arc::new(AtomicU64::new(BOOT_BASE));
    let clock = sample.clone();
    service.test_set_clock_sample(Arc::new(move || {
        Ok(("d1-fixed-boot".into(), clock.load(Ordering::SeqCst)))
    }));
    // Establish the durable anchor BEFORE the session and threshold sample.
    assert_eq!(service.supervised_now().unwrap(), 0);
    let binding = SurfaceBinding::new(
        SURFACE.into(),
        vec![setup["manifest"]["id"].as_str().unwrap().into()],
    )
    .unwrap();
    let (mut peer, mut stream) = UnixStream::pair().unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    peer.set_write_timeout(Some(WAIT)).unwrap();
    peer.set_read_timeout(Some(WAIT)).unwrap();
    // Use the real transport's kernel credential lookup, never a claimed hello.
    // SAFETY: geteuid has no preconditions.
    assert_eq!(peer_uid_linux(&stream).unwrap(), unsafe { libc::geteuid() });
    let mut session = SurfaceSession::with_service(binding, Arc::new(Mutex::new(service))).unwrap();
    sample.store(BOOT_BASE + LAST, Ordering::SeqCst);
    assert_eq!(session.transport_now(Instant::now()).unwrap(), LAST);
    assert_eq!(
        session.send(
            &frame(
                "hello",
                json!({"surface":SURFACE,
        "offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)})
            ),
            LAST
        ),
        0
    );
    let reply: Value = coglet_types::decode("surface-message", &session.pop().unwrap())
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(reply["type"], "hello_reply");
    assert_eq!(reply["body"]["selection"]["version"], "0.1");
    assert_eq!(session.test_last_received(), LAST);

    // write_all returning is the finite successful-send acknowledgement. The
    // receiver queue is independently measured BEFORE any transport read/tick.
    peer.write_all(bytes).unwrap();
    peer.shutdown(std::net::Shutdown::Write).unwrap(); // finite empty/partial EOF
    assert_eq!(queued(&stream), bytes.len());
    sample.store(BOOT_BASE + initial, Ordering::SeqCst); // BEFORE header tick
    assert_eq!(session.transport_now(Instant::now()).unwrap(), initial);

    let inspect = stream.try_clone().unwrap();
    let cleanup = stream.try_clone().unwrap();
    let (observed_tx, observed_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    session.test_set_pump_hook(Box::new(move |session, now| {
        observed_tx
            .send(Observation {
                queued: queued(&inspect),
                last: session.test_last_received(),
                sampled: now,
            })
            .unwrap();
        release_rx
            .recv_timeout(WAIT)
            .expect("D1 pump release watchdog");
    }));
    let handle = std::thread::spawn(move || {
        let result =
            serve_session(&mut stream, &mut session, Instant::now(), None).map(|end| match end {
                ServeEnd::Eof => "eof",
                ServeEnd::Closed => "closed",
            });
        let outcome = Outcome {
            result,
            last: session.test_last_received(),
            remaining: queued(&stream),
        };
        session.disconnect();
        outcome
    });
    let mut worker = Worker {
        handle: Some(handle),
        release: Some(release_tx),
        cleanup,
    };
    let mut observations = Vec::new();
    loop {
        match observed_rx.recv_timeout(WAIT + WAIT) {
            Ok(observation) => {
                let index = observations.len();
                observations.push(observation);
                if jump_at_pump == Some(index) {
                    // Worker is INSIDE pump; its tick has ALREADY used initial.
                    // Publish exact authority before release; hold it thereafter.
                    sample.store(BOOT_BASE + EXACT, Ordering::SeqCst);
                }
                worker.release.as_ref().unwrap().send(()).unwrap();
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => panic!("D1 worker acknowledgement watchdog"),
        }
    }
    let outcome = worker
        .handle
        .take()
        .unwrap()
        .join()
        .expect("D1 worker panicked");
    let expected_sample = if jump_at_pump.is_some() {
        EXACT
    } else {
        initial
    };
    assert_eq!(sample.load(Ordering::SeqCst), BOOT_BASE + expected_sample);
    // End the response direction while retaining the receiver clone: dropping
    // a Unix socket with unread input can reset the peer. EOF here is explicit,
    // not an assertion about a portable reset errno or full input consumption.
    worker.cleanup.shutdown(std::net::Shutdown::Write).unwrap();
    let mut wire = Vec::new();
    peer.read_to_end(&mut wire).unwrap();
    drop(worker);
    let mut responses = Vec::new();
    let mut rest = wire.as_slice();
    while !rest.is_empty() {
        assert!(rest.len() >= 4);
        let n = u32::from_be_bytes(rest[..4].try_into().unwrap()) as usize;
        assert!(rest.len() >= 4 + n);
        responses.push(
            coglet_types::decode("surface-message", &rest[4..4 + n])
                .unwrap()
                .deserialized()
                .unwrap(),
        );
        rest = &rest[4 + n..];
    }
    (outcome, observations, responses)
}

fn packet(raw: &[u8]) -> Vec<u8> {
    let mut bytes = (raw.len() as u32).to_be_bytes().to_vec();
    bytes.extend_from_slice(raw);
    bytes
}

fn observation(queued: usize, last: u64, sampled: u64) -> Observation {
    Observation {
        queued,
        last,
        sampled,
    }
}

#[test]
fn d1_full_valid_frame_fixed_below_refreshes_only_after_body_and_send() {
    let raw = marker();
    let bytes = packet(&raw);
    let (out, seen, wire) = run(&bytes, BELOW, None);
    assert_eq!(out.result, Ok("eof"));
    assert_eq!(out.last, BELOW);
    assert_eq!(out.remaining, 0);
    assert_eq!(
        seen,
        vec![
            observation(bytes.len(), LAST, BELOW), // tick/pump before header read
            observation(raw.len(), LAST, BELOW),   // header consumed, body queued
            observation(0, BELOW, BELOW),          // accepted send, post-send pump
            observation(0, BELOW, BELOW),          // next header -> clean EOF
        ]
    );
    let response = wire
        .iter()
        .find(|v| v["body"]["correlation_id"] == "2".repeat(32))
        .expect("accepted marker's wire reply");
    assert_eq!(response["type"], "health");
    assert_eq!(response["body"]["clock"], BELOW);
}

#[test]
fn d1_full_valid_frame_fixed_exact_expires_before_header_consumption() {
    let bytes = packet(&marker());
    let (out, seen, wire) = run(&bytes, EXACT, None);
    assert_eq!(out.result, Err("surface inactivity timeout".into()));
    assert_eq!(out.last, LAST);
    assert_eq!(out.remaining, bytes.len());
    assert!(seen.is_empty(), "expired tick closes before pump hook/read");
    assert!(wire.is_empty());
}

#[test]
fn d1_jump_inside_header_pump_consumes_header_but_body_tick_rejects() {
    let raw = marker();
    let bytes = packet(&raw);
    let (out, seen, wire) = run(&bytes, BELOW, Some(0));
    assert_eq!(out.result, Err("surface inactivity timeout".into()));
    assert_eq!(out.last, LAST);
    assert_eq!(out.remaining, raw.len()); // ONLY header consumed
    assert_eq!(seen, vec![observation(bytes.len(), LAST, BELOW)]);
    assert!(
        !wire
            .iter()
            .any(|v| v["body"]["correlation_id"] == "2".repeat(32))
    );
}

#[test]
fn d1_jump_inside_body_pump_consumes_body_but_presend_rejects() {
    let raw = marker();
    let bytes = packet(&raw);
    let (out, seen, wire) = run(&bytes, BELOW, Some(1));
    assert_eq!(out.result, Err("fatal surface message".into()));
    assert_eq!(out.last, LAST);
    assert_eq!(out.remaining, 0); // FULL bytes consumed, frame NOT accepted
    assert_eq!(
        seen,
        vec![
            observation(bytes.len(), LAST, BELOW),
            observation(raw.len(), LAST, BELOW)
        ]
    );
    assert!(
        !wire
            .iter()
            .any(|v| v["body"]["correlation_id"] == "2".repeat(32))
    );
}

#[test]
fn d1_empty_and_partial_eof_do_not_refresh_receive_authority() {
    let raw = marker();
    let bytes = packet(&raw);
    for (input, expected, expected_seen) in [
        (&bytes[..0], Ok("eof"), vec![observation(0, LAST, BELOW)]),
        (
            &bytes[..2],
            Err("EOF mid-frame".into()),
            vec![observation(2, LAST, BELOW), observation(0, LAST, BELOW)],
        ),
        (
            &bytes[..6],
            Err("EOF mid-frame".into()),
            vec![
                observation(6, LAST, BELOW),
                observation(2, LAST, BELOW),
                observation(0, LAST, BELOW),
            ],
        ),
    ] {
        let (out, seen, wire) = run(input, BELOW, None);
        assert_eq!(out.result, expected);
        assert_eq!(out.last, LAST);
        assert_eq!(out.remaining, 0);
        assert_eq!(seen, expected_seen);
        assert!(
            !wire
                .iter()
                .any(|v| v["body"]["correlation_id"] == "2".repeat(32))
        );
    }
}

#[test]
fn d1_complete_invalid_cbor_consumed_without_receive_refresh() {
    // Valid nonzero length, invalid surface-message CBOR. Consumption alone is
    // not acceptance even with a timely sample and a fully negotiated session.
    let bytes = packet(&[0xff]);
    let (out, seen, wire) = run(&bytes, BELOW, None);
    assert_eq!(out.result, Err("fatal surface message".into()));
    assert_eq!(out.last, LAST);
    assert_eq!(out.remaining, 0);
    assert_eq!(
        seen,
        vec![
            observation(bytes.len(), LAST, BELOW),
            observation(1, LAST, BELOW)
        ]
    );
    assert!(
        !wire
            .iter()
            .any(|v| v["body"]["correlation_id"] == "2".repeat(32))
    );
}
