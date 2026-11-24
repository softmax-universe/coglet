//! Protected-local admission component, not a CI fetcher or a ref-CAS executor.
//! The caller owns provisioning the allowlist from authenticated CI, sampling
//! current delegation/revocation/time, retaining evidence, and atomic signing/CAS.
//! No function here turns caller-provided CI identity strings into authority.
use crate::{PROFILE, Refuse, sha256_hex};
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
    pub transform_source_sha256: String,
    pub toolchain_sha256: String,
    pub dependency_lock_sha256: String,
    pub recipe_sha256: String,
    pub corpus_sha256: String,
    pub gate_outputs_sha256: String,
    pub engine_artifact_sha256: String,
    pub original_sha256: String,
    pub output_sha256: String,
}

/// Protected configuration. Never deserialize this from a publication request.
/// Entries must be provisioned only after independently authenticating exact CI
/// head/run/job/artifact success; this component does not implement that bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Allowlist {
    pub issuer: [u8; 32],
    pub entries: Vec<QualifiedPins>,
}

/// Actual retained bytes, not hashes of labels. The containing builder must
/// retain these in its existing custody transaction before sign/commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Evidence {
    pub pins: QualifiedPins,
    pub transform_artifact: Vec<u8>,
    pub transform_source: Vec<u8>,
    pub profile: Vec<u8>,
    pub toolchain: Vec<u8>,
    pub dependency_lock: Vec<u8>,
    pub recipe: Vec<u8>,
    pub corpus: Vec<u8>,
    pub gate_outputs: Vec<u8>,
    pub engine_artifact: Vec<u8>,
    pub original: Vec<u8>,
    pub output: Vec<u8>,
}

fn frame(out: &mut Vec<u8>, value: &[u8]) {
    out.extend((value.len() as u64).to_be_bytes());
    out.extend(value);
}

impl Evidence {
    pub fn digest(&self) -> String {
        let p = &self.pins;
        let mut out = b"coglet/transform-evidence/v1\0".to_vec();
        for value in [
            &p.profile,
            &p.transform_artifact_sha256,
            &p.profile_bytes_sha256,
            &p.engine,
            &p.compute_mode,
            &p.arch,
            &p.exec_mode,
            &p.source_commit,
            &p.transform_source_sha256,
            &p.toolchain_sha256,
            &p.dependency_lock_sha256,
            &p.recipe_sha256,
            &p.corpus_sha256,
            &p.gate_outputs_sha256,
            &p.engine_artifact_sha256,
            &p.original_sha256,
            &p.output_sha256,
        ] {
            frame(&mut out, value.as_bytes());
        }
        sha256_hex(&out)
    }

    fn verify(&self) -> Result<(), Refuse> {
        let p = &self.pins;
        if p.profile != PROFILE
            || self.profile != crate::profile_bytes()
            || p.source_commit.is_empty()
            || p.engine.is_empty()
            || p.compute_mode.is_empty()
            || p.arch.is_empty()
            || p.exec_mode.is_empty()
        {
            return Err(Refuse::Unqualified);
        }
        for (bytes, hash) in [
            (&self.transform_artifact, &p.transform_artifact_sha256),
            (&self.transform_source, &p.transform_source_sha256),
            (&self.profile, &p.profile_bytes_sha256),
            (&self.toolchain, &p.toolchain_sha256),
            (&self.dependency_lock, &p.dependency_lock_sha256),
            (&self.recipe, &p.recipe_sha256),
            (&self.corpus, &p.corpus_sha256),
            (&self.gate_outputs, &p.gate_outputs_sha256),
            (&self.engine_artifact, &p.engine_artifact_sha256),
            (&self.original, &p.original_sha256),
            (&self.output, &p.output_sha256),
        ] {
            if bytes.is_empty() || sha256_hex(bytes) != *hash {
                return Err(Refuse::BindingMismatch);
            }
        }
        // These are explicitly source bytes, not a claimed executable artifact.
        if self.transform_source != crate::transform_source_bytes() {
            return Err(Refuse::Unqualified);
        }
        Ok(())
    }
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
    pub valid_from_unix: u64,
    pub expires_at_unix: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedAdmission {
    pub issuer: String,
    pub signature: String,
    pub statement: AdmissionStatement,
}

/// Fresh protected inputs, sampled again under the builder's final authority/CAS
/// fence. Constructing this from an untrusted request would violate the API contract.
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
    pub qualification: QualifiedPins,
    pub now_unix: u64,
    pub delegation_expires_at_unix: u64,
    pub revoked: bool,
}

/// This only identifies the unchanged integer profile; it grants no authority.
pub fn integer_profile_admitted(profile: &str) -> bool {
    profile == crate::INTEGER_PROFILE
}

pub fn preimage(s: &AdmissionStatement) -> Vec<u8> {
    let mut out = b"coglet/transform-admission/v2\0".to_vec();
    for value in [
        &s.publisher,
        &s.router,
        &s.coglet,
        &s.runtime,
        &s.source_commit,
        &s.original_sha256,
        &s.output_sha256,
        &s.profile,
        &s.evidence_sha256,
        &s.delegation_digest,
        &s.ref_old_tip,
        &s.ref_new_tip,
    ] {
        frame(&mut out, value.as_bytes());
    }
    for value in [
        s.delegation_generation,
        s.ref_generation,
        s.valid_from_unix,
        s.expires_at_unix,
    ] {
        out.extend(value.to_be_bytes());
    }
    out
}

pub fn sign(key: &SigningKey, statement: AdmissionStatement) -> SignedAdmission {
    SignedAdmission {
        issuer: hex::encode(key.verifying_key().to_bytes()),
        signature: hex::encode(key.sign(&preimage(&statement)).to_bytes()),
        statement,
    }
}

/// Verifies the local component only. Success is not a signing/CAS capability;
/// callers must perform this check again inside their final atomic fence.
pub fn admit_publication(
    signed: &SignedAdmission,
    allowlist: &Allowlist,
    now: &Authority,
    evidence: &Evidence,
) -> Result<(), Refuse> {
    // No integer-profile bypass: that builder has its own unchanged admission.
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
    if now.revoked
        || now.now_unix >= now.delegation_expires_at_unix
        || s.expires_at_unix <= s.valid_from_unix
        || now.now_unix < s.valid_from_unix
        || now.now_unix >= s.expires_at_unix
    {
        return Err(Refuse::Stale);
    }
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
    if s.delegation_digest != now.delegation_digest
        || s.delegation_generation != now.delegation_generation
    {
        return Err(Refuse::Stale);
    }
    if s.ref_old_tip != now.ref_old_tip
        || s.ref_new_tip != now.ref_new_tip
        || s.ref_generation != now.ref_generation
    {
        return Err(Refuse::Race);
    }
    evidence.verify()?;
    let p = &evidence.pins;
    if *p != now.qualification || !allowlist.entries.contains(p) {
        return Err(Refuse::Unqualified);
    }
    if p.profile != s.profile
        || p.source_commit != s.source_commit
        || p.original_sha256 != s.original_sha256
        || p.output_sha256 != s.output_sha256
        || evidence.digest() != s.evidence_sha256
    {
        return Err(Refuse::BindingMismatch);
    }
    Ok(())
}
