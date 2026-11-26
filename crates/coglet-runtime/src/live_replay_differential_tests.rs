//! Differential/fail-closed coverage: the detached live-prefix reconciliation
//! (`reconcile_attempt_inner(.., true)`) must agree with full-journal
//! reconstruction (`false`) on the original attempt, and refuse tampering.
use super::*;

fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let find = |name: &str| {
        cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()["steps"][0]["send"]
            .clone()
    };
    let mut setup = find("action");
    // Borrow the authenticated inbox append for a non-empty boundary.
    setup["boundary_change"] = find("inbox-boundary")["inputs"]["boundary"][0].clone();
    setup["wall_abort"] = find("abort-wall")["guest_script"].clone();
    setup
}

fn hexid(n: u64) -> String {
    format!("{n:032x}")
}

/// Prefix/final guest script kinds: committed no-op, committed with file
/// writes, host calls (emit/log/timer), and an aborted step.
fn script(setup: &Value, kind: &str, n: u64) -> Value {
    let mut s = setup["guest_script"].clone();
    s["calls"] = json!([]);
    let id = hexid(0x1000 + n);
    let coglet = setup["manifest"]["id"].clone();
    match kind {
        "plain" => {}
        "files" => {
            s["writes"] = json!({
                format!("/data/w{n}"): hex::encode(format!("bytes{n}")),
                "/data/count": hex::encode(format!("{n}")),
            })
        }
        "calls" => {
            s["calls"] = json!([
                {"op":"emit","argument":{"id":id,"coglet":coglet,"capability":"motor.move",
                  "args":{"x":n},"deadline":100000,"correlation_id":id},"result":0},
                {"op":"log","argument":{"level":"info","message":"m","fields":{}},"result":0},
                {"op":"timer","argument":{"id":hexid(0x2000+n),"at":900000+n},"result":0},
            ]);
            s["writes"] = json!({format!("/data/c{n}"): hex::encode("x")});
        }
        "aborted" => {
            let mut a = setup["wall_abort"].clone();
            a["calls"][0]["argument"]["id"] = json!(id);
            a["calls"][0]["argument"]["correlation_id"] = json!(id);
            s = a;
        }
        _ => unreachable!(),
    }
    s
}

fn event(setup: &Value, n: u64) -> Value {
    let mut e = setup["inputs"]["events"][0].clone();
    e["seq"] = json!(n);
    e["logical_time"] = json!(10 + n);
    e["correlation_id"] = json!(hexid(0x3000 + n));
    e
}

fn ctx(setup: &Value, n: u64, boundary: Vec<Value>) -> Value {
    let mut c = setup["inputs"].clone();
    c["logical_time"] = json!(10 + n);
    c["boundary"] = json!(boundary);
    c
}

struct Scenario {
    _dir: tempfile::TempDir,
    service: RuntimeService,
    journal: Value,
    setup: Value,
    next_seq: u64,
}

/// Commit `prefix` kinds, then fence (not execute) a final attempt of `last`,
/// observing the original journal from a runtime clone, as the detached worker.
fn scenario(prefix: &[&str], last: &str, boundary: bool) -> Scenario {
    let setup = setup();
    let dir = tempfile::tempdir().unwrap();
    let mut service = RuntimeService::open(&dir.path().join("s.wal"), &setup, 32).unwrap();
    for (n, kind) in prefix.iter().enumerate() {
        let n = n as u64;
        service.enqueue(event(&setup, n)).unwrap();
        service
            .run_next(
                &ctx(&setup, n, vec![]),
                &mut MockEngine::new(script(&setup, kind, n)),
            )
            .unwrap()
            .unwrap();
    }
    let n = prefix.len() as u64;
    service.enqueue(event(&setup, n)).unwrap();
    let changes = if boundary {
        vec![setup["boundary_change"].clone()]
    } else {
        vec![]
    };
    let (inputs, _, _) = service
        .begin_attempt(&ctx(&setup, n, changes))
        .unwrap()
        .unwrap();
    let mut observer = service.state.runtime.clone();
    let journal = observer
        .step(&inputs, &mut MockEngine::new(script(&setup, last, n)))
        .unwrap()["journal"]
        .clone();
    service.state.runtime.unresolved_attempt = Some(inputs);
    service.state.fenced_staged_prefix = Some(0);
    Scenario {
        _dir: dir,
        service,
        journal,
        setup,
        next_seq: n + 1,
    }
}

impl Scenario {
    fn both(&self, state: &ServiceState, record: &Value) -> (Result<Value>, Result<Value>) {
        let wal = self.service.store.records();
        let run = |live| {
            RuntimeService::reconcile_attempt_inner(state, wal, record, live)
                .map(|s| serde_json::to_value(&s).unwrap())
        };
        (run(true), run(false))
    }
    /// Valid original attempt: both routes must succeed with identical state.
    fn assert_agree(&self, state: &ServiceState, label: &str) {
        let (live, full) = self.both(state, &self.journal);
        assert!(full.is_ok(), "{label}: full route rejected: {full:?}");
        assert_eq!(live, full, "{label}: live/full divergence");
        let next: ServiceState = serde_json::from_value(live.unwrap()).unwrap();
        assert!(next.runtime.unresolved_attempt.is_none(), "{label}");
    }
    fn assert_both_refuse(&self, state: &ServiceState, record: &Value, label: &str) {
        let (live, full) = self.both(state, record);
        assert!(live.is_err(), "{label}: live accepted tampered input");
        assert!(full.is_err(), "{label}: full accepted tampered input");
    }
}

