//! Release-pinned operation catalog (`spec/capabilities/bindings.json`).
//!
//! The catalog is embedded at build time; nothing is ever fetched. Both directions of
//! every operation are validated against the generated `coglet-types` schemas, because
//! a valid generic envelope is not a valid operation (bindings.md).

use serde_json::Value as Json;
use std::collections::BTreeMap;
use std::sync::OnceLock;

const CATALOG_JSON: &str = include_str!("bindings.json");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Event,
    Read,
    Action,
    Gateway,
    /// ADR-0014 trusted harness catalog only; never parsed from production data.
    FixtureEventAction,
}

#[derive(Clone, Debug)]
pub struct Op {
    pub name: String,
    pub entry: Option<String>,
    pub direction: Direction,
    /// `family:rule`
    pub input: String,
    pub output: Option<String>,
}

#[derive(Debug)]
pub struct Catalog {
    ops: BTreeMap<String, Op>,
}

/// `family:rule` (catalog notation) to the generated rule key `family::rule`.
pub fn rule_key(rule: &str) -> String {
    rule.replacen(':', "::", 1)
}

/// Pinned schema URI for an advertised operation (input for events/actions, output for reads).
pub fn schema_uri(rule: &str) -> String {
    let (family, name) = rule.split_once(':').unwrap_or(("", rule));
    format!("https://coglet.dev/spec/messages/{family}.schema.json#/$defs/{name}")
}

pub fn validate_rule(rule: &str, value: &Json) -> Result<(), String> {
    let v = crate::codec::to_profile(value)?;
    coglet_types::validate(&rule_key(rule), &v)
}

pub fn catalog() -> Result<&'static Catalog, String> {
    static C: OnceLock<Result<Catalog, String>> = OnceLock::new();
    C.get_or_init(|| Catalog::parse(CATALOG_JSON))
        .as_ref()
        .map_err(Clone::clone)
}

impl Catalog {
    /// A separate immutable catalog, selected only by the trusted M1 constructor.
    /// No production catalog mutation and no wildcard test operation.
    pub(crate) fn echo_fixture() -> Result<&'static Self, String> {
        static C: OnceLock<Result<Catalog, String>> = OnceLock::new();
        C.get_or_init(|| {
            let mut c = Self::parse(CATALOG_JSON)?;
            c.ops.insert(
                "test.echo".into(),
                Op {
                    name: "test.echo".into(),
                    entry: Some("test.echo".into()),
                    direction: Direction::FixtureEventAction,
                    input: "common:profile-map".into(),
                    output: Some("surface:empty-map".into()),
                },
            );
            Ok(c)
        })
        .as_ref()
        .map_err(Clone::clone)
    }
    fn parse(src: &str) -> Result<Self, String> {
        let j: Json = serde_json::from_str(src).map_err(|e| format!("catalog: {e}"))?;
        if j["version"] != "0.1" {
            return Err("catalog: unsupported version".into());
        }
        let mut ops = BTreeMap::new();
        for (name, o) in j["operations"].as_object().ok_or("catalog: operations")? {
            let direction = match o["direction"].as_str() {
                Some("event") => Direction::Event,
                Some("read") => Direction::Read,
                Some("action") => Direction::Action,
                Some("gateway") => Direction::Gateway,
                _ => return Err(format!("catalog: bad direction for {name}")),
            };
            ops.insert(
                name.clone(),
                Op {
                    name: name.clone(),
                    entry: o["entry"].as_str().map(str::to_owned),
                    direction,
                    input: o["input"].as_str().ok_or("catalog: input")?.to_owned(),
                    output: o["output"].as_str().map(str::to_owned),
                },
            );
        }
        let c = Catalog { ops };
        // Every referenced rule must exist in the generated schemas (fail at startup).
        for op in c.ops.values() {
            for r in std::iter::once(&op.input).chain(op.output.iter()) {
                let key = rule_key(r);
                if coglet_types::validate(&key, &coglet_types::Value::Null)
                    .err()
                    .is_some_and(|e| e.contains("unknown rule"))
                {
                    return Err(format!("catalog: {} references unknown rule {r}", op.name));
                }
            }
        }
        Ok(c)
    }

    pub fn op(&self, name: &str) -> Option<&Op> {
        self.ops.get(name)
    }
    pub fn ops(&self) -> impl Iterator<Item = &Op> {
        self.ops.values()
    }
    pub fn validate_input(&self, op: &str, value: &Json) -> Result<(), String> {
        validate_rule(&self.op(op).ok_or("unknown operation")?.input, value)
    }
    pub fn validate_output(&self, op: &str, value: &Json) -> Result<(), String> {
        match &self.op(op).ok_or("unknown operation")?.output {
            Some(r) => validate_rule(r, value),
            None => Err("operation has no output".into()),
        }
    }
}

