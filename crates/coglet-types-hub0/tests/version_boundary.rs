use coglet_types_hub0::{Value, validate};
use serde_json::json;

// Deserialize JSON tokens rather than using a generic serde serialization bridge:
// workspace feature unification enables serde_json/arbitrary_precision, whose
// Number serializer exposes a private map instead of an integer to ciborium.
fn profile(json: &serde_json::Value) -> Value {
    serde_json::from_str(&serde_json::to_string(json).unwrap()).unwrap()
}

#[test]
fn database_schema_changes_only_its_explicit_record_versions() {
    let old: serde_json::Value = serde_json::from_str(coglet_types_hub0::SCHEMA_JSON).unwrap();
    let current: serde_json::Value = serde_json::from_str(current_types::SCHEMA_JSON).unwrap();
    let versioned = [
        "database::db-manifest",
        "database::db-step-record",
        "database::db-trace",
        "database::db-boundary-record",
    ];
    let mut count = 0;
    let mut changed = 0;
    for (rule, shape) in old.as_object().unwrap() {
        if rule.starts_with("database::") {
            let mut expected = shape.clone();
            if versioned.contains(&rule.as_str()) {
                fn bump_version(shape: &mut serde_json::Value) -> usize {
                    if let Some(fields) = shape.get_mut("fields").and_then(|f| f.as_array_mut()) {
                        let Some(version) = fields.iter_mut().find(|f| f["name"] == "version")
                        else {
                            return 0;
                        };
                        assert_eq!(version["type"], json!({"op":"literal", "value":"0.2"}));
                        version["type"]["value"] = json!("1.1");
                        1
                    } else {
                        // Step records are intersections of record groups.
                        shape["items"]
                            .as_array_mut()
                            .unwrap()
                            .iter_mut()
                            .map(bump_version)
                            .sum()
                    }
                }
                assert!(bump_version(&mut expected) > 0);
                changed += 1;
            }
            assert_eq!(Some(&expected), current.get(rule), "shared rule {rule}");
            count += 1;
        }
    }
    assert_eq!(changed, versioned.len());
    assert_eq!(
        count,
        current
            .as_object()
            .unwrap()
            .keys()
            .filter(|r| r.starts_with("database::"))
            .count()
    );
    assert!(
        count > changed,
        "non-versioned database rules must remain shared"
    );
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
    // ADR-0035 preserves the already-published CBOR manage 1.0 contract;
    // git tree identities therefore arrive in router major 2.0.
    new["version"] = json!("2.0");
    new["source_tree"] = json!("ab".repeat(32));
    let value = profile(&new);
    current_types::validate("hub-build-record", &value).unwrap();
    assert!(validate("hub-build-record", &value).is_err());
    new["version"] = json!("1.0");
    assert!(current_types::validate("hub-build-record", &profile(&new)).is_err());
    assert!(validate("hub-build-record", &profile(&new)).is_err());
}
