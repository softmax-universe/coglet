//! Source-bound ADR-0034 counterexample, NOT historical failed01/02 attribution.
use super::*;
use std::time::Instant;

fn ready_host(name: &str) -> (Host, Link) {
    let mut host = Host::new(true, name);
    host.stop();
    std::fs::remove_file(host.dir.join("state.wal")).unwrap();
    host.setup = with_wasm(setup(), &program(false));
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
    let (mut host, mut link) = ready_host("ready-health-latency");
    // No-obstruction control: real C send/recv, not SurfaceSession injection.
    link.send(
        "health",
        json!({"state":"ready","clock":1200,"pending":0,"correlation_id":"9".repeat(32)}),
    );
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
    verify_guest(&mut host, link);
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
    let hold = std::sync::Arc::new(timing::obstruction::PumpHold::default());
    unsafe {
        (&mut *raw).hold_recv_pump(hold.clone());
    }
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
    hold.release();
    worker.join().unwrap();
    let result = ready
        .as_ref()
        .copied()
        .unwrap_or_else(|_| rx.recv().unwrap());
    let (history, failure) = unsafe { (&*raw).timings() };
    eprintln!(
        "ADR0034 SOURCE-BOUND OBSTRUCTION ready={ready:?}; entries={}; history={history:?}; first_failure={failure:?}",
        hold.entries()
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
    let entries = hold.entries();
    verify_guest(&mut host, link);
    assert!(ready.is_ok(), "ready health obstructed by optional pump");
    assert_eq!(entries, 0, "size query/short-buffer/copy entered pump");
}
