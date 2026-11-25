//! Pure `r3-scalar-nan-v1` rewrite and local publication admission.
//!
//! No signing keys, network, mutable registry or clock are used by the rewrite.
//! Digest records are not credentials.
pub mod admit;
pub mod qualification;
pub use qualification::{
    CiQualification, QualificationError, RawCiEvidence, TrustRoot, verify_ci_qualification,
};
pub mod oracle;
pub mod rewrite;

pub use admit::{
    AdmissionStatement, Allowlist, Authority, Evidence, QualifiedPins, SignedAdmission,
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

/// A source-closure identity only. The actual built tool artifact is a separate
/// mandatory pin in retained publication evidence; source bytes are not it.
pub fn transform_source_bytes() -> &'static [u8] {
    concat!(
        "lib.rs\0",
        include_str!("lib.rs"),
        "\0rewrite.rs\0",
        include_str!("rewrite.rs"),
        "\0qualification.rs\0",
        include_str!("qualification.rs"),
        "\0oracle.rs\0",
        include_str!("oracle.rs"),
        "\0admit.rs\0",
        include_str!("admit.rs"),
        "\0bin/coglet-transform.rs\0",
        include_str!("bin/coglet-transform.rs"),
        "\0Cargo.toml\0",
        include_str!("../Cargo.toml"),
        "\0Cargo.lock\0",
        include_str!("../../../Cargo.lock"),
        "\0PROFILE.md\0",
        include_str!("../PROFILE.md"),
    )
    .as_bytes()
}
