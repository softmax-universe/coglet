use coglet_proto::wire::{decode, encode};
use serde_json::Value;

#[test]
fn rejects_all_frozen_invalid_profile_vectors() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../spec/fixtures/common/wire-invalid.json"
    ))
    .unwrap();
    for case in cases {
        assert!(
            decode(&hex::decode(case["cbor_hex"].as_str().unwrap()).unwrap()).is_err(),
            "{}",
            case["name"]
        );
    }
}

#[test]
fn preserves_integer_extremes_and_rejects_float_encoding() {
    for value in [Value::from(u64::MAX), Value::from(i64::MIN)] {
        assert_eq!(decode(&encode(&value).unwrap()).unwrap(), value);
    }
    assert!(encode(&serde_json::json!(1.0)).is_err());
}

#[test]
fn frozen_golden_profile_round_trips() {
    for family in ["common", "hub"] {
        let path = format!(
            "{}/../../spec/fixtures/{family}/cases.json",
            env!("CARGO_MANIFEST_DIR")
        );
        let cases: Vec<Value> =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        for case in cases {
            if case["valid"] != true {
                continue;
            }
            let Some(golden) = case["cbor_hex"].as_str() else {
                continue;
            };
            let bytes = hex::decode(golden).unwrap();
            assert_eq!(encode(&case["value"]).unwrap(), bytes, "{}", case["name"]);
            assert_eq!(decode(&bytes).unwrap(), case["value"], "{}", case["name"]);
        }
    }
}
