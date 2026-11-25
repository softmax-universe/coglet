//! Regression tests for the local component, NOT CI/builder qualification.
mod support;
use coglet_transform::Refuse;
use coglet_transform::admit::{admit_publication, preimage, sign};
use coglet_transform::rewrite::{Pins, rewrite};

#[test]
fn rewrite_refuses_excessive_locals_before_expansion() {
    let wasm = wat::parse_str(format!(
        "(module (func (local {}) f32.const 0 f32.sqrt drop))",
        "f32 ".repeat(5000)
    ))
    .unwrap();
    assert_eq!(rewrite(&wasm, &Pins::default()), Err(Refuse::Limit));
}

#[test]
fn wasi_wrong_signature_is_refused_by_rewrite() {
    let module = wat::parse_str(
        r#"(module (import "wasi_snapshot_preview1" "fd_write" (func (param f64) (result f64))))"#,
    )
    .unwrap();
    assert!(rewrite(&module, &Pins::default()).is_err());
}

#[test]
fn signature_preimage_has_unambiguous_field_boundaries() {
    let (_, _, _, _, mut a) = support::fixture();
    let mut b = a.clone();
    a.publisher = "alice\nrouter=bob".into();
    a.router = "carol".into();
    b.publisher = "alice".into();
    b.router = "bob\nrouter=carol".into();
    assert_ne!(a, b);
    assert_ne!(preimage(&a), preimage(&b));
}

#[test]
fn publication_rejects_unbound_evidence_and_profile_pins() {
    let (key, evidence, allow, now, statement) = support::fixture();
    let good = sign(&key, statement);
    assert_eq!(admit_publication(&good, &allow, &now, &evidence), Ok(()));
    let mut wrong = allow.clone();
    wrong.entries[0].profile_bytes_sha256 = "wrong".into();
    assert_eq!(
        admit_publication(&good, &wrong, &now, &evidence),
        Err(Refuse::Unqualified)
    );
    let mut altered = evidence.clone();
    altered.gate_outputs.push(0);
    assert_eq!(
        admit_publication(&good, &allow, &now, &altered),
        Err(Refuse::BindingMismatch)
    );
}

#[test]
fn all_qualification_pins_are_required() {
    let (key, evidence, allow, now, statement) = support::fixture();
    let good = sign(&key, statement);
    type Change = fn(&mut coglet_transform::GatePins);
    let changes: &[Change] = &[
        |p| p.profile.push('x'),
        |p| p.transform_artifact_sha256.push('x'),
        |p| p.profile_bytes_sha256.push('x'),
        |p| p.engine.push('x'),
        |p| p.compute_mode.push('x'),
        |p| p.arch.push('x'),
        |p| p.exec_mode.push('x'),
        |p| p.transform_source_sha256.push('x'),
        |p| p.toolchain_sha256.push('x'),
        |p| p.dependency_lock_sha256.push('x'),
        |p| p.recipe_sha256.push('x'),
        |p| p.corpus_sha256.push('x'),
        |p| p.gate_outputs_sha256.push('x'),
        |p| p.engine_artifact_sha256.push('x'),
    ];
    for change in changes {
        let mut wrong = allow.clone();
        change(&mut wrong.entries[0]);
        assert_eq!(
            admit_publication(&good, &wrong, &now, &evidence),
            Err(Refuse::Unqualified)
        );
        let mut current = now.clone();
        change(&mut current.qualification);
        assert_eq!(
            admit_publication(&good, &allow, &current, &evidence),
            Err(Refuse::Unqualified)
        );
    }
}

#[test]
fn local_admission_signature_authority_expiry_revocation_and_ref_binding() {
    let (key, evidence, allow, now, statement) = support::fixture();
    let good = sign(&key, statement.clone());
    assert_eq!(admit_publication(&good, &allow, &now, &evidence), Ok(()));
    type Change = fn(&mut coglet_transform::Authority);
    let mismatches: &[Change] = &[
        |a| a.publisher.push('x'),
        |a| a.router.push('x'),
        |a| a.coglet.push('x'),
        |a| a.runtime.push('x'),
        |a| a.source_commit.push('x'),
        |a| a.original_sha256.push('x'),
        |a| a.output_sha256.push('x'),
        |a| a.evidence_sha256.push('x'),
    ];
    for change in mismatches {
        let mut current = now.clone();
        change(&mut current);
        assert_eq!(
            admit_publication(&good, &allow, &current, &evidence),
            Err(Refuse::BindingMismatch)
        );
    }
    let stale: &[Change] = &[
        |a| a.revoked = true,
        |a| a.now_unix = 150,
        |a| a.now_unix = 89,
        |a| a.delegation_expires_at_unix = 100,
        |a| a.delegation_generation += 1,
        |a| a.delegation_digest.push('x'),
    ];
    for change in stale {
        let mut current = now.clone();
        change(&mut current);
        assert_eq!(
            admit_publication(&good, &allow, &current, &evidence),
            Err(Refuse::Stale)
        );
    }
    let races: &[Change] = &[
        |a| a.ref_old_tip.push('x'),
        |a| a.ref_new_tip.push('x'),
        |a| a.ref_generation += 1,
    ];
    for change in races {
        let mut current = now.clone();
        change(&mut current);
        assert_eq!(
            admit_publication(&good, &allow, &current, &evidence),
            Err(Refuse::Race)
        );
    }
    let other = ed25519_dalek::SigningKey::from_bytes(&[8; 32]);
    assert_eq!(
        admit_publication(&sign(&other, statement), &allow, &now, &evidence),
        Err(Refuse::Unauthenticated)
    );
    let mut altered = good.clone();
    altered.statement.publisher.push('x');
    assert_eq!(
        admit_publication(&altered, &allow, &now, &evidence),
        Err(Refuse::Unauthenticated)
    );
    altered = good.clone();
    altered.signature.clear();
    assert_eq!(
        admit_publication(&altered, &allow, &now, &evidence),
        Err(Refuse::Unauthenticated)
    );
    altered = good;
    altered.issuer.clear();
    assert_eq!(
        admit_publication(&altered, &allow, &now, &evidence),
        Err(Refuse::Unauthenticated)
    );
}

#[test]
fn unqualified_transform_publication_refused() {
    let (key, evidence, mut allow, now, statement) = support::fixture();
    let good = sign(&key, statement);
    allow.entries.clear();
    assert_eq!(
        admit_publication(&good, &allow, &now, &evidence),
        Err(Refuse::Unqualified)
    );
    let mut integer = now;
    integer.profile = coglet_transform::INTEGER_PROFILE.into();
    assert!(admit_publication(&good, &allow, &integer, &evidence).is_err());
    assert!(coglet_transform::integer_profile_admitted(
        coglet_transform::INTEGER_PROFILE
    ));
    // Integer builder is not modified or authorized by this component.
}
