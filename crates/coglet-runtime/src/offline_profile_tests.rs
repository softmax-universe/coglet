//! Explicit, read-only historical-fixture profiling. Not a required timing gate.
use super::*;
use std::io::Read;
use std::time::Instant;

#[test]
#[ignore = "requires an explicitly supplied immutable ordinary-ABI WAL fixture"]
fn profile_retained_reconciliation_read_only() {
    let path = std::env::var_os("COGLET_PROFILE_WAL").expect("COGLET_PROFILE_WAL");
    let mut input = std::fs::File::open(path).unwrap(); // read-only, never DurableStore::open
    let mut previous = Value::Null;
    let mut prior_state: Option<ServiceState> = None;
    let mut records = Vec::new();
    let mut frames = 0;
    let mut reconciled = 0;
    loop {
        let mut header = [0; 4];
        if input.read(&mut header[..1]).unwrap() == 0 {
            break;
        }
        input.read_exact(&mut header[1..]).expect("complete header");
        let n = u32::from_be_bytes(header) as usize;
        assert!(n > 0 && n <= 64 * 1024 * 1024);
        let mut raw = vec![0; n];
        input.read_exact(&mut raw).expect("complete frame");
        let mut transaction: Value = coglet_types::decode_profile(&raw)
            .unwrap()
            .deserialized()
            .unwrap();
        let fields = transaction.as_object_mut().unwrap();
        // This diagnostic intentionally refuses newer database-frame semantics.
        assert_eq!(fields.len(), 4, "ordinary JSON-journal fixture only");
        let hash = fields.remove("hash").unwrap();
        assert_eq!(transaction["previous"], previous);
        let encoded = cbor(&transaction).unwrap();
        assert_eq!(hash, Value::String(digest(&encoded)), "fixture hash chain");
        previous = hash;
        let next: ServiceState = serde_json::from_value(transaction["state"].clone()).unwrap();
        let journal = &transaction["journal"];
        if !journal.is_null() {
            let before = prior_state.as_ref().expect("pre-attempt state");
            assert!(before.runtime.unresolved_attempt.is_some());
            let started = Instant::now();
            let recovered = RuntimeService::reconcile_attempt(before, &records, journal).unwrap();
            let reconcile_ns = started.elapsed().as_nanos();
            // No field exclusions, synthetic monotonic updates or normalization.
            assert_eq!(
                serde_json::to_value(recovered).unwrap(),
                transaction["state"]
            );
            let started = Instant::now();
            let serialized = serde_json::to_value(&next).unwrap();
            let serialize_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            let bytes = cbor(&serialized).unwrap();
            let cbor_ns = started.elapsed().as_nanos();
            let started = Instant::now();
            let _digest = digest(&bytes);
            let digest_ns = started.elapsed().as_nanos();
            eprintln!(
                "offline-profile frame={frames} prior_records={} step={} reconcile_ns={reconcile_ns} state_serialize_ns={serialize_ns} cbor_ns={cbor_ns} digest_ns={digest_ns} state_bytes={}",
                records.len(),
                journal["step"],
                bytes.len()
            );
            records.push(journal.clone());
            reconciled += 1;
        }
        prior_state = Some(next);
        frames += 1;
    }
    assert!(reconciled > 0, "no pure transitions exercised");
    eprintln!(
        "offline-profile complete_frames={frames} reconciled={reconciled} exact_state_equality=true writes=none"
    );
}
