//! Explicit JSON-debug-value conversion, independent of serde_json feature flags.
//! `arbitrary_precision` serializes numbers via a private map; that representation
//! MUST NOT become CBOR or weaken integer/profile validation in embedding workspaces.
use coglet_types::Value;
use serde_json::Value as Json;
pub fn to_profile(j: &Json) -> Result<Value, String> {
    Ok(match j {
        Json::Null => Value::Null,
        Json::Bool(v) => Value::Bool(*v),
        Json::String(v) => Value::Text(v.clone()),
        Json::Number(n) => {
            if let Some(v) = n.as_u64() {
                Value::Integer(v.into())
            } else if let Some(v) = n.as_i64() {
                Value::Integer(v.into())
            } else {
                return Err("number outside integer profile".into());
            }
        }
        Json::Array(a) => Value::Array(a.iter().map(to_profile).collect::<Result<_, _>>()?),
        Json::Object(m) => Value::Map(
            m.iter()
                .map(|(k, v)| Ok((Value::Text(k.clone()), to_profile(v)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}
pub fn encode(rule: &str, j: &Json) -> Result<Vec<u8>, String> {
    coglet_types::encode(rule, &to_profile(j)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn integer_precision_and_profile_are_feature_independent() {
        for v in [json!(u64::MAX), json!(i64::MIN), json!(42)] {
            let p = to_profile(&v).unwrap();
            assert!(matches!(p, Value::Integer(_)));
            assert_eq!(
                coglet_types::decode_profile(&coglet_types::encode_profile(&p).unwrap()).unwrap(),
                p
            );
        }
        assert!(to_profile(&json!(1.5)).is_err());
    }
}
