//! Adversarial core tests: negotiation, closed envelopes, admission order, dedup, reads,
//! health/clock, kill. Every message crosses the real deterministic-CBOR boundary.

mod common;
use coglet_surface_rust::ControlPeer;
use common::*;
use serde_json::{Value as Json, json};

fn set_expr(r: &mut Rig, id: u64, intensity: u64) -> Vec<Json> {
    r.act(
        id,
        "expression.set_expression",
        json!({"expression":"listening","intensity":intensity,"duration_ms":1000}),
    )
}

#[test]
fn hello_and_negotiation_shape() {
    // Provider-session operations need allowlisted routes; with them all 15 are advertised.
    let mut r = Rig::with(Opts {
        mode: "speech_to_speech",
        routes: &["preset-a"],
        tools: &["a.b"],
        ..Opts::default()
    });
    let hello = r.start();
    assert_eq!(hello["type"], "hello");
    assert_eq!(hello["body"]["surface"], SURFACE);
    assert_eq!(hello["body"]["offer"], json!({"versions": ["0.1"]}));
    assert_eq!(hello["body"]["clock"], 0);
    let reply = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hid(1)}});
    let out = r.send(&reply);
    assert_eq!(out[0]["body"]["revision"], 1);
    assert_eq!(out[1]["body"]["epoch"], 1);
    let entries = out[0]["body"]["entries"].as_array().unwrap();
    // each operation in exactly one entry, each with a pinned schema URI (never fetched)
    let mut seen = std::collections::BTreeSet::new();
    for e in entries {
        for k in ["events", "reads", "actions"] {
            for op in e[k].as_array().unwrap() {
                assert!(
                    seen.insert(op.as_str().unwrap().to_owned()),
                    "{op} advertised twice"
                );
                let uri = e["schemas"][op.as_str().unwrap()].as_str().unwrap();
                assert!(
                    uri.starts_with("https://coglet.dev/spec/messages/")
                        && uri.contains("#/$defs/"),
                    "{uri}"
                );
            }
        }
        assert_eq!(
            e["schemas"].as_object().unwrap().len(),
            ["events", "reads", "actions"]
                .iter()
                .map(|k| e[*k].as_array().unwrap().len())
                .sum::<usize>()
        );
    }
    assert_eq!(seen.len(), ALL_SPEECH.len());
    let ctl = entries
        .iter()
        .find(|e| e["name"] == "speech.control")
        .unwrap();
    assert_eq!(
        ctl["schemas"]["speech.config"],
        "https://coglet.dev/spec/messages/speech.schema.json#/$defs/speech-config"
    );
    assert_eq!(
        ctl["schemas"]["speech.stop"],
        "https://coglet.dev/spec/messages/speech.schema.json#/$defs/stop"
    );
}

#[test]
fn lifecycle_violations_close_before_effects() {
    // act before hello_reply
    let mut r = Rig::new();
    r.start();
    let m = r.act_body(
        B,
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        5_000_000,
        1,
    );
    assert!(r.send_raw(&encode(&m)).is_err());
    assert!(r.backend.executed().is_empty());
    // wrong correlation, wrong version, duplicate hello_reply
    let mut r = Rig::new();
    r.start();
    let bad = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hid(2)}});
    assert!(r.send_raw(&encode(&bad)).is_err());
    let mut r = Rig::new();
    r.start();
    let bad = json!({"type":"hello_reply","body":{"selection":{"version":"0.2"},"correlation_id":hid(1)}});
    assert!(r.send_raw(&encode(&bad)).is_err());
    let mut r = Rig::new();
    r.connect();
    let again = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hid(1)}});
    assert!(r.send_raw(&encode(&again)).is_err());
    // nothing before start
    let mut r = Rig::new();
    let reply = json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hid(1)}});
    assert!(r.send_raw(&encode(&reply)).is_err());
}