const PREFIXES: &[&[&str]] = &[
    &[],
    &["plain", "files", "calls"],
    &["calls", "aborted", "files", "plain", "calls", "aborted"],
];
const LASTS: &[&str] = &["plain", "files", "calls", "aborted"];

#[test]
fn live_matches_full_for_prefix_and_final_step_kinds() {
    for prefix in PREFIXES {
        for last in LASTS {
            for boundary in [false, true] {
                let s = scenario(prefix, last, boundary);
                let label = format!("{prefix:?}+{last} boundary={boundary}");
                s.assert_agree(&s.service.state, &label);
                if boundary {
                    assert!(
                        !s.journal["boundary"].as_array().unwrap().is_empty(),
                        "{label}"
                    );
                }
            }
        }
    }
}

#[test]
fn live_matches_full_with_highwaters_beyond_journal_prefix() {
    for prefix in PREFIXES {
        for last in ["calls", "aborted"] {
            for boundary in [false, true] {
                let s = scenario(prefix, last, boundary);
                let base = s.service.state.clone();
                let mut cases: Vec<(&str, ServiceState)> = vec![];
                let mut seq = base.clone();
                seq.runtime.next_source_seq += 5;
                cases.push(("seq+5", seq));
                let attempt_time =
                    base.runtime.unresolved_attempt.as_ref().unwrap()["logical_time"]
                        .as_u64()
                        .unwrap();
                let mut time = base.clone();
                time.runtime.time = attempt_time;
                cases.push(("time=attempt", time));
                let mut both = base.clone();
                both.runtime.next_source_seq += 2;
                both.runtime.time = attempt_time;
                cases.push(("seq+time", both));
                // A time highwater past the attempt is an invalid fence on both routes.
                let mut late = base.clone();
                late.runtime.time = attempt_time + 1;
                s.assert_both_refuse(&late, &s.journal, "time past attempt");
                for (name, state) in cases {
                    let label = format!("{prefix:?}+{last} b={boundary} {name}");
                    let (live, full) = s.both(&state, &s.journal);
                    assert_eq!(live, full, "{label}: live/full divergence");
                    assert!(full.is_ok(), "{label}: {full:?}");
                    let next: ServiceState = serde_json::from_value(full.unwrap()).unwrap();
                    assert!(
                        next.runtime.next_source_seq >= state.runtime.next_source_seq
                            && next.runtime.time >= state.runtime.time,
                        "{label}: highwater lost"
                    );
                }
            }
        }
    }
}

#[test]
fn live_matches_full_with_events_queued_after_the_fence() {
    for prefix in PREFIXES {
        for last in ["plain", "calls"] {
            let mut s = scenario(prefix, last, false);
            // Concurrent admission while the guest ran: later router entries
            // must survive reconciliation identically on both routes.
            let ev = event(&s.setup, s.next_seq + 4);
            let queued = s.service.state.router.enqueue(ev.clone());
            assert!(queued.is_ok());
            let state = s.service.state.clone();
            s.assert_agree(&state, &format!("{prefix:?}+{last} queued"));
            let (live, _) = s.both(&state, &s.journal);
            let next: ServiceState = serde_json::from_value(live.unwrap()).unwrap();
            assert!(next.router.events().any(|e| e == &ev));
        }
    }
}

#[test]
fn live_and_full_both_refuse_tampered_observations() {
    for prefix in [PREFIXES[1], PREFIXES[2]] {
        let s = scenario(prefix, "calls", true);
        let state = s.service.state.clone();
        type Mutation = (&'static str, Box<dyn Fn(&mut Value)>);
        let mut cases: Vec<Mutation> = vec![
            ("trace_id", Box::new(|r| r["trace_id"] = json!(hexid(9)))),
            (
                "step",
                Box::new(|r| r["step"] = json!(r["step"].as_u64().unwrap() + 1)),
            ),
            ("previous", Box::new(|r| r["previous"] = json!(hexid(9)))),
            ("seed", Box::new(|r| r["seed"] = json!(99))),
            ("boundary", Box::new(|r| r["boundary"] = json!([]))),
            ("events", Box::new(|r| r["events"] = json!([]))),
            ("grants", Box::new(|r| r["grants"] = json!([]))),
            (
                "actions",
                Box::new(|r| r["actions"][0]["id"] = json!(hexid(9))),
            ),
            ("actions-dropped", Box::new(|r| r["actions"] = json!([]))),
            ("usage", Box::new(|r| r["usage"]["compute"] = json!(11))),
            ("status", Box::new(|r| r["status"] = json!("aborted"))),
        ];
        for key in ["before_hash", "input_hash", "output_hash", "record_hash"] {
            assert!(
                s.journal.get(key).is_some(),
                "missing tested hash field {key}"
            );
            cases.push((key, Box::new(move |r| r[key] = json!("0".repeat(64)))));
        }
        for (name, tamper) in cases {
            let mut record = s.journal.clone();
            tamper(&mut record);
            assert_ne!(record, s.journal, "mutation {name} was not exercised");
            s.assert_both_refuse(&state, &record, name);
        }
        // Fence/precondition refusals.
        let mut unfenced = state.clone();
        unfenced.runtime.unresolved_attempt = None;
        s.assert_both_refuse(&unfenced, &s.journal, "no fence");
        let mut consumed = state.clone();
        let n = consumed.runtime.unresolved_attempt.as_ref().unwrap()["events"]
            .as_array()
            .unwrap()
            .len();
        consumed.router.consume(n).unwrap();
        s.assert_both_refuse(&consumed, &s.journal, "router prefix consumed");
    }
}
