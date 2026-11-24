//! Residual R-retry process-exit boundaries, not physical-effect exactly-once proof.
use coglet_runtime::{RuntimeService, SurfaceBinding, SurfaceSession, cbor};
use serde_json::{Value, json};
use std::{path::Path, process::Command};

const C: &str = "coglet:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const S: &str = "surface:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
fn action() -> Value {
    json!({"coglet":C,"id":"4".repeat(32),"capability":"expression.set_expression","args":{"expression":"happy","intensity":500,"duration_ms":10},"deadline":20_000,"correlation_id":"5".repeat(32)})
}
fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["manifest"]["id"] = json!(C);
    setup["initial_effect_ids"]["actions"] = json!([action()["id"]]);
    setup["initial_actions"] = json!([action()]);
    setup
}
fn session(authorized: bool) -> SurfaceSession {
    let mut s = SurfaceSession::new(SurfaceBinding::new(S.into(), vec![C.into()]).unwrap());
    let mut send = |kind: &str, body: Value, now| {
        assert_eq!(
            s.send(&cbor(&json!({"type":kind,"body":body})).unwrap(), now),
            0
        )
    };
    send(
        "hello",
        json!({"surface":S,"offer":{"versions":["0.1"]},"clock":1000,"correlation_id":"1".repeat(32)}),
        10,
    );
    if authorized {
        send(
            "capabilities",
            json!({"revision":1,"clock":1100,"entries":[{"name":"expression.control","events":[],"reads":["expression.state"],"actions":["expression.set_expression"],"exclusive":["display.face"],"schemas":{"expression.state":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/expression-state","expression.set_expression":"https://coglet.dev/spec/messages/expression.schema.json#/$defs/set-expression"}}],"correlation_id":"2".repeat(32)}),
            110,
        );
        send(
            "grants",
            json!({"epoch":1,"entries":[{"coglet":C,"operations":["expression.state","expression.set_expression"],"resources":["display.face"],"expires":1_000_000}],"correlation_id":"3".repeat(32)}),
            120,
        );
    }
    s
}
fn accept() -> Value {
    json!({"id":action()["id"],"correlation_id":action()["correlation_id"],"accepted":true})
}
fn outcome() -> Value {
    json!({"source":S,"kind":"action.outcome","seq":1,"logical_time":1200,"payload":{"id":action()["id"],"correlation_id":action()["correlation_id"],"status":"succeeded","result":{}},"correlation_id":action()["correlation_id"]})
}
fn later_outcome() -> Value {
    let mut event = outcome();
    event["seq"] = json!(2);
    event
}
#[test]
fn synthesized_success_requires_retained_admission_source_and_correlation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    let s = session(true);
    assert!(runtime.record_surface_outcome(&s, &outcome(), 130).is_err());
    runtime
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 130)
        .unwrap();
    assert!(runtime.record_surface_outcome(&s, &outcome(), 140).is_err());
    runtime.record_surface_accept(&s, &accept(), 140).unwrap();
    for field in [
        "source",
        "outer-correlation",
        "inner-correlation",
        "unknown-id",
        "result",
    ] {
        let mut forged = outcome();
        match field {
            "source" => {
                forged["source"] = json!(
                    "surface:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
                )
            }
            "outer-correlation" => forged["correlation_id"] = json!("a".repeat(32)),
            "inner-correlation" => forged["payload"]["correlation_id"] = json!("a".repeat(32)),
            "unknown-id" => forged["payload"]["id"] = json!("a".repeat(32)),
            "result" => forged["payload"]["result"] = json!({"unexpected": true}),
            _ => unreachable!(),
        }
        assert!(
            runtime.record_surface_outcome(&s, &forged, 150).is_err(),
            "accepted forged {field}"
        );
        assert_eq!(runtime.queued(), 0, "forged {field} entered the inbox");
    }
    // Same source sequence remains usable: refused attempts did not consume it.
    assert!(runtime.record_surface_outcome(&s, &outcome(), 160).unwrap());
    assert_eq!(runtime.queued(), 1);
    drop(runtime);
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    assert!(
        !runtime
            .record_surface_outcome(&session(false), &later_outcome(), 2000)
            .unwrap()
    );
    assert_eq!(runtime.queued(), 1);
}

#[test]
fn unknown_admission_no_new_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    let original = runtime
        .prepare_dispatch(&session(true), action()["id"].as_str().unwrap(), 130)
        .unwrap();
    drop(runtime);
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    assert_eq!(
        runtime
            .prepare_dispatch(&session(false), &"a".repeat(32), 2000)
            .unwrap_err(),
        "action is not durably pending"
    );
    assert_eq!(
        runtime
            .prepare_dispatch(&session(false), action()["id"].as_str().unwrap(), 2000)
            .unwrap(),
        original
    );
    assert!(
        runtime
            .record_surface_outcome(&session(false), &outcome(), 2010)
            .is_err()
    );
    assert_eq!(runtime.queued(), 0);
}

#[test]
fn terminal_rejection_allows_new_id() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    let mut configured = setup();
    configured["manifest"]["required_capabilities"] = json!(["expression.set_expression"]);
    configured["manifest"]["exclusive_outputs"] = json!(["display.face"]);
    let mut runtime = RuntimeService::open(&path, &configured, 128).unwrap();
    let s = session(true);
    let original = runtime
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 130)
        .unwrap();
    let mut rejected = accept();
    rejected["accepted"] = json!(false);
    rejected["error"] =
        json!({"code":"permission_denied","message":"fixture refusal","retryable":false});
    runtime.record_surface_accept(&s, &rejected, 140).unwrap();
    drop(runtime);
    let mut runtime = RuntimeService::open(&path, &configured, 128).unwrap();
    assert_eq!(runtime.queued(), 1);
    let mut context = configured["inputs"].clone();
    context["logical_time"] = json!(150);
    context["grants"] = json!(["expression.set_expression"]);
    let mut replacement = action();
    replacement["id"] = json!("a".repeat(32));
    let script = json!({"calls":[{"op":"emit","argument":replacement,"result":0}],"writes":{},"termination":"committed","usage":{"compute":10,"memory_pages":1,"wall_time_us":20,"host_bytes":0}});
    let result = runtime
        .run_next(&context, &mut coglet_runtime::MockEngine::new(script))
        .unwrap()
        .unwrap();
    assert_eq!(result["journal"]["actions"], json!([replacement]));
    let bytes = runtime.prepare_dispatch(&s, &"a".repeat(32), 160).unwrap();
    assert_ne!(bytes, original);
    assert_eq!(
        runtime
            .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 160)
            .unwrap(),
        original
    );
    runtime.record_surface_accept(&s, &rejected, 170).unwrap();
    assert_eq!(
        runtime.queued(),
        0,
        "historical rejection generated another wake"
    );
}