#[test]
fn wrong_direction_messages_are_fatal() {
    let wrong = [
        json!({"type":"hello","body":{"surface":SURFACE,"offer":{"versions":["0.1"]},"clock":0,"correlation_id":hid(1)}}),
        json!({"type":"capabilities","body":{"revision":1,"clock":0,"entries":[],"correlation_id":hid(1)}}),
        json!({"type":"grants","body":{"epoch":9,"entries":[{"coglet":B,"operations":["speech.stop"],"resources":["audio.speaker"],"expires":99}],"correlation_id":hid(1)}}),
        json!({"type":"event","body":{"source":B,"kind":"speech.start","seq":0,"logical_time":0,"payload":{"utterance":hid(1),"position_ms":0},"correlation_id":hid(1)}}),
        json!({"type":"read_reply","body":{"correlation_id":hid(1),"ok":false,"error":{"code":"internal","message":"x","retryable":false}}}),
        json!({"type":"accept","body":{"id":hid(1),"correlation_id":hid(1),"accepted":true}}),
        json!({"type":"kill","body":{"reason":"runtime says stop","correlation_id":hid(1)}}),
    ];
    for m in wrong {
        let mut r = Rig::new();
        r.connect();
        assert!(
            r.send_raw(&encode(&m)).is_err(),
            "{} must be fatal",
            m["type"]
        );
    }
}

#[test]
fn malformed_bytes_never_reach_dispatch() {
    let mut r = Rig::new();
    r.connect();
    let good = encode(&r.act_body(
        B,
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        5_000_000,
        1,
    ));
    // trailing bytes
    let mut t = good.clone();
    t.push(0);
    assert!(r.send_raw(&t).is_err());
    assert!(r.send_raw(&[]).is_err());
    assert!(r.send_raw(&good[..good.len() - 1]).is_err());
    // non-shortest length encoding (0x78 0x04 instead of 0x64) for the "type" key
    let mut nonshort = vec![0xa2, 0x78, 0x04];
    nonshort.extend_from_slice(b"type");
    nonshort.extend_from_slice(&[0x66]);
    nonshort.extend_from_slice(b"health");
    assert!(r.send_raw(&nonshort).is_err());
    // indefinite-length map, float, byte string, tag
    for bad in [
        vec![0xbf, 0xff],
        vec![0xf9, 0x3c, 0x00],
        vec![0x41, 0x00],
        vec![0xc1, 0x01],
    ] {
        assert!(r.send_raw(&bad).is_err());
    }
    // oversized frame
    assert!(r.send_raw(&vec![0u8; 1_048_577]).is_err());
    // unknown field / unknown type / missing field
    let m = json!({"type":"health","body":{"state":"ready","clock":1,"pending":0,"correlation_id":hid(1),"extra":1}});
    assert!(
        coglet_types::encode_typed(RULE, &m).is_err(),
        "closed record"
    );
    assert!(r.backend.executed().is_empty());
    // a valid action still works afterwards: errors above did not poison state
    let out = r.send_raw(&good).unwrap();
    assert!(!out.is_empty());
}

#[test]
fn accept_then_outcome_and_expression_readback() {
    let mut r = Rig::new();
    r.connect();
    r.advance(10_000);
    let out = set_expr(&mut r, 1, 500);
    assert_eq!(out.len(), 2);
    assert_eq!(accepted(&out[0]), Some(true));
    assert_eq!(out[0]["body"]["correlation_id"], hid(1));
    assert_eq!(status(&out[1]), "succeeded");
    assert_eq!(out[1]["body"]["payload"]["result"], json!({}));
    assert_eq!(
        out[1]["body"]["correlation_id"],
        out[1]["body"]["payload"]["correlation_id"]
    );
    assert_eq!(out[1]["body"]["source"], SURFACE);
    let s1 = r.read(10, "expression.state", json!({}));
    assert_eq!(s1["body"]["value"]["expression"], "listening");
    assert_eq!(s1["body"]["value"]["intensity"], 500);
    let rem = s1["body"]["value"]["remaining_ms"].as_u64().unwrap();
    assert!((1..=1000).contains(&rem), "{rem}");
    // timer expires on the native clock even with no coglet step
    r.advance(1_100_000);
    let s2 = r.read(11, "expression.state", json!({}));
    assert_eq!(
        s2["body"]["value"],
        json!({"expression":"neutral","intensity":0,"remaining_ms":0})
    );
    assert!(s2["body"]["snapshot"].as_u64().unwrap() > s1["body"]["snapshot"].as_u64().unwrap());
}

