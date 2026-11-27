//! Source-bound ADR-0034 counterexample, NOT historical failed01/02 attribution.
use super::*;
use std::time::Instant;

#[cfg(feature = "ffi-test-hooks")]
fn ready_host(name: &str) -> (Host, Link) {
    ready_host_with(name, &program(false))
}
fn ready_host_with(name: &str, wasm: &[u8]) -> (Host, Link) {
    let mut host = Host::new(true, name);
    host.stop();
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    host.setup = with_wasm(setup(), wasm);
    host.start();
    let mut link = host.connect();
    link.hello();
    link.grants(1);
    for id in ["f", "4"] {
        assert_eq!(
            link.recv_until("act")["body"]["action"]["id"],
            id.repeat(32)
        );
    }
    (host, link)
}
#[cfg(feature = "ffi-test-hooks")]
fn verify_guest(host: &mut Host, link: Link) {
    drop(link);
    host.stop();
    let recovered = RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 1);
    let record = &recovered.records()[0];
    assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
    assert_eq!(record["status"], "committed");
    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    assert_eq!(record["calls"].as_array().unwrap().len(), 2);
}

#[test]
fn ready_health_latency_bounded_under_pump_load() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // Pending native guest work is distinct from an already-entered guest,
    // whose service lock may still delay mandatory preflight under ADR-0034.
    let mut data = String::new();
    let mut calls = String::new();
    for (i, action) in setup()["initial_actions"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
    {
        let bytes = cbor(action).unwrap();
        let ptr = i * 2048;
        data.push_str(&format!("(data (i32.const {ptr}) \"{}\")", escaped(&bytes)));
        calls.push_str(&format!(
            "i32.const {ptr} i32.const {} call $emit drop ",
            bytes.len()
        ));
    }
    let timer = cbor(&json!({"id":"a".repeat(32),"at":1_000_000})).unwrap();
    let wasm = wat::parse_str(format!(
        "(module
        (import \"coglet_v0\" \"emit\" (func $emit (param i32 i32) (result i32)))
        (import \"coglet_v0\" \"timer\" (func $timer (param i32 i32) (result i32)))
        (memory (export \"memory\") 1 2) {data}
        (data (i32.const 4096) \"{}\")
        (func (export \"abi_alloc\") (param i32) (result i32) i32.const 8192)
        (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32)
            {calls} i32.const 4096 i32.const {} call $timer drop i32.const 0))",
        escaped(&timer),
        timer.len()
    ))
    .unwrap();
    let (mut host, mut link) = ready_host_with("ready-health-latency", &wasm);
    std::thread::sleep(Duration::from_millis(1050));
    let started = Instant::now();
    assert_eq!(decode(&link.recv())["type"], "health");
    let Link::Ffi(raw) = &link else {
        unreachable!()
    };
    let (history, first_failure) = unsafe { (&**raw).timings() };
    let calls: Vec<_> = history.iter().rev().take(2).collect();
    eprintln!(
        "ADR0034 no-obstruction query/copy={calls:?}; first_failure={first_failure:?}; wall={:?}",
        started.elapsed()
    );
    let no_pump = calls
        .iter()
        .all(|call| !call.send && call.ready_us.is_some() && call.pump_entry_us.is_none());
    let bounded = started.elapsed() < Duration::from_millis(500);
    // Only the subsequent empty receive may run the pending real timer guest.
    let raw = *raw;
    let mut n = usize::MAX;
    assert_eq!(
        unsafe { coglet_surface_recv(raw, std::ptr::null_mut(), 0, &mut n) },
        1
    );
    assert_eq!(n, 0);
    drop(link);
    host.stop();
    let recovered = RuntimeService::open(&host.dir.join("state.wal"), &host.setup, 128).unwrap();
    assert_eq!(recovered.records().len(), 2);
    for record in recovered.records() {
        assert_eq!(record["engine"]["build"], coglet_engine_wamr::BUILD);
        assert_eq!(record["status"], "committed");
        assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
    }
    assert_eq!(
        recovered.records()[0]["timers"].as_array().unwrap().len(),
        1
    );
    assert_eq!(recovered.records()[1]["events"][0]["kind"], "runtime.timer");
    eprintln!(
        "ADR0034 pending native load: two committed WAMR steps, second runtime.timer; empty recv progressed"
    );
    assert!(no_pump, "ready output invoked optional pump; timings above");
    assert!(
        bounded,
        "environment qualification threshold, not a real-time guarantee"
    );
}

#[cfg(feature = "ffi-test-hooks")]
#[test]
fn ready_output_not_obstructed_by_pump() {
    let _guard = FFI_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let (mut host, link) = ready_host("ready-health-obstruction");
    let Link::Ffi(raw) = &link else {
        unreachable!()
    };
    let raw = *raw;
    let hold = unsafe { coglet_surface_test_hold(raw) };
    assert!(!hold.is_null());
    // Let the production tick naturally generate health. The peer remains well
    // within its frozen 3s inactivity allowance; no clock or queue fabrication.
    std::thread::sleep(Duration::from_millis(1050));
    let address = raw as usize;
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut n = 0;
        let status = unsafe {
            coglet_surface_recv(
                address as *mut CogletSurfaceLink,
                std::ptr::null_mut(),
                0,
                &mut n,
            )
        };
        tx.send((status, n)).unwrap();
    });
    let ready = rx.recv_timeout(Duration::from_millis(250));
    // Always release and join before inspecting or closing the exclusive handle,
    // including the failing old-ordering experiment.
    unsafe { coglet_surface_test_release(hold) };
    worker.join().unwrap();
    let result = ready
        .as_ref()
        .copied()
        .unwrap_or_else(|_| rx.recv().unwrap());
    let (history, failure) = unsafe { (&*raw).timings() };
    eprintln!(
        "ADR0034 SOURCE-BOUND OBSTRUCTION ready={ready:?}; entries={}; history={history:?}; first_failure={failure:?}",
        unsafe { coglet_surface_test_entries(hold) }
    );
    assert_eq!(result.0, 2);
    let mut short = [0u8; 1];
    let mut n = 0;
    assert_eq!(
        unsafe { coglet_surface_recv(raw, short.as_mut_ptr(), 1, &mut n) },
        2
    );
    assert_eq!(n, result.1);
    let mut bytes = vec![0; n];
    assert_eq!(
        unsafe { coglet_surface_recv(raw, bytes.as_mut_ptr(), bytes.len(), &mut n) },
        0
    );
    assert_eq!(decode(&bytes)["type"], "health");
    let entries = unsafe { coglet_surface_test_entries(hold) };
    unsafe { coglet_surface_test_free_hold(hold) };
    // Capture only after worker join: snapshot never aliases an in-flight recv.
    let mut size = 0;
    assert_eq!(
        unsafe { coglet_surface_test_timings(raw, std::ptr::null_mut(), 0, &mut size) },
        2
    );
    assert!(size < 32_768, "fixed diagnostic budget");
    let mut capture = vec![0; size];
    assert_eq!(
        unsafe { coglet_surface_test_timings(raw, capture.as_mut_ptr(), size, &mut size) },
        0
    );
    let capture: Value = serde_json::from_slice(&capture).unwrap();
    assert!(capture["adapter_fifo_depth"].is_null());
    assert!(capture["recent"].as_array().unwrap().len() <= 16);
    let last = capture["recent"].as_array().unwrap().last().unwrap();
    assert_eq!(last["output_kind"], "Health");
    assert_eq!(last["output_depth"], 1);
    eprintln!("ADR0034 C timing export: {capture}");
    verify_guest(&mut host, link);
    assert!(ready.is_ok(), "ready health obstructed by optional pump");
    assert_eq!(entries, 0, "size query/short-buffer/copy entered pump");
}
