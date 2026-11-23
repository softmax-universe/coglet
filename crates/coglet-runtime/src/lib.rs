//! Product-neutral runtime. Guest code never receives native surface handles.
mod boundary;
mod execution;
mod filesystem;
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

/// Convert a JSON fixture/debug view without losing the profile's uint64 range.
pub fn cbor(value: &Value) -> Result<Vec<u8>> {
    let value = coglet_types::Value::serialized(value).map_err(|e| e.to_string())?;
    coglet_types::encode_profile(&value)
}

pub fn validate(rule: &str, value: &Value) -> Result<()> {
    let value = coglet_types::Value::serialized(value).map_err(|e| e.to_string())?;
    coglet_types::validate(rule, &value)
}
