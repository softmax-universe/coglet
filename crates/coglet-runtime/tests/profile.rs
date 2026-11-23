use coglet_runtime::cbor;
use serde_json::json;

#[test]
fn json_numbers_remain_integers_with_arbitrary_precision_feature_unification() {
    for number in [json!(10000), json!(u64::MAX), json!(i64::MIN)] {
        let decoded = coglet_types::decode_profile(&cbor(&number).unwrap()).unwrap();
        assert!(
            matches!(decoded, coglet_types::Value::Integer(_)),
            "JSON number became {decoded:?}"
        );
    }
    assert!(cbor(&json!(1.5)).is_err());
}
