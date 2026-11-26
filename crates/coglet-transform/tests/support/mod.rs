//! Local crypto/binding unit fixtures ONLY. Never provision production trust.
use coglet_transform::admit::{
    AdmissionStatement, Allowlist, Authority, Evidence, GatePins, PublicationInputs,
};
use coglet_transform::{PROFILE, profile_bytes, sha256_hex, transform_source_bytes};
pub fn fixture() -> (
    ed25519_dalek::SigningKey,
    Evidence,
    Allowlist,
    Authority,
    AdmissionStatement,
) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let issuer = key.verifying_key().to_bytes();
    let source = transform_source_bytes().to_vec();
    let profile = profile_bytes().to_vec();
    let (raw, trust) = coglet_transform::qualification::test_fixtures::evidence();
    let qualification = coglet_transform::verify_ci_qualification(&raw, &trust).unwrap();
    let files = raw.verified_files(&trust).unwrap();
    let inputs = PublicationInputs {
        source_sha256: sha256_hex(b"fixture source"),
        original_sha256: sha256_hex(b"fixture original"),
        output_sha256: sha256_hex(b"fixture output"),
    };
    let pins = GatePins {
        profile: PROFILE.into(),
        transform_artifact_sha256: qualification.transform_exe_sha256().into(),
        profile_bytes_sha256: sha256_hex(&profile),
        engine: "fixture engine".into(),
        compute_mode: "fixture mode".into(),
        arch: "fixture arch".into(),
        exec_mode: "fixture execution".into(),
        transform_source_sha256: sha256_hex(&source),
        toolchain_sha256: sha256_hex(b"fixture toolchain"),
        dependency_lock_sha256: sha256_hex(b"fixture lock"),
        recipe_sha256: sha256_hex(b"fixture recipe"),
        corpus_sha256: qualification.corpus_sha256().into(),
        gate_outputs_sha256: sha256_hex(b"fixture gates"),
        engine_artifact_sha256: sha256_hex(b"fixture engine artifact"),
    };
    let evidence = Evidence {
        pins: pins.clone(),
        qualification: qualification.clone(),
        inputs: inputs.clone(),
        source: b"fixture source".to_vec(),
        transform_artifact: files["coglet-transform"].clone(),
        transform_source: source,
        profile,
        toolchain: b"fixture toolchain".to_vec(),
        dependency_lock: b"fixture lock".to_vec(),
        recipe: b"fixture recipe".to_vec(),
        corpus: files["fuzz.corpus"].clone(),
        gate_outputs: b"fixture gates".to_vec(),
        publication_gate_outputs: b"fixture publication gates".to_vec(),
        engine_artifact: b"fixture engine artifact".to_vec(),
        original: b"fixture original".to_vec(),
        output: b"fixture output".to_vec(),
    };
    let now = Authority {
        publisher: "p".into(),
        router: "r".into(),
        coglet: "c".into(),
        runtime: "rt".into(),
        source_commit: "fixture commit".into(),
        original_sha256: inputs.original_sha256.clone(),
        output_sha256: inputs.output_sha256.clone(),
        profile: pins.profile.clone(),
        evidence_sha256: evidence.digest(),
        delegation_generation: 1,
        delegation_digest: "delegation".into(),
        ref_old_tip: "old".into(),
        ref_new_tip: "new".into(),
        ref_generation: 1,
        issuer,
        qualification: pins.clone(),
        ci_qualification: qualification,
        inputs,
        now_unix: 100,
        delegation_expires_at_unix: 200,
        revoked: false,
    };
    let statement = AdmissionStatement {
        publisher: now.publisher.clone(),
        router: now.router.clone(),
        coglet: now.coglet.clone(),
        runtime: now.runtime.clone(),
        source_commit: now.source_commit.clone(),
        original_sha256: now.original_sha256.clone(),
        output_sha256: now.output_sha256.clone(),
        profile: now.profile.clone(),
        evidence_sha256: now.evidence_sha256.clone(),
        delegation_generation: now.delegation_generation,
        delegation_digest: now.delegation_digest.clone(),
        ref_old_tip: now.ref_old_tip.clone(),
        ref_new_tip: now.ref_new_tip.clone(),
        ref_generation: now.ref_generation,
        valid_from_unix: 90,
        expires_at_unix: 150,
    };
    (
        key,
        evidence,
        Allowlist {
            issuer,
            entries: vec![pins],
        },
        now,
        statement,
    )
}
