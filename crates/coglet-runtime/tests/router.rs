use coglet_runtime::Router;
use serde_json::{Value, json};

fn event(source: char, seq: u64, time: u64) -> Value {
    json!({"source":format!("surface:{}", source.to_string().repeat(64)),
        "kind":"sensor.tick", "seq":seq, "logical_time":time,
        "payload":{}, "correlation_id":"a".repeat(32)})
}

#[test]
fn batches_by_arrival_not_foreign_clock() {
    let mut router = Router::new(3).unwrap();
    let first = event('1', 0, 500);
    let second = event('2', 0, 1);
    assert_eq!(router.enqueue(first.clone()).unwrap(), 0);
    assert_eq!(router.enqueue(second.clone()).unwrap(), 1);
    assert_eq!(router.batch(1).unwrap(), vec![first]);
    // Events arriving while a batch executes must not enter its pinned prefix.
    router.enqueue(event('1', 1, 501)).unwrap();
    assert_eq!(router.batch(1).unwrap(), vec![event('1', 0, 500)]);
    router.consume(1).unwrap();
    assert_eq!(router.batch(1).unwrap(), vec![second]);
}
