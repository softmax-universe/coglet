use coglet_hub::Store;
use serde_json::json;
#[test]
fn acknowledged_queue_restores_capacity_but_keeps_old_batch_receipts() {
    let mut s = Store::open(":memory:").unwrap();
    for batch in 0..4 {
        let events = (0..64)
            .map(|n| json!({"source":"s","seq":batch*64+n+1}))
            .collect::<Vec<_>>();
        s.admit_events("d", "c", batch * 64, &events).unwrap();
    }
    assert!(
        s.admit_events("d", "c", 256, &[json!({"source":"s","seq":257})])
            .is_err()
    );
    let deliveries = s.event_deliveries("c").unwrap();
    assert_eq!(deliveries.len(), 256);
    s.ack_events("c", deliveries.last().unwrap().0).unwrap();
    assert!(s.pending_events("c").unwrap().is_empty());
    let old = (0..64)
        .map(|n| json!({"source":"s","seq":n+1}))
        .collect::<Vec<_>>();
    assert_eq!(s.admit_events("d", "c", 0, &old).unwrap(), 64);
    assert!(s.pending_events("c").unwrap().is_empty());
    assert_eq!(
        s.admit_events("d", "c", 256, &[json!({"source":"s","seq":257})])
            .unwrap(),
        257
    );
    s.ack_events("c", deliveries.last().unwrap().0).unwrap();
    assert_eq!(s.pending_events("c").unwrap().len(), 1);
}
