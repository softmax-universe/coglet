//! Bridge selected surface roots without unpinning the legacy hub implementation.
use coglet_types::Value;

type Result<T> = std::result::Result<T, String>;

pub(crate) fn validate(rule: &str, value: &Value) -> Result<()> {
    if rule.starts_with("surface02::") {
        // Rust Value identities differ. Deterministic CBOR preserves integers and
        // map keys without serde_json feature-dependent numeric serialization.
        let bytes = coglet_types::encode_profile(value)?;
        let selected = coglet_types_current::decode_profile(&bytes)?;
        coglet_types_current::validate(rule, &selected)
    } else {
        coglet_types::validate(rule, value)
    }
}

pub(crate) fn encode(rule: &str, value: &Value) -> Result<Vec<u8>> {
    validate(rule, value)?;
    coglet_types::encode_profile(value)
}

pub(crate) fn decode(rule: &str, raw: &[u8]) -> Result<Value> {
    let value = coglet_types::decode_profile(raw)?;
    validate(rule, &value)?;
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_family_does_not_unpin_default_or_hub_contracts() {
        let name = Value::Text("softmax.app.state".into());
        assert!(coglet_types::validate("surface02::s02-capability-name", &name).is_err());
        assert!(validate("surface02::s02-capability-name", &name).is_ok());
        assert!(validate("capability-name", &name).is_err());
        assert!(validate("surface02::missing", &name).is_err());
        let old = serde_json::json!({"version":"0.1", "source_commit":"ab".repeat(20),
            "recipe_sha256":"aa".repeat(32), "policy_sha256":"bb".repeat(32),
            "transform_sha256":"cc".repeat(32), "original_sha256":"dd".repeat(32),
            "rewritten_sha256":"ee".repeat(32)});
        let value = crate::profile_value(&old, 0).unwrap();
        assert!(validate("hub-build-record", &value).is_ok());
        let bytes = coglet_types::encode_profile(&value).unwrap();
        assert!(coglet_types_current::decode("hub-build-record", &bytes).is_err());
        let mut extra = old;
        extra["unexpected"] = serde_json::json!(true);
        assert!(
            validate(
                "hub-build-record",
                &crate::profile_value(&extra, 0).unwrap()
            )
            .is_err()
        );
    }

    #[test]
    fn selected_codec_preserves_integer_boundaries_and_closed_validation() {
        let event = serde_json::json!({
            "source":format!("surface:{}", "b".repeat(64)), "kind":"softmax.app.changed", "seq":u64::MAX,
            "logical_time":0, "correlation_id":"1".repeat(32),
            "payload":{"low":i64::MIN,"high":u64::MAX}
        });
        let value = crate::profile_value(&event, 0).unwrap();
        let raw = encode("surface02::s02-event", &value).unwrap();
        assert_eq!(raw, coglet_types::encode_profile(&value).unwrap());
        let decoded = decode("surface02::s02-event", &raw).unwrap();
        assert_eq!(coglet_types::encode_profile(&decoded).unwrap(), raw);
        assert_eq!(decoded.deserialized::<serde_json::Value>().unwrap(), event);
        assert!(decode("event", &raw).is_err());
        let bad = Value::Map(vec![(Value::Text("unexpected".into()), Value::Bool(true))]);
        assert!(encode("surface02::s02-event", &bad).is_err());
    }
}
