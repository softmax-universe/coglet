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
            (Self::Surface01, Some("0.1")) => "abi::manifest",
            (Self::Surface01, Some("0.2")) => "database::db-manifest",
            (Self::Surface02, Some("0.1")) => "surface02::s02-manifest",
            (Self::Surface02, Some("0.2")) => "surface02::s02-db-manifest",
            _ => return Err("unsupported manifest format".into()),
        };
        validate(rule, manifest)
    }
}
