//! Product-neutral runtime. Guest code never receives native surface handles.
mod boundary;
mod execution;
mod filesystem;
mod heard;
mod host;
mod journal;
mod router;
pub use execution::{Engine, EngineReport, MockEngine, Runtime, RuntimeService};
pub use filesystem::{Snapshot, digest, valid_path};
pub use host::{EffectLedger, StepHost};
pub use journal::DurableStore;
pub use router::Router;

use serde_json::Value;

pub type Result<T> = std::result::Result<T, String>;

fn profile_value(value: &Value, depth: usize) -> Result<coglet_types::Value> {
    use coglet_types::Value as Cbor;
    if depth > coglet_types::MAX_DEPTH {
        return Err("profile nesting too deep".into());
    }
    Ok(match value {
        Value::Null => Cbor::Null,
        Value::Bool(value) => Cbor::Bool(*value),
        Value::String(value) => Cbor::Text(value.clone()),
        Value::Number(value) => {
            // Do not pass serde_json::Number through generic Serialize: Cargo
            // feature unification can enable arbitrary_precision, whose private
            // serialization token would otherwise become a CBOR map.
            if let Some(value) = value.as_u64() {
                Cbor::Integer(value.into())
            } else if let Some(value) = value.as_i64() {
                Cbor::Integer(value.into())
            } else {
                return Err("non-profile JSON number".into());
            }
        }
        Value::Array(values) => Cbor::Array(
            values
                .iter()
                .map(|v| profile_value(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Value::Object(values) => Cbor::Map(
            values
                .iter()
                .map(|(key, value)| Ok((Cbor::Text(key.clone()), profile_value(value, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
    })
}

/// Convert a JSON fixture/debug view without losing the profile's uint64 range.
pub fn cbor(value: &Value) -> Result<Vec<u8>> {
    coglet_types::encode_profile(&profile_value(value, 0)?)
}

pub fn validate(rule: &str, value: &Value) -> Result<()> {
    coglet_types::validate(rule, &profile_value(value, 0)?)
}
