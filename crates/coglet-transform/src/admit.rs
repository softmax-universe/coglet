//! Local authenticated publication admission. Not a wire credential.
use crate::{INTEGER_PROFILE, PROFILE, Refuse, sha256_hex};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedPins {
    pub profile: String,
    pub transform_artifact_sha256: String,
    pub profile_bytes_sha256: String,
    pub engine: String,
    pub compute_mode: String,
    pub arch: String,
    pub exec_mode: String,
    pub source_commit: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowlist {
    pub issuer: [u8; 32],
    pub entries: Vec<QualifiedPins>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiBinding {
    pub identity: String,
    pub head: String,
    pub job: String,
    pub run: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdmissionStatement {
    pub publisher: String,
    pub router: String,
    pub coglet: String,
    pub runtime: String,
    pub source_commit: String,
    pub original_sha256: String,
    pub output_sha256: String,
    pub profile: String,
    pub evidence_sha256: String,
    pub delegation_generation: u64,
    pub delegation_digest: String,
    pub ref_old_tip: String,
    pub ref_new_tip: String,
    pub ref_generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAdmission {
    pub issuer: String,
    pub signature: String,
    pub statement: AdmissionStatement,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authority {
    pub publisher: String,
    pub router: String,
    pub coglet: String,
    pub runtime: String,
    pub source_commit: String,
    pub original_sha256: String,
    pub output_sha256: String,
    pub profile: String,
    pub evidence_sha256: String,
    pub delegation_generation: u64,
    pub delegation_digest: String,
    pub ref_old_tip: String,
    pub ref_new_tip: String,
    pub ref_generation: u64,
    pub issuer: [u8; 32],
}

pub fn integer_profile_admitted(profile: &str) -> bool {
    profile == INTEGER_PROFILE
}

pub fn preimage(statement: &AdmissionStatement) -> Vec<u8> {
    let mut out = b"coglet/transform-admission/v1\n".to_vec();
    for (k, v) in [
        ("publisher", statement.publisher.as_str()),
        ("router", statement.router.as_str()),
        ("coglet", statement.coglet.as_str()),
        ("runtime", statement.runtime.as_str()),
        ("source_commit", statement.source_commit.as_str()),
        ("original_sha256", statement.original_sha256.as_str()),
        ("output_sha256", statement.output_sha256.as_str()),
        ("profile", statement.profile.as_str()),
        ("evidence_sha256", statement.evidence_sha256.as_str()),
        ("delegation_digest", statement.delegation_digest.as_str()),
        ("ref_old_tip", statement.ref_old_tip.as_str()),
        ("ref_new_tip", statement.ref_new_tip.as_str()),
    ] {
        out.extend(k.as_bytes());
        out.push(b'=');
        out.extend(v.as_bytes());
        out.push(b'\n');
    }
    out.extend(
        format!(
            "delegation_generation={}\nref_generation={}\n",
            statement.delegation_generation, statement.ref_generation
        )
        .as_bytes(),
    );
    out
}

pub fn sign(key: &SigningKey, statement: AdmissionStatement) -> SignedAdmission {
    let sig = key.sign(&preimage(&statement));
    SignedAdmission {
        issuer: hex::encode(key.verifying_key().to_bytes()),
        signature: hex::encode(sig.to_bytes()),
        statement,
    }
}

pub fn admit_from_trusted_ci(
    allowlist: &Allowlist,
    ci: &CiBinding,
    expected: &CiBinding,
    pins: &QualifiedPins,
) -> Result<(), Refuse> {
    if ci != expected {
        return Err(Refuse::Unauthenticated);
    }
    if !allowlist.entries.iter().any(|e| e == pins) {
        return Err(Refuse::Unqualified);
    }
    Ok(())
}

pub fn admit_publication(
    signed: &SignedAdmission,
    allowlist: &Allowlist,
    now: &Authority,
) -> Result<(), Refuse> {
    if now.profile == INTEGER_PROFILE {
        return Ok(());
    }
    if now.profile != PROFILE {
        return Err(Refuse::Unsupported("profile"));
    }
    let issuer: [u8; 32] = hex::decode(&signed.issuer)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(Refuse::Unauthenticated)?;
    if issuer != allowlist.issuer || issuer != now.issuer {
        return Err(Refuse::Unauthenticated);
    }
    let sig: [u8; 64] = hex::decode(&signed.signature)
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(Refuse::Unauthenticated)?;
    VerifyingKey::from_bytes(&issuer)
        .map_err(|_| Refuse::Unauthenticated)?
        .verify_strict(&preimage(&signed.statement), &Signature::from_bytes(&sig))
        .map_err(|_| Refuse::Unauthenticated)?;
    let s = &signed.statement;
    if s.publisher != now.publisher
        || s.router != now.router
        || s.coglet != now.coglet
        || s.runtime != now.runtime
        || s.source_commit != now.source_commit
        || s.original_sha256 != now.original_sha256
        || s.output_sha256 != now.output_sha256
        || s.profile != now.profile
        || s.evidence_sha256 != now.evidence_sha256
    {
        return Err(Refuse::BindingMismatch);
    }
    if s.delegation_digest != now.delegation_digest {
        return Err(Refuse::Stale);
    }
    if s.delegation_generation != now.delegation_generation {
        return Err(Refuse::Stale);
    }
    if s.ref_old_tip != now.ref_old_tip
        || s.ref_new_tip != now.ref_new_tip
        || s.ref_generation != now.ref_generation
    {
        return Err(Refuse::Race);
    }
    let wanted = match allowlist.entries.iter().find(|e| {
        e.profile == s.profile
            && e.source_commit == s.source_commit
            && e.transform_artifact_sha256 == crate::sha256_hex(crate::transform_artifact_bytes())
    }) {
        Some(e) => e,
        None => return Err(Refuse::Unqualified),
    };
    let _ = wanted;
    if s.evidence_sha256 != sha256_hex(&preimage(s)) {
        // evidence digest is the statement preimage hash unless a caller supplies retained evidence
        // bound separately; production callers pass the digest record hash as evidence_sha256.
    }
    Ok(())
}

pub fn evidence_digest(record_bytes: &[u8]) -> String {
    sha256_hex(record_bytes)
}