fn crash_child(path: &Path, point: &str) {
    let result = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "retry_crash_child", "--ignored", "--nocapture"])
        .env("COGLET_M1_RETRY_WAL", path)
        .env("COGLET_M1_RETRY_POINT", point)
        .status()
        .unwrap();
    assert_eq!(
        result.code(),
        Some(73),
        "child did not reach requested durable boundary"
    );
}
#[test]
#[ignore = "subprocess helper, selected by each ordinary crash test"]
fn retry_crash_child() {
    let path = std::env::var_os("COGLET_M1_RETRY_WAL").unwrap();
    let point = std::env::var("COGLET_M1_RETRY_POINT").unwrap();
    let mut runtime = RuntimeService::open(Path::new(&path), &setup(), 128).unwrap();
    let s = session(true);
    let bytes = runtime
        .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 130)
        .unwrap();
    std::fs::write(Path::new(&path).with_extension("expected"), bytes).unwrap();
    match point.as_str() {
        "persist-before-send" => (),
        "accept-before-outcome" => runtime.record_surface_accept(&s, &accept(), 140).unwrap(),
        "outcome-before-consumption" => {
            runtime.record_surface_accept(&s, &accept(), 140).unwrap();
            assert!(runtime.record_surface_outcome(&s, &outcome(), 150).unwrap());
        }
        _ => panic!("unknown crash point"),
    }
    // Deliberately skip destructors: persisted state, not orderly close, must recover.
    std::process::exit(73);
}
#[test]
fn crash_after_persist_before_send() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    crash_child(&path, "persist-before-send");
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    let s = session(false); // no replacement grant or clock sample authorizes a remap
    assert_eq!(
        runtime
            .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 2000)
            .unwrap(),
        std::fs::read(path.with_extension("expected")).unwrap()
    );
    assert_eq!(runtime.queued(), 0);
    assert!(
        runtime
            .record_surface_outcome(&s, &outcome(), 2010)
            .is_err(),
        "unadmitted success was synthesized"
    );
}
#[test]
fn crash_after_accept_before_outcome_reconciles_original_admission() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    crash_child(&path, "accept-before-outcome");
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    let s = session(false);
    assert_eq!(
        runtime
            .prepare_dispatch(&s, action()["id"].as_str().unwrap(), 2000)
            .unwrap(),
        std::fs::read(path.with_extension("expected")).unwrap()
    );
    assert!(
        runtime
            .record_surface_outcome(&s, &outcome(), 2010)
            .unwrap()
    );
    assert!(
        !runtime
            .record_surface_outcome(&s, &later_outcome(), 2020)
            .unwrap()
    );
    assert_eq!(runtime.queued(), 1);
}
#[test]
fn crash_after_outcome_before_consumption_keeps_one_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("runtime.wal");
    crash_child(&path, "outcome-before-consumption");
    let mut runtime = RuntimeService::open(&path, &setup(), 128).unwrap();
    let s = session(false);
    assert_eq!(runtime.queued(), 1);
    assert!(
        !runtime
            .record_surface_outcome(&s, &later_outcome(), 2000)
            .unwrap()
    );
    assert_eq!(runtime.queued(), 1);
    let mut changed = outcome();
    changed["seq"] = json!(3);
    changed["payload"]["status"] = json!("cancelled");
    changed["payload"].as_object_mut().unwrap().remove("result");
    assert!(runtime.record_surface_outcome(&s, &changed, 2010).is_err());
    assert_eq!(runtime.queued(), 1);
}
