use crate::{ContractProfile, Result, Snapshot, cbor};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

pub type EffectLedger = BTreeMap<String, BTreeSet<String>>;

/// Per-attempt transactional host. No I/O or dispatch. This entire value is
/// discarded on abort; only diagnostics survive in the immutable step record.
pub struct StepHost {
    pub files: Snapshot,
    pub calls: Vec<Value>,
    pub actions: Vec<Value>,
    pub timers: Vec<Value>,
    pub llm_requests: Vec<Value>,
    pub logs: Vec<Value>,
    pub ledger: EffectLedger,
    identity: Value,
    grants: Value,
    host_limit: u64,
    host_used: u64,
    pub(crate) contract_profile: ContractProfile,
}
impl StepHost {
    pub(crate) fn new(
        files: Snapshot,
        ledger: EffectLedger,
        inputs: &Value,
        contract_profile: ContractProfile,
    ) -> Self {
        Self {
            files,
            ledger,
            calls: vec![],
            actions: vec![],
            timers: vec![],
            llm_requests: vec![],
            logs: vec![],
            identity: inputs["coglet"].clone(),
            grants: inputs["grants"].clone(),
            host_limit: inputs["budgets"]["host_bytes"].as_u64().unwrap(),
            host_used: 0,
            contract_profile,
        }
    }

    /// Copy and validate an import's raw bytes; return codes never come from a
    /// test script. The Wasm engine owns pointer bounds checks before this seam.
    pub fn import(&mut self, op: &str, raw: &[u8]) -> Result<i32> {
        let (schema, kind) = match op {
            "emit" => ("action", "actions"),
            "timer" => ("timer-request", "timers"),
            "llm" => ("llm-request", "llm_requests"),
            "log" => ("log-entry", "logs"),
            _ => return Err(format!("unimplemented host operation: {op}")),
        };
        let argument: Option<Value> =
            crate::profile_codec::decode(self.contract_profile.rule(schema), raw)
                .ok()
                .and_then(|v| v.deserialized().ok());
        let mut result = 0;
        if let Some(argument) = &argument {
            if (op == "emit" && argument["coglet"] != self.identity)
                || (op == "llm" && argument["capability"] != "llm.complete")
                || (matches!(op, "emit" | "llm")
                    && !self
                        .grants
                        .as_array()
                        .ok_or("invalid grants")?
                        .contains(&argument["capability"]))
            {
                result = 2;
            } else if op != "log"
                && self
                    .ledger
                    .get(kind)
                    .is_some_and(|ids| ids.contains(argument["id"].as_str().unwrap()))
            {
                result = 4;
            } else if self
                .host_used
                .checked_add(raw.len() as u64)
                .is_none_or(|used| used > self.host_limit)
            {
                result = 3;
            }
        } else {
            result = 1;
        }
        let call = if let Some(argument) = argument {
            if result == 0 {
                self.host_used += raw.len() as u64;
                if op != "log" {
                    self.ledger
                        .entry(kind.into())
                        .or_default()
                        .insert(argument["id"].as_str().unwrap().into());
                }
                if op == "emit" {
                    self.actions.push(argument.clone());
                } else if op == "timer" {
                    self.timers.push(argument.clone());
                } else if op == "llm" {
                    self.llm_requests.push(argument.clone());
                } else {
                    let mut line = serde_json::to_vec(&argument).map_err(|e| e.to_string())?;
                    line.push(b'\n');
                    self.files.append_host_log(&line);
                    self.logs.push(argument.clone());
                }
            }
            json!({"op":op,"argument":argument,"result":result})
        } else {
            json!({"op":op,"argument_hex":hex::encode(raw),"result":1})
        };
        self.contract_profile.validate("call", &call)?;
        self.calls.push(call);
        Ok(result)
    }

    pub fn guest_write(&mut self, path: &str, bytes: &[u8]) -> Result<()> {
        self.files.guest_write(path, bytes)
    }

    pub fn import_value(&mut self, op: &str, argument: &Value) -> Result<i32> {
        self.import(op, &cbor(argument)?)
    }
}
