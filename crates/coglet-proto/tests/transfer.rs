use coglet_proto::transfer::{Receiver, Sender};

#[test]
fn sender_waits_for_credit_and_never_exceeds_contiguous_window() {
    let id = "ab".repeat(16);
    let mut tx = Sender::new(&id, 70_000).unwrap();
    assert!(tx.chunk(&[7]).is_err());
    assert!(tx.finish().is_err());
    tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":65536}))
        .unwrap();
    assert!(tx.chunk(&vec![7; 32769]).is_err());
    assert!(tx.chunk(&[]).is_err());
    assert_eq!(tx.chunk(&vec![7; 32768]).unwrap()["offset"], 0);
    assert_eq!(tx.chunk(&vec![7; 32768]).unwrap()["offset"], 32768);
    assert!(tx.chunk(&[7]).is_err());
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":65535}))
            .is_err()
    );
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":"cd".repeat(16),"bytes":70000}))
            .is_err()
    );
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":70001}))
            .is_err()
    );
    tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":70000}))
        .unwrap();
    assert_eq!(tx.chunk(&vec![7; 4464]).unwrap()["offset"], 65536);
    assert_eq!(
        tx.finish().unwrap(),
        json!({"type":"end","correlation_id":id})
    );
    assert!(tx.finish().is_err());
    assert!(tx.chunk(&[7]).is_err());
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":70000}))
            .is_err()
    );
}
use serde_json::json;
use sha2::{Digest, Sha256};

#[test]
fn receiver_end_is_terminal_and_credit_cannot_reopen_it() {
    let id = "ab".repeat(16);
    let hash = hex::encode(Sha256::digest([7]));
    let mut rx = Receiver::new(&id, 1, &hash).unwrap();
    rx.grant(1).unwrap();
    rx.chunk(&json!({"type":"chunk","correlation_id":id,"offset":0,"hex":"07"}))
        .unwrap();
    rx.consumed();
    let end = json!({"type":"end","correlation_id":id});
    rx.finish(&end).unwrap();
    assert!(rx.finish(&end).is_err());
    assert!(rx.grant(1).is_err());
}

#[test]
fn sender_rejects_excessive_initial_window_and_malformed_credit() {
    let id = "ab".repeat(16);
    assert!(Sender::new(&id, 0).is_err());
    assert!(Sender::new(&id, 67108865).is_err());
    assert!(Sender::new("bad", 1).is_err());
    let mut tx = Sender::new(&id, 200_000).unwrap();
    for bytes in [65537u64, u64::MAX] {
        assert!(
            tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":bytes}))
                .is_err()
        );
    }
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":1,"extra":true}))
            .is_err()
    );
    tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":65536}))
        .unwrap();
    tx.chunk(&vec![7; 32768]).unwrap();
    assert!(
        tx.credit(&json!({"type":"credit","correlation_id":id,"bytes":98305}))
            .is_err()
    );
    assert_eq!(tx.available(), 32768);
}

#[test]
fn transfer_requires_contiguous_credited_durable_bytes_and_complete_hash() {
    let id = "ab".repeat(16);
    let bytes = vec![7; 100_000];
    let hash = hex::encode(Sha256::digest(&bytes));
    let mut rx = Receiver::new(&id, bytes.len() as u64, &hash).unwrap();
    assert!(rx.grant(65537).is_err());
    rx.grant(65536).unwrap();
    let chunk = |offset: u64, data: &[u8]| json!({"type":"chunk","correlation_id":id,"offset":offset,"hex":hex::encode(data)});
    assert!(rx.chunk(&chunk(1, &bytes[..32768])).is_err());
    assert_eq!(
        rx.chunk(&chunk(0, &bytes[..32768])).unwrap(),
        &bytes[..32768]
    );
    assert!(rx.grant(65537).is_err()); // not durably consumed yet
    rx.consumed();
    rx.grant(98304).unwrap();
    assert!(rx.grant(98303).is_err());
    rx.chunk(&chunk(32768, &bytes[32768..65536])).unwrap();
    rx.consumed();
    rx.grant(100000).unwrap();
    assert!(rx.chunk(&chunk(65536, &bytes[65536..])).is_err()); // >32KiB chunk
    rx.chunk(&chunk(65536, &bytes[65536..98304])).unwrap();
    rx.chunk(&chunk(98304, &bytes[98304..])).unwrap();
    rx.consumed();
    assert!(rx.chunk(&chunk(100000, &[0])).is_err());
    rx.finish(&json!({"type":"end","correlation_id":id}))
        .unwrap();
}

#[test]
fn transfer_rejects_missing_bytes_bad_digest_and_correlation() {
    let id = "ab".repeat(16);
    assert!(Receiver::new(&id, 0, &"00".repeat(32)).is_err());
    assert!(Receiver::new(&id, 67108865, &"00".repeat(32)).is_err());
    let mut rx = Receiver::new(&id, 1, &"00".repeat(32)).unwrap();
    rx.grant(1).unwrap();
    assert!(
        rx.finish(&json!({"type":"end","correlation_id":id}))
            .is_err()
    );
    assert!(
        rx.chunk(&json!({"type":"chunk","correlation_id":"cd".repeat(16),"offset":0,"hex":"aa"}))
            .is_err()
    );
    rx.chunk(&json!({"type":"chunk","correlation_id":id,"offset":0,"hex":"aa"}))
        .unwrap();
    rx.consumed();
    assert!(
        rx.finish(&json!({"type":"end","correlation_id":id}))
            .is_err()
    );
}
