use coglet_types_hub0::{Value, validate};
use serde_json::json;

// Deserialize JSON tokens rather than using a generic serde serialization bridge:
// workspace feature unification enables serde_json/arbitrary_precision, whose
// Number serializer exposes a private map instead of an integer to ciborium.
fn profile(json: &serde_json::Value) -> Value {
    serde_json::from_str(&serde_json::to_string(json).unwrap()).unwrap()
}

#[test]
fn shared_database_schema_is_identical() {
    let old: serde_json::Value = serde_json::from_str(coglet_types_hub0::SCHEMA_JSON).unwrap();
    let current: serde_json::Value = serde_json::from_str(current_types::SCHEMA_JSON).unwrap();
    let mut count = 0;
    for (rule, shape) in old.as_object().unwrap() {
        if rule.starts_with("database::") {
            assert_eq!(Some(shape), current.get(rule), "shared rule {rule}");
            count += 1;
        }
    }
    assert!(count > 0, "database namespace must not be silently omitted");
}

#[test]
fn frozen_git_contract_is_strict_and_not_a_current_protocol_alias() {
    let vectors: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/signatures.json")).unwrap();
    let statement = vectors
        .as_array()
        .unwrap()
        .iter()
        .map(|v| &v["statement"])
        .find(|s| s["purpose"] == "logs-ref")
        .unwrap();
    let value = profile(statement);
    validate("hub-ref-statement", &value).unwrap();
    assert!(current_types::validate("hub-ref-statement", &value).is_err());
    let mut extra = statement.clone();
    extra["unknown"] = json!(true);
    assert!(validate("hub-ref-statement", &profile(&extra)).is_err());
    let mut bad = statement.clone();
    bad["new"] = json!("not-a-sha1");
    assert!(validate("hub-ref-statement", &profile(&bad)).is_err());
}

#[test]
fn provenance_revisions_do_not_cross_validate() {
    let old = json!({"version":"0.1", "source_commit":"ab".repeat(20),
        "recipe_sha256":"aa".repeat(32), "policy_sha256":"bb".repeat(32),
        "transform_sha256":"cc".repeat(32), "original_sha256":"dd".repeat(32),
        "rewritten_sha256":"ee".repeat(32)});
    let value = profile(&old);
    validate("hub-build-record", &value).unwrap();
    assert!(current_types::validate("hub-build-record", &value).is_err());
    let mut new = old.clone();
    new.as_object_mut().unwrap().remove("source_commit");
    new["version"] = json!("1.0");
    new["source_tree"] = json!("ab".repeat(32));
    let value = profile(&new);
    current_types::validate("hub-build-record", &value).unwrap();
    assert!(validate("hub-build-record", &value).is_err());
}
