//! Signature primitives. The caller selects authority, never the untrusted signer field.
use ed25519_dalek::{Signature, VerifyingKey};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
#[error("invalid signed statement")]
pub struct InvalidSignature;

pub fn preimage(purpose: &str, statement: &Value) -> Result<Vec<u8>, InvalidSignature> {
    if !matches!(
        purpose,
        "delegation" | "hub-ref" | "published-ref" | "logs-ref"
    ) || statement["purpose"] != purpose
    {
        return Err(InvalidSignature);
    }
    let mut bytes = format!("coglet/hub/0:{purpose}\n").into_bytes();
    bytes.extend(crate::wire::encode(statement).map_err(|_| InvalidSignature)?);
    Ok(bytes)
}

/// Checks closed signed shape, selected purpose, pinned signer and strict Ed25519.
/// Delegation expiry/device binding and ref CAS/watermarks remain admission checks.
pub fn verify(
    signed: &Value,
    purpose: &str,
    expected_signer: &str,
) -> Result<(), InvalidSignature> {
    let rule = if purpose == "delegation" {
        "hub-signed-delegation"
    } else {
        "hub-signed-ref"
    };
    let value = ciborium::Value::serialized(signed).map_err(|_| InvalidSignature)?;
    coglet_types::validate(rule, &value).map_err(|_| InvalidSignature)?;
    if signed["signer"] != expected_signer {
        return Err(InvalidSignature);
    }
    let key: [u8; 32] = hex::decode(expected_signer)
        .map_err(|_| InvalidSignature)?
        .try_into()
        .map_err(|_| InvalidSignature)?;
    let sig: [u8; 64] = hex::decode(signed["signature"].as_str().ok_or(InvalidSignature)?)
        .map_err(|_| InvalidSignature)?
        .try_into()
        .map_err(|_| InvalidSignature)?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| InvalidSignature)?
        .verify_strict(
            &preimage(purpose, &signed["statement"])?,
            &Signature::from_bytes(&sig),
        )
        .map_err(|_| InvalidSignature)
}
