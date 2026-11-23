use coglet_proto::wire::{MAX_FRAME, read_frame, write_frame};
use serde_json::{Value, json};

#[tokio::test]
async fn framed_values_round_trip_and_frozen_negatives_fail() {
    let mut bytes = Vec::new();
    write_frame(&mut bytes, &json!({"value": u64::MAX}))
        .await
        .unwrap();
    assert_eq!(
        read_frame(&mut bytes.as_slice()).await.unwrap(),
        json!({"value": u64::MAX})
    );
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../spec/fixtures/hub/framing.json")).unwrap();
    for case in cases {
        let bytes = hex::decode(case["hex"].as_str().unwrap()).unwrap();
        let mut source = bytes.as_slice();
        let result = read_frame(&mut source).await;
        // Trailing bytes are an operation-order violation (not part of this frame).
        if case["name"] == "trailing-frame" {
            assert!(result.is_ok());
            assert!(!source.is_empty());
        } else {
            assert!(result.is_err(), "{}", case["name"]);
        }
    }
    assert!(
        write_frame(&mut Vec::new(), &json!("a".repeat(MAX_FRAME)))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn oversize_is_rejected_without_reading_or_allocating_body() {
    let (mut tx, mut rx) = tokio::io::duplex(8);
    use tokio::io::AsyncWriteExt;
    tx.write_all(&((MAX_FRAME + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    let result =
        tokio::time::timeout(std::time::Duration::from_millis(100), read_frame(&mut rx)).await;
    assert!(result.unwrap().is_err()); // writer stays open without providing body
}
