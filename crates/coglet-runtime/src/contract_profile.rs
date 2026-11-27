//! Trusted local validation profile, independent of manifest format and wire negotiation.
use crate::{Result, validate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum ContractProfile {
    #[default]
    #[serde(rename = "surface-0.1")]
    Surface01,
    #[serde(rename = "surface-0.2")]
    Surface02,
}

impl FromStr for ContractProfile {
    type Err = String;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "surface-0.1" => Ok(Self::Surface01),
            "surface-0.2" => Ok(Self::Surface02),
            _ => Err("unsupported local contract_profile".into()),
        }
    }
}

impl ContractProfile {
    /// Select only transitive name-bearing contracts; unchanged leaves and
    /// operation-specific bindings retain their frozen grammar. This is local
    /// validation selection, not connection negotiation or registration.
    pub fn rule(self, frozen: &str) -> &str {
        if self == Self::Surface01 {
            return frozen;
        }
        match frozen {
            "capability-name" | "common::capability-name" => "surface02::s02-capability-name",
            "event" | "common::event" => "surface02::s02-event",
            "action" | "common::action" => "surface02::s02-action",
            "call" | "abi::call" => "surface02::s02-call",
            "inbox-message" | "abi::inbox-message" => "surface02::s02-inbox-message",
            "llm-request" | "abi::llm-request" => "surface02::s02-llm-request",
            "manifest" | "abi::manifest" => "surface02::s02-manifest",
            "step-record" | "abi::step-record" => "surface02::s02-step-record",
            "trace" | "abi::trace" => "surface02::s02-trace",
            "database::db-enqueue" => "surface02::s02-db-enqueue",
            "database::db-request" => "surface02::s02-db-request",
            "database::db-host-call" => "surface02::s02-db-host-call",
            "database::db-manifest" => "surface02::s02-db-manifest",
            "database::db-step-record" => "surface02::s02-db-step-record",
            "database::db-trace" => "surface02::s02-db-trace",
            "surface-message" | "surface::surface-message" => "surface02::s02-surface-message",
            "capabilities" | "surface::capabilities" => "surface02::s02-capabilities",
            "cap-entry" | "surface::cap-entry" => "surface02::s02-cap-entry",
            "schema-refs" | "surface::schema-refs" => "surface02::s02-schema-refs",
            "grant" | "surface::grant" => "surface02::s02-grant",
            "grants" | "surface::grants" => "surface02::s02-grants",
            "read" | "surface::read" => "surface02::s02-read",
            "act" | "surface::act" => "surface02::s02-act",
            _ => frozen,
        }
    }
    pub fn validate(self, rule: &str, value: &Value) -> Result<()> {
        validate(self.rule(rule), value)
    }

    pub(crate) fn from_setup(setup: &Value) -> Result<Self> {
        match setup.get("contract_profile") {
            None => Ok(Self::Surface01),
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| e.to_string()),
        }
    }
    /// Local shape validation is not binding registration or wire authority.
    pub fn validate_name(self, name: &str) -> Result<()> {
        validate(
            match self {
                Self::Surface01 => "common::capability-name",
                Self::Surface02 => "surface02::s02-capability-name",
            },
            &Value::String(name.to_owned()),
        )
    }

    /// Select the manifest format first; the profile only selects name rules.
    /// This is structural validation, not capability admission or DB authority.
    pub fn validate_manifest(self, manifest: &Value) -> Result<()> {
        let rule = match (self, manifest["version"].as_str()) {
            (Self::Surface01, Some("1.0")) => "abi::manifest",
            (Self::Surface01, Some("1.1")) => "database::db-manifest",
            (Self::Surface02, Some("1.0")) => "surface02::s02-manifest",
            (Self::Surface02, Some("1.1")) => "surface02::s02-db-manifest",
            // Private historical scope only; the projection maps the version.
            (Self::Surface02, Some("0.1")) if crate::hash_upgrade::historical() => {
                "surface02::s02-manifest"
            }
            (Self::Surface02, Some("0.2")) if crate::hash_upgrade::historical() => {
                "surface02::s02-db-manifest"
            }
            _ => return Err("unsupported manifest format".into()),
        };
        validate(rule, manifest)
    }
}