#[test]
fn admission_failures_use_the_right_error_and_no_outcome() {
    let mut r = Rig::with(Opts {
        bound_c: true,
        ..Opts::default()
    });
    r.connect();
    let ok = json!({"expression":"happy","intensity":1,"duration_ms":0});
    let cases: Vec<(Json, &str)> = vec![
        // unbound coglet
        (
            r.act_body(C, 1, "expression.set_expression", ok.clone(), 5_000_000, 1),
            "permission_denied",
        ),
        // bound but no grant entry
        (
            r.act_body(
                "coglet:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
                2,
                "expression.set_expression",
                ok.clone(),
                5_000_000,
                1,
            ),
            "permission_denied",
        ),
        // stale epoch (too low and too high)
        (
            r.act_body(B, 3, "expression.set_expression", ok.clone(), 5_000_000, 7),
            "permission_denied",
        ),
        // reserved / illustrative / unknown names
        (
            r.act_body(B, 4, "runtime.inbox", json!({}), 5_000_000, 1),
            "permission_denied",
        ),
        (
            r.act_body(B, 5, "test.echo", json!({}), 5_000_000, 1),
            "permission_denied",
        ),
        (
            r.act_body(B, 6, "speech.speak", json!({}), 5_000_000, 1),
            "permission_denied",
        ),
        // granted operation of the wrong direction (event / read used as action)
        (
            r.act_body(
                B,
                7,
                "speech.start",
                json!({"utterance":hid(1),"position_ms":0}),
                5_000_000,
                1,
            ),
            "invalid_argument",
        ),
        (
            r.act_body(B, 8, "expression.state", json!({}), 5_000_000, 1),
            "invalid_argument",
        ),
        // closed args: extra field, wrong type, out-of-range, float-ish, missing
        (
            r.act_body(
                B,
                9,
                "expression.set_expression",
                json!({"expression":"happy","intensity":1,"duration_ms":0,"x":1}),
                5_000_000,
                1,
            ),
            "invalid_argument",
        ),
        (
            r.act_body(
                B,
                10,
                "expression.set_expression",
                json!({"expression":"happy","intensity":1001,"duration_ms":0}),
                5_000_000,
                1,
            ),
            "invalid_argument",
        ),
        (
            r.act_body(
                B,
                11,
                "expression.set_expression",
                json!({"expression":"happy","intensity":"1","duration_ms":0}),
                5_000_000,
                1,
            ),
            "invalid_argument",
        ),
        (
            r.act_body(
                B,
                12,
                "expression.set_expression",
                json!({"expression":"happy"}),
                5_000_000,
                1,
            ),
            "invalid_argument",
        ),
        // deadline: zero is expired, equality is expired, more than 60 s ahead is invalid
        (
            r.act_body(B, 13, "expression.set_expression", ok.clone(), 0, 1),
            "deadline_exceeded",
        ),
        (
            r.act_body(
                B,
                14,
                "expression.set_expression",
                ok.clone(),
                60_000_001,
                1,
            ),
            "invalid_argument",
        ),
    ];
    r.advance(0);
    for (m, want) in cases {
        let out = r.send(&m);
        assert_eq!(out.len(), 1, "rejections have no outcome: {out:?}");
        assert_eq!(accepted(&out[0]), Some(false));
        assert_eq!(code(&out[0]), want, "{}", m["body"]["action"]["capability"]);
    }
    assert!(r.backend.executed().is_empty());
    // exactly 60 s ahead is accepted
    let m = r.act_body(B, 20, "expression.set_expression", ok, 60_000_000, 1);
    assert_eq!(accepted(&r.send(&m)[0]), Some(true));
}

