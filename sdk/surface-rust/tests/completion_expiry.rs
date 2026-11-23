mod common;
use coglet_surface_rust::ControlPeer;
use common::*;
use serde_json::json;

fn completion_at(at: u64, grant_expires: u64, expected: &str) {
    let opts = || Opts {
        expires: grant_expires,
        ..Opts::default()
    };
    let mut r = Rig::with(opts());
    r.connect();
    let request = r.act_body(
        B,
        7,
        "speech.speak_chunk",
        json!({
            "stream": hid(50), "chunk": 0, "text": "hello", "final": true
        }),
        100,
        r.epoch,
    );
    assert_eq!(accepted(&r.send(&request)[0]), Some(true));
    r.now = at;
    r.backend.complete(&hid(7), Ok(()));
    let out = r.tick();
    let terminal = outcomes(&out);
    assert_eq!(terminal.len(), 1, "{out:?}");
    assert_eq!(status(terminal[0]), expected);
    let payload = terminal[0]["body"]["payload"].clone();
    if expected == "succeeded" {
        assert!(r.backend.cancelled().is_empty());
    } else {
        assert!(r.backend.cancelled().contains(&hid(7)));
    }
    if expected == "expired" {
        assert_eq!(payload["error"]["code"], "deadline_exceeded");
    }
    if expected == "cancelled" {
        assert!(out.iter().any(|m| m["type"] == "grants"
            && m["body"]["epoch"].as_u64().unwrap() > r.epoch
            && m["body"]["entries"] == json!([])));
    }
    // Late callbacks never emit another terminal result.
    r.backend.complete(&hid(7), Ok(()));
    assert!(outcomes(&r.tick()).is_empty());
    let retry = r.send(&request);
    assert_eq!(accepted(&retry[0]), Some(true));
    assert_eq!(outcomes(&retry).len(), 1);
    assert_eq!(outcomes(&retry)[0]["body"]["payload"], payload);
    assert_eq!(r.backend.executed_ops().len(), 1);
    // The receipt survives a real ledger reopen, even though its epoch is old.
    r.s.disconnect(r.now).unwrap();
    let mut r = r.restart(opts());
    r.connect();
    let retry = r.send(&request);
    assert_eq!(accepted(&retry[0]), Some(true));
    assert_eq!(outcomes(&retry).len(), 1);
    assert_eq!(outcomes(&retry)[0]["body"]["payload"], payload);
    assert!(r.backend.executed_ops().is_empty());
}

#[test]
fn queued_completion_before_deadline_succeeds() {
    completion_at(99, 1000, "succeeded");
}
#[test]
fn queued_completion_at_deadline_expires() {
    completion_at(100, 1000, "expired");
}
#[test]
fn queued_completion_after_deadline_expires() {
    completion_at(101, 1000, "expired");
}
#[test]
fn queued_completion_before_grant_expiry_succeeds() {
    completion_at(49, 50, "succeeded");
}
#[test]
fn queued_completion_at_grant_expiry_is_cancelled() {
    completion_at(50, 50, "cancelled");
}
#[test]
fn queued_completion_after_grant_expiry_is_cancelled() {
    completion_at(51, 50, "cancelled");
}
