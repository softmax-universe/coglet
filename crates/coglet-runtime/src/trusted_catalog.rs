//! Explicit supervisor authority. No network resolution and no process-global registration.
use crate::{ContractProfile, Result, catalog, digest, profile_value};
use serde::Deserialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Default)]
pub struct TrustedExtensionCatalog {
    // ADR-0014: in-process only; never parsed from an extension artifact.
    pub(crate) echo_fixture: bool,
    pin: Option<String>,
    profile: ContractProfile,
    artifact: Option<Artifact>,
    ir: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    version: u32,
    namespace: String,
    rules: BTreeMap<String, Value>,
    schemas: BTreeMap<String, String>,
    entries: BTreeMap<String, Vec<String>>,
    operations: BTreeMap<String, Operation>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Operation {
    entry: String,
    direction: String,
    input: String,
    output: Option<String>,
    resources: Vec<String>,
    single_owner: bool,
    selection: Option<Selection>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Selection {
    operation_field: String,
    args_field: String,
    value_field: String,
    reads: Vec<String>,
}
impl TrustedExtensionCatalog {
    pub(crate) fn without_artifact(profile: ContractProfile) -> Self {
        Self {
            profile,
            ..Self::default()
        }
    }
    pub(crate) fn echo_fixture() -> Self {
        Self {
            echo_fixture: true,
            ..Default::default()
        }
    }
    /// Hash the exact bytes, including local schemas. A missing/invalid pin never enables extensions.
    pub fn from_pinned_bytes(raw: &[u8], sha256: &str) -> Result<Self> {
        Self::from_pinned_bytes_with_profile(raw, sha256, ContractProfile::Surface01)
    }
    /// Explicit supervisor-selected profile; never inferred from artifact contents.
    pub fn from_pinned_bytes_with_profile(
        raw: &[u8],
        sha256: &str,
        profile: ContractProfile,
    ) -> Result<Self> {
        if sha256.len() != 64
            || !sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || digest(raw) != sha256
        {
            return Err("trusted extension catalog pin mismatch or absent".into());
        }
        if raw.len() > 1_048_576 {
            return Err("trusted extension catalog exceeds byte limit".into());
        }
        let artifact: Artifact = serde_json::from_slice(raw).map_err(|e| e.to_string())?;
        artifact.validate(profile)?;
        let ir = serde_json::to_string(&artifact.rules).map_err(|e| e.to_string())?;
        Ok(Self {
            echo_fixture: false,
            pin: Some(sha256.to_owned()),
            profile,
            artifact: Some(artifact),
            ir,
        })
    }
    pub(crate) fn pin(&self) -> Option<&str> {
        self.pin.as_deref()
    }
    pub fn contract_profile(&self) -> ContractProfile {
        self.profile
    }
    /// True for a provisioned product operation, in any direction.
    pub(crate) fn is_extension_operation(&self, name: &str) -> bool {
        self.extension(name).is_some()
    }
    /// True for any capability/entry/resource name owned by the provisioned
    /// product namespace, including empty entries and resources that no
    /// operation uses. Local trusted data only; never peer-derived.
    pub(crate) fn is_extension_name(&self, name: &str) -> bool {
        self.artifact.as_ref().is_some_and(|a| {
            a.operations.contains_key(name)
                || a.entries.contains_key(name)
                || a.entries.values().flatten().any(|r| r == name)
                || name.split('.').next() == Some(a.namespace.as_str())
        })
    }
    fn extension(&self, name: &str) -> Option<&Operation> {
        self.artifact.as_ref()?.operations.get(name)
    }
    pub(crate) fn binding(&self, name: &str, direction: &str) -> Result<(String, String)> {
        if let Some(op) = self.extension(name) {
            if op.direction != direction {
                return Err("operation direction mismatch".into());
            }
            return Ok((
                op.entry.clone(),
                if direction == "read" {
                    op.output.as_ref().ok_or("missing output")?.clone()
                } else {
                    op.input.clone()
                },
            ));
        }
        let op = catalog::operation_with_echo(self.echo_fixture, name, direction)?;
        Ok((
            op["entry"].as_str().ok_or("missing entry")?.to_owned(),
            catalog::uri(
                &op[if direction == "read" {
                    "output"
                } else {
                    "input"
                }],
            )?,
        ))
    }
    pub(crate) fn check(
        &self,
        name: &str,
        direction: &str,
        output: bool,
        value: &Value,
    ) -> Result<()> {
        if let Some(op) = self.extension(name) {
            self.binding(name, direction)?;
            let uri = if output {
                op.output.as_ref().ok_or("missing output")?
            } else {
                &op.input
            };
            let rule = &self.artifact.as_ref().unwrap().schemas[uri];
            coglet_types::validate_with_schema(&self.ir, rule, &profile_value(value, 0)?)
        } else {
            catalog::check(
                &catalog::operation_with_echo(self.echo_fixture, name, direction)?
                    [if output { "output" } else { "input" }],
                value,
            )
        }
    }
    pub(crate) fn resources(&self, name: &str) -> Vec<&str> {
        self.extension(name)
            .map(|op| op.resources.iter().map(String::as_str).collect())
            .unwrap_or_else(|| catalog::resources(name).to_vec())
    }
    pub(crate) fn resource_in_entry(&self, entry: &str, resource: &str) -> bool {
        self.artifact
            .as_ref()
            .and_then(|a| a.entries.get(entry))
            .is_some_and(|rs| rs.iter().any(|r| r == resource))
    }
    pub(crate) fn single_owner(&self, name: &str) -> bool {
        self.extension(name).is_some_and(|op| op.single_owner)
    }
    /// Return selected read after exact sibling validation. The caller must intersect BOTH grants.
    pub(crate) fn selected_read<'a>(
        &self,
        name: &str,
        payload: &'a Value,
    ) -> Result<Option<&'a str>> {
        let Some(selection) = self.extension(name).and_then(|op| op.selection.as_ref()) else {
            return Ok(None);
        };
        let read = payload[&selection.operation_field]
            .as_str()
            .ok_or("missing selected operation")?;
        if !selection.reads.iter().any(|r| r == read) {
            return Err("unsupported selected read".into());
        }
        self.check(read, "read", false, &payload[&selection.args_field])?;
        self.check(read, "read", true, &payload[&selection.value_field])?;
        Ok(Some(read))
    }
}
impl Artifact {
    fn validate(&self, profile: ContractProfile) -> Result<()> {
        if self.version != 1
            || self.namespace.is_empty()
            || self.namespace.contains('.')
            || [
                "runtime",
                "speech",
                "expression",
                "llm",
                "action",
                "hub",
                "test",
                "sim",
                "sensor",
                "motor",
                "other",
            ]
            .contains(&self.namespace.as_str())
            || self.operations.is_empty()
            || self.rules.is_empty()
        {
            return Err("unsupported/reserved extension namespace/version".into());
        }
        let prefix = format!("{}.", self.namespace);
        let name = |s: &str| -> Result<()> {
            profile.validate_name(s)?;
            if !s.starts_with(&prefix) {
                return Err("extension name outside namespace".into());
            }
            Ok(())
        };
        for (entry, resources) in &self.entries {
            name(entry)?;
            let mut seen = BTreeSet::new();
            for resource in resources {
                name(resource)?;
                if !seen.insert(resource) {
                    return Err("duplicate entry resource".into());
                }
            }
        }
        for (uri, rule) in &self.schemas {
            if !uri.starts_with("https://")
                || uri.contains("coglet.dev/spec/messages/")
                || uri.contains(char::is_whitespace)
                || !self.rules.contains_key(rule)
            {
                return Err("unknown/reserved schema URI or rule".into());
            }
        }
        for rule in self.rules.keys() {
            closed_ir(
                &self.rules,
                &self.rules[rule],
                &mut BTreeSet::from([rule.clone()]),
                0,
            )?;
        }
        for (operation, op) in &self.operations {
            name(operation)?;
            if catalog::operation(operation, &op.direction).is_ok()
                || !op.single_owner
                || !["event", "read", "action"].contains(&op.direction.as_str())
                || !self.entries.contains_key(&op.entry)
                || !self.schemas.contains_key(&op.input)
                || (op.direction == "event" && op.output.is_some())
                || (op.direction != "event"
                    && op
                        .output
                        .as_ref()
                        .is_none_or(|r| !self.schemas.contains_key(r)))
            {
                return Err("invalid extension operation binding".into());
            }
            let mut seen = BTreeSet::new();
            for resource in &op.resources {
                if !self.entries[&op.entry].contains(resource) || !seen.insert(resource) {
                    return Err("unknown/duplicate operation resource".into());
                }
            }
            if let Some(s) = &op.selection {
                let fields = [&s.operation_field, &s.args_field, &s.value_field];
                if op.direction != "event"
                    || s.reads.is_empty()
                    || fields.iter().any(|f| f.is_empty())
                    || fields.iter().collect::<BTreeSet<_>>().len() != 3
                {
                    return Err("invalid selected-read binding".into());
                }
                let mut seen = BTreeSet::new();
                for read in &s.reads {
                    if !seen.insert(read)
                        || self
                            .operations
                            .get(read)
                            .is_none_or(|r| r.direction != "read")
                    {
                        return Err("unknown selected read".into());
                    }
                }
            }
        }
        // ADR0017 observations cannot be provisioned without exact selected-read validation.
        if self
            .operations
            .get("game.observation")
            .is_some_and(|o| o.selection.is_none())
        {
            return Err("game.observation requires selected read binding".into());
        }
        Ok(())
    }
}