#[test]
fn deadline_equal_to_now_is_expired() {
    let mut r = Rig::new();
    r.connect();
    r.advance(500);
    let m = r.act_body(
        B,
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        500,
        1,
    );
    assert_eq!(code(&r.send(&m)[0]), "deadline_exceeded");
}

#[test]
fn duplicate_conflict_and_receipt_semantics() {
    let mut r = Rig::new();
    r.connect();
    let first = set_expr(&mut r, 1, 500);
    let seq1 = first[1]["body"]["seq"].as_u64().unwrap();
    // identical retry (same bytes): stored accept + identical outcome payload + fresh seq
    let retry = set_expr(&mut r, 1, 500);
    assert_eq!(accepted(&retry[0]), Some(true));
    assert_eq!(retry[1]["body"]["payload"], first[1]["body"]["payload"]);
    assert!(retry[1]["body"]["seq"].as_u64().unwrap() > seq1);
    // the retry with a different clock sample is a *different envelope* (deadline moved)
    r.advance(1);
    let moved = set_expr(&mut r, 1, 500);
    assert_eq!(accepted(&moved[0]), Some(false));
    assert_eq!(code(&moved[0]), "conflict");
    // changed args under the same id: conflict, no execution
    let changed = set_expr(&mut r, 1, 501);
    assert_eq!(code(&changed[0]), "conflict");
    // conflicting envelope never emits an outcome and never executes
    assert_eq!(changed.len(), 1);
    // seq strictly increases across every delivery
    let mut last = seq1;
    for _ in 0..3 {
        let m = r.act_body(
            B,
            1,
            "expression.set_expression",
            json!({"expression":"listening","intensity":500,"duration_ms":1000}),
            5_000_000,
            r.epoch,
        );
        let o = r.send(&m);
        if o.len() == 2 {
            let s = o[1]["body"]["seq"].as_u64().unwrap();
            assert!(s > last);
            last = s;
        }
    }
}

