//! Pinned signed-ref metadata, checked before any receive credit is issued.
//! Signature validity is NOT a grant, delegation, verified bundle, or activation.
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefKind {
    Hub,
    Published,
    Logs,
}
impl RefKind {
    pub fn purpose(self) -> &'static str {
        match self {
            Self::Hub => "hub-ref",
            Self::Published => "published-ref",
            Self::Logs => "logs-ref",
        }
    }
    pub fn reference(self) -> &'static str {
        match self {
            Self::Hub => "refs/heads/hub",
            Self::Published => "refs/published/main",
            Self::Logs => "refs/heads/logs",
        }
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum MetadataError {
    #[error("invalid signed Git metadata")]
    Invalid,
    #[error("conflict")]
    Conflict,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation {
    Advance,
    Identical,
}

/// Immutable metadata only. Never use this type as proof of current authority or
/// successful quarantine validation. Recheck both at the application boundary.
#[derive(Clone, Debug)]
pub struct VerifiedRef {
    signed: Value,
}
impl VerifiedRef {
    /// Select the ref kind from the requested operation, and hub/coglet from
    /// trusted routing. The remote `signer` never selects its own authority.
    pub fn verify(
        signed: &Value,
        kind: RefKind,
        hub: &str,
        coglet: &str,
    ) -> Result<Self, MetadataError> {
        let profile = crate::wire::profile(&Value::String(coglet.into()))
            .map_err(|_| MetadataError::Invalid)?;
        coglet_types::validate("coglet-id", &profile).map_err(|_| MetadataError::Invalid)?;
        let signer = match kind {
            RefKind::Hub => hub,
            RefKind::Published | RefKind::Logs => &coglet[7..],
        };
        crate::signing::verify(signed, kind.purpose(), signer)
            .map_err(|_| MetadataError::Invalid)?;
        let statement = &signed["statement"];
        if statement["hub"] != hub
            || statement["coglet"] != coglet
            || statement["ref"] != kind.reference()
        {
            return Err(MetadataError::Invalid);
        }
        Ok(Self {
            signed: signed.clone(),
        })
    }
    /// Pure pre-credit comparison, NOT persistence or transfer acceptance. Use
    /// the latest durable statement and repeat inside the eventual commit CAS.
    /// Even Identical requires revalidation of transfer and current authority.
    pub fn check_observation(&self, current: Option<&Self>) -> Result<Observation, MetadataError> {
        let Some(current) = current else {
            return Ok(Observation::Advance);
        };
        for field in ["hub", "coglet", "ref", "purpose"] {
            if self.signed["statement"][field] != current.signed["statement"][field] {
                return Err(MetadataError::Conflict);
            }
        }
        match self.generation().cmp(&current.generation()) {
            std::cmp::Ordering::Greater => Ok(Observation::Advance),
            std::cmp::Ordering::Equal if self.signed == current.signed => {
                Ok(Observation::Identical)
            }
            _ => Err(MetadataError::Conflict),
        }
    }
    /// Authoritative transition precheck. Neither comparison nor its result is
    /// a lease: repeat against current tip/generation in one durable transaction.
    /// Retain generation tombstones even if the tracked ref has been removed.
    pub fn check_authoritative(
        &self,
        tip: Option<&str>,
        generation: Option<u64>,
    ) -> Result<(), MetadataError> {
        if (tip.is_some() && generation.is_none())
            || self.old_tip() != tip
            || generation.is_some_and(|g| self.generation() <= g)
        {
            return Err(MetadataError::Conflict);
        }
        Ok(())
    }
    pub fn generation(&self) -> u64 {
        self.signed["statement"]["generation"]
            .as_u64()
            .expect("validated uint64")
    }
    pub fn new_tip(&self) -> &str {
        self.signed["statement"]["new"]
            .as_str()
            .expect("validated oid")
    }
    pub fn old_tip(&self) -> Option<&str> {
        self.signed["statement"]["old"].as_str()
    }
    pub fn archive_sha256(&self) -> &str {
        self.signed["statement"]["archive_sha256"]
            .as_str()
            .expect("validated hash")
    }
    pub fn signed(&self) -> &Value {
        &self.signed
    }
}