// Closed finite subset of the generated CDDL IR. Unbounded maps/rest and recursive
// references cannot disguise an arbitrary profile-map fallback. Unknown IR is refused at load.
fn closed_ir(
    rules: &BTreeMap<String, Value>,
    e: &Value,
    stack: &mut BTreeSet<String>,
    depth: usize,
) -> Result<()> {
    if depth > 64 {
        return Err("extension schema nesting".into());
    }
    let object = e.as_object().ok_or("invalid IR expression")?;
    let keys: &[&str] = match e["op"].as_str().ok_or("missing IR opcode")? {
        "ref" => {
            let rule = e["name"].as_str().ok_or("missing IR reference")?;
            if !stack.insert(rule.to_owned()) {
                return Err("recursive extension schema".into());
            }
            closed_ir(
                rules,
                rules.get(rule).ok_or("unknown IR reference")?,
                stack,
                depth + 1,
            )?;
            stack.remove(rule);
            &["op", "name"]
        }
        "record" => {
            let mut seen = BTreeSet::new();
            for f in e["fields"].as_array().ok_or("missing IR fields")? {
                if f.as_object().ok_or("invalid field")?.len() != 3
                    || f["optional"].as_bool().is_none()
                    || !seen.insert(f["name"].as_str().ok_or("missing field name")?)
                {
                    return Err("invalid/duplicate IR field".into());
                }
                closed_ir(rules, &f["type"], stack, depth + 1)?;
            }
            &["op", "fields"]
        }
        "union" | "intersection" => {
            let items = e["items"].as_array().ok_or("missing IR alternatives")?;
            if items.is_empty() {
                return Err("empty IR alternatives".into());
            }
            for item in items {
                closed_ir(rules, item, stack, depth + 1)?;
            }
            &["op", "items"]
        }
        "array" => {
            if e["min"]
                .as_u64()
                .zip(e["max"].as_u64())
                .is_none_or(|(a, b)| a > b || b > 262144)
            {
                return Err("unbounded IR array".into());
            }
            closed_ir(rules, &e["item"], stack, depth + 1)?;
            &["op", "min", "max", "item"]
        }
        "primitive" => {
            if !matches!(e["name"].as_str(), Some("tstr" | "bool" | "null")) {
                return Err("unknown primitive".into());
            }
            &["op", "name"]
        }
        "range" | "size" => {
            let bound = |v: &Value| {
                v.as_i64()
                    .map(i128::from)
                    .or_else(|| v.as_u64().map(i128::from))
            };
            let (lo, hi) = (
                bound(&e["min"]).ok_or("invalid IR bounds")?,
                bound(&e["max"]).ok_or("invalid IR bounds")?,
            );
            if lo > hi || (e["op"] == "size" && lo < 0) {
                return Err("invalid IR bounds".into());
            }
            &["op", "min", "max"]
        }
        "literal" => {
            crate::cbor(&e["value"])?;
            &["op", "value"]
        }
        "regexp" => {
            if e["pattern"].as_str().is_none() {
                return Err("missing regexp".into());
            }
            &["op", "pattern"]
        }
        _ => return Err("unsupported/open extension schema IR".into()),
    };
    if object.len() != keys.len() || object.keys().any(|k| !keys.contains(&k.as_str())) {
        return Err("unknown/missing IR field".into());
    }
    Ok(())
}