/// Exclusive resources an operation requires its caller to own (speech.md / expression.md).
pub fn required_resources(op: &str) -> &'static [&'static str] {
    match op {
        "speech.speak_chunk" | "speech.backchannel" | "speech.stop" | "speech.position" => {
            &["audio.speaker"]
        }
        "speech.start" | "speech.partial" | "speech.end" => &["audio.mic"],
        "speech.barge_in"
        | "speech.session_open"
        | "speech.session_close"
        | "speech.tool_call"
        | "speech.tool_result" => &["audio.mic", "audio.speaker"],
        "expression.set_expression" => &["display.face"],
        _ => &[],
    }
}

/// Operations of the catalog that this SDK core does NOT implement and so never advertises,
/// whatever a backend declares (S-speech/ADR-0010 qualification gates are open).
pub const UNQUALIFIED: &[&str] = &[
    "speech.heard",
    "speech.work_state",
    "speech.work_intent",
    "speech.work_done",
    "speech.work_result",
    "llm.complete",
];

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_pins_rules() {
        let c = catalog().unwrap();
        assert_eq!(c.op("speech.stop").unwrap().direction, Direction::Action);
        assert!(c.op("test.echo").is_none() && c.op("runtime.inbox").is_none());
        assert_eq!(
            schema_uri("speech:stop"),
            "https://coglet.dev/spec/messages/speech.schema.json#/$defs/stop"
        );
    }

    #[test]
    fn closed_records_and_direction() {
        let c = catalog().unwrap();
        let ok = json!({"expression":"happy","intensity":1000,"duration_ms":60000});
        c.validate_input("expression.set_expression", &ok).unwrap();
        for bad in [
            json!({"expression":"happy","intensity":1001,"duration_ms":0}),
            json!({"expression":"happy","intensity":1,"duration_ms":60001}),
            json!({"expression":"angry","intensity":1,"duration_ms":0}),
            json!({"expression":"happy","intensity":1,"duration_ms":0,"x":1}),
            json!({"expression":"happy","intensity":-1,"duration_ms":0}),
            json!({}),
        ] {
            assert!(
                c.validate_input("expression.set_expression", &bad).is_err(),
                "{bad}"
            );
        }
        c.validate_output("speech.stop", &json!({})).unwrap();
        assert!(c.validate_output("speech.stop", &json!({"x":1})).is_err());
        assert!(c.validate_output("speech.start", &json!({})).is_err());
        // The generic root union is not an operation binding.
        assert!(
            c.validate_input("speech.stop", &json!({"stream": "zz"}))
                .is_err()
        );
    }

    #[test]
    fn pinned_copy_matches_spec_when_checked_out() {
        let spec = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../spec/capabilities/bindings.json"
        );
        if let Ok(s) = std::fs::read_to_string(spec) {
            assert_eq!(
                s, CATALOG_JSON,
                "src/bindings.json drifted from spec/capabilities/bindings.json"
            );
        }
    }
}