#[test]
fn rejected_retry_returns_stored_rejection_even_if_now_valid() {
    let mut r = Rig::new();
    r.connect();
    // rejected for stale epoch
    let m = r.act_body(
        B,
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        5_000_000,
        7,
    );
    assert_eq!(code(&r.send(&m)[0]), "permission_denied");
    let again = r.send(&m);
    assert_eq!(code(&again[0]), "permission_denied");
    assert_eq!(again.len(), 1);
    // a NEW id with the current epoch succeeds (terminal rejection permits it)
    let out = r.act(
        2,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(accepted(&out[0]), Some(true));
    // changing the envelope for the rejected id is conflict
    let m2 = r.act_body(
        B,
        1,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        5_000_000,
        r.epoch,
    );
    assert_eq!(code(&r.send(&m2)[0]), "conflict");
}

#[test]
fn pending_retry_is_not_executed_twice() {
    let mut r = Rig::new();
    r.connect();
    let args = json!({"stream":hid(50),"chunk":0,"text":"hello","final":true});
    let m = r.act_body(B, 1, "speech.speak_chunk", args, 5_000_000, 1);
    let a = r.send(&m);
    assert_eq!(a.len(), 1);
    assert_eq!(accepted(&a[0]), Some(true));
    let b = r.send(&m);
    assert_eq!(b.len(), 1, "pending retry returns accept only");
    assert_eq!(accepted(&b[0]), Some(true));
    assert_eq!(r.backend.executed().len(), 1);
    // action by another id for the same chunk is conflict, stream is not reusable
    let again = r.act(
        2,
        "speech.speak_chunk",
        json!({"stream":hid(50),"chunk":0,"text":"hello","final":true}),
    );
    assert_eq!(code(&again[0]), "conflict");
}

#[test]
fn unadvertised_operations_fail_closed_and_are_not_granted() {
    // backend implements only expression; grants ask for far more
    let o = Opts {
        ops: &["expression.set_expression", "expression.state"],
        grant_ops: Some(ALL_SPEECH),
        ..Opts::default()
    };
    let mut r = Rig::with(o);
    let (caps, grants) = r.connect();
    let names: Vec<&str> = caps["body"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["expression.control"]);
    let granted = grants["body"]["entries"][0]["operations"]
        .as_array()
        .unwrap();
    assert_eq!(granted.len(), 2);
    assert!(!r.s.restrictions().is_empty());
    let out = r.act(
        1,
        "speech.speak_chunk",
        json!({"stream":hid(5),"chunk":0,"text":"x","final":true}),
    );
    assert_eq!(code(&out[0]), "permission_denied");
    assert!(r.backend.executed().is_empty());
}

#[test]
fn unqualified_optional_operations_are_never_advertised() {
    let mut all: Vec<&str> = ALL_SPEECH.to_vec();
    all.extend([
        "speech.heard",
        "speech.work_state",
        "speech.work_intent",
        "speech.work_done",
        "speech.work_result",
        "llm.complete",
        "runtime.inbox",
        "test.echo",
    ]);
    let o = Opts {
        ops: &all,
        grant_ops: Some(&all[..ALL_SPEECH.len()]),
        ..Opts::default()
    };
    let mut r = Rig::with(o);
    let (caps, _) = r.connect();
    let text = caps.to_string();
    for banned in [
        "speech.heard",
        "speech.work",
        "llm.complete",
        "runtime.inbox",
        "test.echo",
        "speech.history",
        "speech.hands_free",
        "speech.push_to_talk",
    ] {
        assert!(!text.contains(banned), "{banned} must not be advertised");
    }
    let w: Vec<&str> =
        r.s.advertisement()
            .withheld
            .iter()
            .map(|(n, _)| n.as_str())
            .collect();
    for n in [
        "speech.heard",
        "speech.work_state",
        "llm.complete",
        "runtime.inbox",
        "test.echo",
    ] {
        assert!(w.contains(&n), "{n} withheld");
    }
    // a read of the withheld operation is denied/invalid, never served
    let m = json!({"type":"read","body":{"coglet":B,"capability":"speech.heard","args":{"item":hid(1)},"grant_epoch":1,"deadline":5_000_000,"correlation_id":hid(3)}});
    let out = r.send(&m);
    assert_eq!(out[0]["body"]["ok"], false);
}

#[test]
fn reads_check_grants_epoch_args_and_deadline() {
    let mut r = Rig::new();
    r.connect();
    r.advance(1000);
    let cfg = r.read(1, "speech.config", json!({}));
    assert_eq!(cfg["body"]["ok"], true);
    assert_eq!(cfg["body"]["snapshot"], 1);
    assert_eq!(cfg["body"]["value"]["barge"], "stop");
    let mk = |corr, coglet: &str, op: &str, args: Json, epoch, deadline| json!({"type":"read","body":{"coglet":coglet,"capability":op,"args":args,"grant_epoch":epoch,"deadline":deadline,"correlation_id":hid(corr)}});
    let bad = [
        (
            mk(2, C, "speech.config", json!({}), 1, 5_000_000),
            "permission_denied",
        ),
        (
            mk(3, B, "speech.config", json!({}), 9, 5_000_000),
            "permission_denied",
        ),
        (
            mk(4, B, "speech.config", json!({"x":1}), 1, 5_000_000),
            "invalid_argument",
        ),
        (
            mk(5, B, "speech.config", json!({}), 1, 1000),
            "deadline_exceeded",
        ),
        (
            mk(6, B, "speech.config", json!({}), 1, 61_000_001),
            "invalid_argument",
        ),
        (
            mk(7, B, "speech.stop", json!({}), 1, 5_000_000),
            "invalid_argument",
        ),
        (
            mk(8, B, "runtime.receipt", json!({}), 1, 5_000_000),
            "permission_denied",
        ),
    ];
    for (m, want) in bad {
        let out = r.send(&m);
        assert_eq!(out[0]["type"], "read_reply");
        assert_eq!(out[0]["body"]["ok"], false, "{m}");
        assert_eq!(out[0]["body"]["error"]["code"], want, "{m}");
        assert_eq!(
            out[0]["body"]["correlation_id"],
            m["body"]["correlation_id"]
        );
    }
}

#[test]
fn health_cadence_silence_and_clock_rollback() {
    let mut r = Rig::new();
    r.connect();
    assert!(
        r.advance(999_999).iter().all(|m| m["type"] != "health"),
        "no health before 1 s"
    );
    let h = r.advance(1);
    assert_eq!(h.len(), 1);
    assert_eq!(h[0]["body"]["state"], "ready");
    assert_eq!(h[0]["body"]["pending"], 0);
    assert_eq!(h[0]["body"]["clock"], 1_000_000);
    r.health();
    assert!(r.advance(500_000).is_empty());
    assert_eq!(r.advance(500_000).len(), 1);
    // degraded backend reports degraded
    r.backend.set_degraded(true);
    r.health();
    let h = r.advance(1_000_000);
    assert_eq!(h[0]["body"]["state"], "degraded");
    // clock rollback is fatal
    assert!(r.s.tick(r.now - 1).is_err());
    // three seconds of silence is fatal
    let mut r = Rig::new();
    r.connect();
    r.now = 2_999_999;
    assert!(r.s.tick(r.now).is_ok());
    assert!(r.s.tick(3_000_000).is_err());
}

#[test]
fn grant_expiry_revokes_and_bumps_epoch() {
    let mut r = Rig::with(Opts {
        expires: 2_000_000,
        ..Opts::default()
    });
    r.connect();
    // a long-running (pending) action is cancelled when the grant expires
    let m = r.act_body(
        B,
        1,
        "speech.speak_chunk",
        json!({"stream":hid(5),"chunk":0,"text":"x","final":true}),
        1_500_000,
        1,
    );
    assert_eq!(accepted(&r.send(&m)[0]), Some(true));
    let out = r.advance(1_999_999);
    assert!(out.iter().any(|m| status_of(m) == Some("expired")));
    r.health();
    let out = r.advance(1);
    let g = out
        .iter()
        .find(|m| m["type"] == "grants")
        .expect("revoking snapshot published");
    assert_eq!(g["body"]["epoch"], 2);
    assert_eq!(g["body"]["entries"], json!([]));
    assert!(r.backend.revoked().contains(&B.to_string()));
    // old epoch is fenced even for new ids
    let out = r.act(
        2,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(code(&out[0]), "permission_denied");
}

fn status_of(m: &Json) -> Option<&str> {
    (m["type"] == "event")
        .then(|| m["body"]["payload"]["status"].as_str())
        .flatten()
}

#[test]
fn operator_install_and_revoke_fence_before_new_epoch() {
    let mut r = Rig::with(Opts {
        bound_c: true,
        ..Opts::default()
    });
    r.connect();
    let m = r.act_body(
        B,
        1,
        "speech.speak_chunk",
        json!({"stream":hid(5),"chunk":0,"text":"x","final":true}),
        5_000_000,
        1,
    );
    r.send(&m);
    // split-owner duplex and duplicate owners are rejected atomically; nothing changes
    let bad = vec![
        grant(B, &["speech.stop"], &["audio.mic"], 90_000_000),
        grant(C, &["speech.stop"], &["audio.speaker"], 90_000_000),
    ];
    assert!(r.s.install_grants(r.now, bad).is_err());
    assert_eq!(r.s.grant_epoch(), 1);
    assert!(r.backend.revoked().is_empty());
    let dup = vec![
        grant(B, &["speech.stop"], &["audio.speaker"], 90_000_000),
        grant(C, &["speech.stop"], &["audio.speaker"], 90_000_000),
    ];
    assert!(r.s.install_grants(r.now, dup).is_err());
    let expired = vec![grant(B, &["speech.stop"], &["audio.speaker"], r.now)];
    assert!(r.s.install_grants(r.now, expired).is_err());
    let unknown = vec![grant(B, &["runtime.inbox"], &[], 90_000_000)];
    assert!(r.s.install_grants(r.now, unknown).is_err());
    // valid replacement removes B's authority over the face: pending B work cancelled
    let new = vec![grant(
        C,
        &["expression.set_expression"],
        &["display.face"],
        90_000_000,
    )];
    let out: Vec<Json> =
        r.s.install_grants(r.now, new)
            .unwrap()
            .iter()
            .map(|b| decode(b))
            .collect();
    assert_eq!(out.last().unwrap()["type"], "grants");
    assert_eq!(out.last().unwrap()["body"]["epoch"], 2);
    let cancelled: Vec<_> = out
        .iter()
        .filter(|m| status_of(m) == Some("cancelled"))
        .collect();
    assert_eq!(
        cancelled.len(),
        1,
        "pending chunk cancelled before the new epoch is published"
    );
    assert!(r.backend.revoked().contains(&B.to_string()));
    // stale epoch still fenced for B
    let out = r.act(
        3,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(code(&out[0]), "permission_denied");
    // operator revoke of C bumps again
    let out = r.s.revoke(r.now, C).unwrap();
    assert_eq!(decode(out.last().unwrap())["body"]["epoch"], 3);
}

#[test]
fn kill_is_local_latched_and_not_clearable_by_wire() {
    let mut r = Rig::new();
    r.connect();
    let m = r.act_body(
        B,
        1,
        "speech.speak_chunk",
        json!({"stream":hid(5),"chunk":0,"text":"x","final":true}),
        5_000_000,
        1,
    );
    r.send(&m);
    r.advance(100_000);
    let ks = r.s.kill_switch();
    assert!(ks.trigger("operator off-switch"));
    assert!(!ks.trigger("again"), "latch is idempotent");
    assert_eq!(
        r.backend.stop_all_calls(),
        2,
        "native stop path invoked directly, before any wire work"
    );
    let out = r.tick();
    assert_eq!(out[0]["type"], "kill");
    assert_eq!(out[0]["body"]["reason"], "operator off-switch");
    assert_eq!(out[1]["type"], "health");
    assert_eq!(out[1]["body"]["state"], "stopped");
    assert_eq!(out[1]["body"]["pending"], 0);
    assert!(
        out.iter()
            .skip(2)
            .any(|m| status_of(m) == Some("cancelled")),
        "accepted work is cancelled"
    );
    assert!(
        out.iter().all(|m| m["type"] != "grants"),
        "no redundant grants snapshot"
    );
    // acts and reads are denied; wire messages cannot restore authority
    let out = r.act(
        2,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(code(&out[0]), "permission_denied");
    let rd = r.read(9, "speech.config", json!({}));
    assert_eq!(rd["body"]["error"]["code"], "permission_denied");
    assert!(r.health().is_empty());
    assert!(
        r.advance(1_000_000)
            .iter()
            .any(|m| m["type"] == "health" && m["body"]["state"] == "stopped")
    );
    // reset needs a disconnected surface, a fresh connection and a higher epoch
    assert!(r.s.reset_kill().is_err());
    r.s.disconnect(r.now).unwrap();
    r.s.reset_kill().unwrap();
    r.now += 10;
    let (_, g) = r.connect();
    assert_eq!(g["body"]["epoch"], 2);
    assert!(!g["body"]["entries"].as_array().unwrap().is_empty());
    let out = r.act(
        3,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
    );
    assert_eq!(accepted(&out[0]), Some(true));
}

#[test]
fn local_kill_helper_flushes_wire_notification() {
    let mut r = Rig::new();
    r.connect();
    let out = r.s.local_kill(100_000, "button").unwrap();
    let out: Vec<Json> = out.iter().map(|b| decode(b)).collect();
    assert_eq!(out[0]["type"], "kill");
    assert_eq!(out[1]["body"]["state"], "stopped");
}

#[test]
fn outbound_bytes_are_core_deterministic() {
    let mut r = Rig::new();
    let hello = r.s.start().unwrap();
    let reply = encode(
        &json!({"type":"hello_reply","body":{"selection":{"version":"0.1"},"correlation_id":hid(1)}}),
    );
    let mut all = hello;
    all.extend(r.s.receive(&reply, 0).unwrap());
    for b in all {
        let v = coglet_types::decode(RULE, &b).unwrap();
        assert_eq!(coglet_types::encode(RULE, &v).unwrap(), b);
    }
}

#[test]
fn disconnect_cancels_pending_clears_grants_and_requires_new_negotiation() {
    let mut r = Rig::new();
    r.connect();
    let m = r.act_body(
        B,
        1,
        "speech.speak_chunk",
        json!({"stream":hid(5),"chunk":0,"text":"x","final":true}),
        5_000_000,
        1,
    );
    r.send(&m);
    r.s.disconnect(10).unwrap();
    r.s.disconnect(11).unwrap(); // idempotent
    assert!(r.backend.stop_all_calls() >= 1);
    assert!(r.backend.revoked().contains(&B.to_string()));
    assert_eq!(r.s.pending_count(), 0);
    assert!(
        r.s.receive(&encode(&m), 12).is_err(),
        "no operations on a closed link"
    );
    // reconnect: higher epoch; old exact retry yields the historical receipt only
    let mut r2 = r;
    r2.now = 100;
    let (_, g) = r2.connect();
    assert_eq!(g["body"]["epoch"], 2);
    let old = r2.send(&m);
    assert_eq!(accepted(&old[0]), Some(true));
    assert_eq!(status(&old[1]), "cancelled");
    assert_eq!(r2.backend.executed().len(), 1, "never re-executed");
    // a NEW id with the stale epoch is permission_denied
    let m = r2.act_body(
        B,
        2,
        "expression.set_expression",
        json!({"expression":"happy","intensity":1,"duration_ms":0}),
        5_000_100,
        1,
    );
    assert_eq!(code(&r2.send(&m)[0]), "permission_denied");
}

#[test]
fn queue_bound_rejects_with_resource_exhausted() {
    let mut r = Rig::with(Opts {
        max_pending: 3,
        ..Opts::default()
    });
    r.connect();
    let st = hid(77);
    for n in 0..3u64 {
        let m = r.act_body(
            B,
            100 + n,
            "speech.speak_chunk",
            json!({"stream":st,"chunk":n,"text":"x","final":false}),
            5_000_000,
            1,
        );
        assert_eq!(accepted(&r.send(&m)[0]), Some(true), "chunk {n}");
    }
    assert_eq!(r.s.pending_count(), 3);
    let m = r.act_body(
        B,
        999,
        "speech.speak_chunk",
        json!({"stream":st,"chunk":3,"text":"x","final":false}),
        5_000_000,
        1,
    );
    let out = r.send(&m);
    assert_eq!(code(&out[0]), "resource_exhausted");
    assert_eq!(r.s.pending_count(), 3, "refused work is not queued");
}

#[test]
fn start_twice_is_refused() {
    let mut r = Rig::new();
    r.s.start().unwrap();
    assert!(r.s.start().is_err());
}

#[test]
fn adapter_config_fixture_drives_the_surface() {
    use coglet_surface_rust::testing::ScriptedBackend;
    let src = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../spec/fixtures/surface/adapter-config.json"
    ))
    .unwrap();
    let fx = coglet_surface_rust::AdapterFixture::parse(&src, None).expect("fixture parses");
    let dir = tmpdir("fixture");
    let backend = ScriptedBackend::new(
        fx.speech.clone(),
        &[
            "expression.set_expression",
            "expression.state",
            "speech.config",
            "speech.stop",
        ],
    );
    let mut s = coglet_surface_rust::Surface::open(fx.config.clone(), &dir, backend)
        .expect("open from fixture");
    let out = s.start().unwrap();
    assert_eq!(decode(&out[0])["body"]["surface"], fx.config.surface);
}
