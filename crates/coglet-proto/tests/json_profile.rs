use coglet_proto::wire;
use serde_json::{Value, json};

#[test]
fn integer_wire_encoding_is_independent_of_serde_feature_unification() {
    // Literal CBOR oracle: never compare two instances of the same bridge.
    assert_eq!(wire::encode(&json!(42)).unwrap(), vec![0x18, 42]);
    assert_eq!(
        wire::encode(&json!(u64::MAX)).unwrap(),
        vec![0x1b, 255, 255, 255, 255, 255, 255, 255, 255]
    );
    assert_eq!(wire::encode(&json!(-1)).unwrap(), vec![0x20]);
    let nested = json!({"numbers":[0, 42, u64::MAX, i64::MIN]});
    assert_eq!(
        wire::decode(&wire::encode(&nested).unwrap()).unwrap(),
        nested
    );
}

#[test]
fn non_profile_json_numbers_are_rejected_without_private_serde_maps() {
    for text in ["1.5", "18446744073709551616", "-9223372036854775809"] {
        let value: Value = serde_json::from_str(text).unwrap();
        assert!(wire::encode(&value).is_err(), "accepted {text}");
    }
}
