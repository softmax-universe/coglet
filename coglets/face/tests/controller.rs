use ciborium::Value;
use coglet_face::{MAX_SOURCES, MAX_STATE_BYTES, STATE_PATH, wake};
use coglet_guest_rust::{Action, Context, Event, Host, HostError, Operation};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default, Clone)]
struct Memory {
    files: BTreeMap<String, Vec<u8>>,
    actions: Vec<Action>,
}
impl Host for Memory {
    fn call(&mut self, op: Operation, bytes: &[u8]) -> i32 {
        assert_eq!(op, Operation::Emit);
        self.actions.push(
            coglet_types::decode("action", bytes)
                .unwrap()
                .deserialized()
                .unwrap(),
        );
        0
    }
    fn read_file(&mut self, path: &str) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self.files.get(path).cloned())
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> Result<(), HostError> {
        self.files.insert(path.into(), bytes.into());
        Ok(())
    }
}
fn event(kind: &str, seq: u64, utterance: u32, rev: u64, text: &str) -> Event {
    Event {
        source: format!("surface:{}", "b".repeat(64)),
        kind: kind.into(),
        seq,
        logical_time: 100,
        payload: BTreeMap::from([
            ("utterance".into(), Value::Text(format!("{utterance:032x}"))),
            ("revision".into(), Value::Integer(rev.into())),
            ("text".into(), Value::Text(text.into())),
            ("position_ms".into(), Value::Integer(0.into())),
        ]),
        correlation_id: "d".repeat(32),
    }
}
fn run(host: &mut Memory, step: u64, events: &[Event]) -> Result<(), HostError> {
    // Simulate the runtime overlay's atomic publish of all effects and writes.
    let mut overlay = host.clone();
    wake(&mut Context::new(&mut overlay, step, 100, 42), events)?;
    *host = overlay;
    Ok(())
}
fn turn(n: u32, seq: u64, text: &str) -> Vec<Event> {
    vec![
        event("speech.start", seq, n, 0, ""),
        event("speech.end", seq + 1, n, 1, text),
    ]
}
#[test]
fn partial_is_not_input_only_terminal_text_is_echoed() {
    let mut h = Memory::default();
    run(
        &mut h,
        1,
        &[
            event("speech.start", 1, 1, 0, ""),
            event("speech.partial", 2, 1, 1, "wrong"),
        ],
    )
    .unwrap();
    assert!(h.actions.is_empty());
    run(&mut h, 2, &[event("speech.end", 3, 1, 2, "Hello, 世界")]).unwrap();
    assert_eq!(h.actions.len(), 1);
    let a = &h.actions[0];
    assert_eq!(a.capability, "speech.speak_chunk");
    assert_eq!(a.args["text"].as_text(), Some("Hello, 世界"));
    assert_eq!(a.args["chunk"], Value::Integer(0.into()));
    assert_eq!(a.args["final"], Value::Bool(true));
    assert_eq!(a.correlation_id, "d".repeat(32));
    assert!(!String::from_utf8_lossy(&h.files[STATE_PATH]).contains("Hello"));
}
#[test]
fn duplicate_end_and_late_final_do_not_reopen_after_restart() {
    let mut h = Memory::default();
    run(&mut h, 1, &turn(1, 1, "once")).unwrap();
    run(
        &mut h,
        2,
        &[
            event("speech.end", 2, 1, 1, "once"),
            event("speech.end", 3, 1, 2, "changed"),
        ],
    )
    .unwrap();
    run(
        &mut h,
        3,
        &[
            event("speech.start", 4, 2, 0, ""),
            event("speech.end", 5, 1, 3, "late"),
        ],
    )
    .unwrap();
    assert_eq!(h.actions.len(), 1);
    run(&mut h, 4, &[event("speech.end", 6, 2, 1, "new")]).unwrap();
    assert_eq!(h.actions.len(), 2);
}
#[test]
fn empty_final_closes_and_stale_revision_does_not_finish() {
    let mut h = Memory::default();
    run(
        &mut h,
        1,
        &[
            event("speech.start", 1, 1, 0, ""),
            event("speech.partial", 2, 1, 2, "partial"),
            event("speech.end", 3, 1, 1, "stale"),
        ],
    )
    .unwrap();
    assert!(h.actions.is_empty());
    run(
        &mut h,
        2,
        &[
            event("speech.end", 4, 1, 3, "  "),
            event("speech.end", 5, 1, 4, "late"),
        ],
    )
    .unwrap();
    assert!(h.actions.is_empty());
}
#[test]
fn utf8_output_bound_preserves_scalars_and_original_prefix() {
    let mut h = Memory::default();
    let input = "界".repeat(2000);
    run(&mut h, 1, &turn(1, 1, &input)).unwrap();
    let output = h.actions[0].args["text"].as_text().unwrap();
    assert_eq!(output.len(), 4095);
    assert!(input.starts_with(output));
}
#[test]
fn interruption_never_replays_old_text_and_next_turn_uses_fresh_stream() {
    let mut h = Memory::default();
    run(&mut h, 1, &turn(1, 1, "interrupted")).unwrap();
    run(
        &mut h,
        2,
        &[
            event("speech.start", 3, 2, 0, ""),
            event("speech.barge_in", 4, 2, 0, ""),
            event("speech.end", 5, 1, 2, "interrupted"),
        ],
    )
    .unwrap();
    assert_eq!(h.actions.len(), 1);
    run(&mut h, 3, &[event("speech.end", 6, 2, 1, "replacement")]).unwrap();
    assert_eq!(h.actions.len(), 2);
    assert_eq!(h.actions[1].args["text"].as_text(), Some("replacement"));
    assert_ne!(h.actions[0].args["stream"], h.actions[1].args["stream"]);
}
#[test]
fn ids_are_deterministic_and_fresh_for_multiple_turns_and_steps() {
    let mut a = Memory::default();
    let mut b = Memory::default();
    let events = [turn(1, 1, "one"), turn(2, 3, "two")].concat();
    for h in [&mut a, &mut b] {
        run(h, 1, &events).unwrap();
        run(h, 2, &turn(3, 5, "three")).unwrap();
    }
    let ids = |h: &Memory| {
        h.actions
            .iter()
            .flat_map(|a| [a.id.clone(), a.args["stream"].as_text().unwrap().into()])
            .collect::<Vec<String>>()
    };
    assert_eq!(ids(&a), ids(&b));
    assert_eq!(ids(&a).into_iter().collect::<BTreeSet<_>>().len(), 6);
}
#[test]
fn bounded_state_fails_closed_without_evicting_source_fences() {
    let mut h = Memory::default();
    for n in 0..MAX_SOURCES {
        let mut e = event("speech.start", 1, 1, 0, "");
        e.source = format!("surface:{n:064x}");
        run(&mut h, n as u64, &[e]).unwrap();
    }
    assert!(h.files[STATE_PATH].len() <= MAX_STATE_BYTES);
    let previous = h.files.clone();
    let mut e = event("speech.start", 1, 1, 0, "");
    e.source = format!("surface:{:064x}", MAX_SOURCES);
    assert_eq!(run(&mut h, 100, &[e]), Err(HostError::ResourceExhausted));
    assert_eq!(previous, h.files);
}
#[test]
fn reset_context_cannot_reuse_effect_ids_and_many_turns_keep_bounded_state() {
    let mut h = Memory::default();
    run(&mut h, 1, &turn(1, 1, "first")).unwrap();
    assert_eq!(
        run(&mut h, 1, &turn(2, 3, "reset")),
        Err(HostError::Conflict)
    );
    assert_eq!(h.actions.len(), 1);
    for n in 2..=1000 {
        run(&mut h, n as u64, &turn(n, 2 * n as u64 - 1, "next")).unwrap();
        assert!(h.files[STATE_PATH].len() <= MAX_STATE_BYTES);
    }
    assert_eq!(h.actions.len(), 1000);
}

#[test]
fn shipped_manifest_validates_against_generated_abi_schema() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("../manifest.json")).unwrap();
    coglet_types::encode_typed("abi::manifest", &manifest).unwrap();
    assert_eq!(manifest["id"], coglet_face::ID);
    assert_eq!(manifest["budgets"]["memory_pages"], 256);
}

#[test]
fn malformed_or_oversized_state_fails_closed() {
    let mut h = Memory::default();
    h.files
        .insert(STATE_PATH.into(), vec![0; MAX_STATE_BYTES + 1]);
    assert_eq!(
        run(&mut h, 1, &turn(1, 1, "no")),
        Err(HostError::ResourceExhausted)
    );
    h.files.insert(STATE_PATH.into(), b"not json".to_vec());
    assert_eq!(
        run(&mut h, 1, &turn(1, 1, "no")),
        Err(HostError::InvalidArgument)
    );
    assert!(h.actions.is_empty());
}
