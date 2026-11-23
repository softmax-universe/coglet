//! Release-pinned schema selection. Peer-provided URIs are compared, never fetched.
use crate::{Result, validate};
use serde_json::Value;
use std::sync::OnceLock;

fn operations() -> &'static Value {
    static CATALOG: OnceLock<Value> = OnceLock::new();
    &CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../../../spec/capabilities/bindings.json"))
            .expect("built-in catalog JSON")
    })["operations"]
}
pub(crate) fn operation(name: &str, direction: &str) -> Result<&'static Value> {
    let op = operations().get(name).ok_or("unsupported operation")?;
    if op["direction"] != direction {
        return Err("operation direction mismatch".into());
    }
    Ok(op)
}
pub(crate) fn check(rule: &Value, value: &Value) -> Result<()> {
    validate(
        &rule
            .as_str()
            .ok_or("missing catalog rule")?
            .replacen(':', "::", 1),
        value,
    )
}
pub(crate) fn uri(rule: &Value) -> Result<String> {
    let (family, name) = rule
        .as_str()
        .ok_or("missing catalog rule")?
        .split_once(':')
        .ok_or("invalid catalog rule")?;
    Ok(format!(
        "https://coglet.dev/spec/messages/{family}.schema.json#/$defs/{name}"
    ))
}
pub(crate) fn resources(name: &str) -> &'static [&'static str] {
    match name {
        "expression.set_expression" => &["display.face"],
        "speech.speak_chunk" | "speech.stop" | "speech.backchannel" | "speech.position"
        | "speech.heard" => &["audio.speaker"],
        "speech.start" | "speech.partial" | "speech.end" => &["audio.mic"],
        "speech.barge_in"
        | "speech.session_open"
        | "speech.session_close"
        | "speech.tool_call"
        | "speech.tool_result" => &["audio.mic", "audio.speaker"],
        "speech.work_intent" | "speech.work_done" | "speech.work_result" | "speech.work_state" => {
            &["audio.mic", "audio.speaker"]
        }
        _ => &[],
    }
}
