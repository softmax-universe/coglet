//! Pure `r3-scalar-nan-v1` rewrite and local publication admission.
//!
//! No signing keys, network, mutable registry or clock are used by the rewrite.
//! Digest records are not credentials.
pub mod admit;
pub mod oracle;
pub mod rewrite;

pub use admit::{
    AdmissionStatement, Allowlist, Authority, CiBinding, QualifiedPins, SignedAdmission,
    admit_publication, integer_profile_admitted,
};
pub use oracle::{Op, Width, r3_bits};
pub use rewrite::{DigestRecord, Pins, Rewrite, rewrite};

pub const PROFILE: &str = "r3-scalar-nan-v1";
pub const INTEGER_PROFILE: &str = "integer-WAT-v1";
pub const F32_CANON: u32 = 0x7fc0_0000;
pub const F64_CANON: u64 = 0x7ff8_0000_0000_0000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refuse {
    Malformed,
    Limit,
    Unsupported(&'static str),
    Unqualified,
    Unauthenticated,
    BindingMismatch,
    Stale,
    Race,
    Unexecuted(&'static str),
}

impl std::fmt::Display for Refuse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for Refuse {}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(bytes))
}

pub fn profile_bytes() -> &'static [u8] {
    include_bytes!("../PROFILE.md")
}

pub fn transform_artifact_bytes() -> &'static [u8] {
    include_bytes!("rewrite.rs")
}
