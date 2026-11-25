//! End-to-end builder tests. The CI envelope is a DEV-ONLY fixture passed
//! through the real verifier; production rewrite and signed-archive admission
//! execute for real. These are not native cross-architecture CI receipts.
use super::*;
use coglet_hub::build::R3Profile;
use coglet_transform::qualification::test_fixtures::evidence_with_tools;

fn tool_named(name: &str) -> Vec<u8> {
    let exe = std::env::current_exe().unwrap();
    let path = exe.ancestors().map(|p| p.join(name))
        .find(|p| p.is_file()).expect("build the production tool first: cargo build -p coglet-transform --bin coglet-transform");
    fs::read(path).unwrap()
}
fn r3_fixture(wat: &str, wrong_output: bool) -> Fixture {
    let mut f = fixture_with(wat, None);
    let bytes = fs::read(f.dir.path().join("guest.wat")).unwrap();
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(f.dir.path().join("manifest.json")).unwrap()).unwrap();
    manifest["budgets"]["compute_mode"] = json!("wamr-2.4.3-fast-cumulative-v1");
    let manifest = serde_json::to_vec(&manifest).unwrap();
    fs::write(f.dir.path().join("manifest.json"), &manifest).unwrap();
    git(f.dir.path(), &["add", "manifest.json"]);
    git(
        f.dir.path(),
        &["commit", "-m", "pin real native engine mode"],
    );
    f.request.source_commit = git(f.dir.path(), &["rev-parse", "HEAD"]);
    let original = wat::parse_bytes(&bytes).unwrap();
    let output = coglet_transform::rewrite(&original, &coglet_transform::Pins::default())
        .unwrap()
        .output;
    let profile = Profile::integer_wat(
        "r3-reviewed",
        hash(&bytes),
        hash(&manifest),
        if wrong_output {
            "00".repeat(32)
        } else {
            hash(&output)
        },
    );
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
    let (raw, trust) = evidence_with_tools(
        tool_named("coglet-transform"),
        tool_named("coglet-transform-gate"),
    );
    // This explicit engine identity is test provisioning only, not CI qualification.
    let engine = tool_named("coglet-transform-gate");
    let r3 = R3Profile::new(
        &raw,
        &trust,
        issuer.clone(),
        issuer.verifying_key().to_bytes(),
        engine.clone(),
        &hash(&engine),
    )
    .unwrap();
    let target = Target {
        repository: f.dir.path().into(),
        coglet: format!("coglet:{}", iroh::SecretKey::from_bytes(&[7; 32]).public()),
        device: iroh::SecretKey::from_bytes(&[9; 32]).public().to_string(),
    };
    f.builder = Builder::new_r3(target, profile, r3).unwrap();
    f.request.profile_id = "r3-reviewed".into();
    f
}
fn float_wat() -> String {
    WAT.replacen(
        "i32.const 0))",
        "f32.const nan:0x12345 f32.const 1 f32.add drop i32.const 0))",
        1,
    )
}
fn router_archive_admitted(s: &Signing, a: &Artifact) -> bool {
    let metadata = coglet_proto::git_metadata::VerifiedRef::verify(
        &a.signed,
        coglet_proto::git_metadata::RefKind::Hub,
        &s.hub.public().to_string(),
        s.provision.signed["statement"]["coglet"].as_str().unwrap(),
    );
    let Ok(metadata) = metadata else {
        return false;
    };
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        use coglet_proto::transfer::DurableSink;
        let Ok(mut sink) =
            coglet_proto::quarantine::BundleSink::create(dir.path(), a.bundle.len() as u64).await
        else {
            return false;
        };
        for chunk in a.bundle.chunks(16 * 1024) {
            if sink.append(chunk).await.is_err() {
                return false;
            }
        }
        let Ok(envelope) = sink
            .check_envelope(&metadata, &hash(&a.bundle), 100000)
            .await
        else {
            return false;
        };
        envelope
            .validate(&s.validator, coglet_proto::validator::Limits::default())
            .await
            .is_ok()
    })
}
fn admitted_archive(s: &Signing, a: &Artifact) -> TempDir {
    assert!(
        router_archive_admitted(s, a),
        "real signed router archive admission must pass"
    );
    coglet_proto::signing::verify(&a.signed, "hub-ref", &s.hub.public().to_string()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("transfer.bundle"), &a.bundle).unwrap();
    git(dir.path(), &["init", "--initial-branch=hub"]);
    git(dir.path(), &["fetch", "transfer.bundle", "refs/heads/hub"]);
    git(dir.path(), &["checkout", "--detach", "FETCH_HEAD"]);
    let archive = Command::new("/usr/bin/git")
        .current_dir(dir.path())
        .args(["archive", "--format=tar", "HEAD"])
        .output()
        .unwrap();
    assert!(archive.status.success());
    assert_eq!(
        hash(&archive.stdout),
        a.signed["statement"]["archive_sha256"]
    );
    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD"]),
        a.signed["statement"]["new"]
    );
    let bytes = fs::read(dir.path().join("build/provenance.cbor")).unwrap();
    assert_eq!(coglet_proto::wire::decode(&bytes).unwrap(), a.provenance);
    dir
}
#[test]
fn rewrite_before_build_commit() {
    let f = r3_fixture(&float_wat(), false);
    let original = wat::parse_str(float_wat()).unwrap();
    let candidate = f.builder.compile(&f.request).unwrap();
    let output = candidate.wasm().to_vec();
    assert_ne!(original, output, "affected arithmetic must really rewrite");
    assert_eq!(candidate.provenance()["original_sha256"], hash(&original));
    assert_eq!(candidate.provenance()["rewritten_sha256"], hash(&output));
    let mut s = Signing::new();
    let a = s.sign(candidate, || Some(100)).unwrap();
    let archive = admitted_archive(&s, &a);
    assert_eq!(
        fs::read(archive.path().join("bin/step.wasm")).unwrap(),
        output
    );
    let index = &a.reproduction["evidence"]["blobs"];
    for (name, digest) in index.as_object().unwrap() {
        let retained = s
            .store
            .build_evidence(digest.as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(hash(&retained), digest.as_str().unwrap(), "{name}");
    }
    assert_eq!(
        s.store
            .build_evidence(index["original"].as_str().unwrap())
            .unwrap()
            .unwrap(),
        original
    );
    assert_eq!(
        s.store
            .build_evidence(index["rewritten"].as_str().unwrap())
            .unwrap()
            .unwrap(),
        output
    );
    let archived_index: serde_json::Value =
        serde_json::from_slice(&fs::read(archive.path().join("build/evidence.json")).unwrap())
            .unwrap();
    assert_eq!(archived_index, a.reproduction["evidence"]);
    assert!(
        serde_json::to_vec(&a.reproduction).unwrap().len() < 64 * 1024,
        "JSON is an index, never blobs"
    );
    let no_op = r3_fixture(WAT, false);
    let c = no_op.builder.compile(&no_op.request).unwrap();
    assert_eq!(c.wasm(), wat::parse_str(WAT).unwrap());
    assert_eq!(
        c.provenance()["original_sha256"],
        c.provenance()["rewritten_sha256"]
    );
    let wrong = r3_fixture(&float_wat(), true);
    assert!(wrong.builder.compile(&wrong.request).is_err());
}
#[test]
fn authenticated_transform_publication_admission() {
    let f = r3_fixture(&float_wat(), false);
    let mut s = Signing::new();
    let a = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    let _archive = admitted_archive(&s, &a);
    let mut altered = a.clone();
    altered.signed["statement"]["new"] = json!("00".repeat(20));
    assert!(!router_archive_admitted(&s, &altered));
    altered = a.clone();
    altered.signed["signature"] = json!("00".repeat(64));
    assert!(!router_archive_admitted(&s, &altered));
    altered = a.clone();
    *altered.bundle.last_mut().unwrap() ^= 1;
    assert!(!router_archive_admitted(&s, &altered));
    let token = &a.reproduction["admission"];
    let statement = &token["statement"];
    assert_eq!(statement["ref_new_tip"], a.signed["statement"]["new"]);
    assert_eq!(
        statement["runtime"],
        s.provision.signed["statement"]["device"]
    );
    assert_eq!(statement["publisher"], s.hub.public().to_string());
    assert_eq!(statement["output_sha256"], a.provenance["rewritten_sha256"]);
    assert_eq!(
        statement["evidence_sha256"],
        a.reproduction["evidence"]["evidence_sha256"]
    );
    // Original operation retrieval is immutable even after expiry. It is not
    // new effect authority; changed intent conflicts before any signing.
    assert_eq!(
        a,
        s.sign(f.builder.compile(&f.request).unwrap(), || None)
            .unwrap()
    );
    s.request.generation += 1;
    assert!(matches!(
        s.sign(f.builder.compile(&f.request).unwrap(), || Some(100)),
        Err(Error::Conflict(_))
    ));
}
#[test]
fn unqualified_transform_builder_refused() {
    let (raw, mut trust) = evidence_with_tools(
        tool_named("coglet-transform"),
        tool_named("coglet-transform-gate"),
    );
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[31; 32]);
    let engine = tool_named("coglet-transform-gate");
    let make = |trust: &coglet_transform::TrustRoot, allowed| {
        R3Profile::new(
            &raw,
            trust,
            issuer.clone(),
            allowed,
            engine.clone(),
            &hash(&engine),
        )
    };
    assert!(make(&trust, [0; 32]).is_err());
    let foreign = b"self-consistent but not the CI engine".to_vec();
    assert!(
        R3Profile::new(
            &raw,
            &trust,
            issuer.clone(),
            issuer.verifying_key().to_bytes(),
            foreign.clone(),
            &hash(&foreign)
        )
        .is_err()
    );
    trust.head_sha = "b".repeat(40);
    assert!(make(&trust, issuer.verifying_key().to_bytes()).is_err());
    let f = fixture_with(&float_wat(), None);
    assert!(
        f.builder.compile(&f.request).is_err(),
        "integer profile never silently acquires R3"
    );
}
#[test]
fn r3_output_gate_failure_and_changed_admission_issuer_refused() {
    let trap = WAT.replacen("i32.const 0))", "unreachable i32.const 0))", 1);
    let f = r3_fixture(&trap, false);
    assert!(
        f.builder.compile(&f.request).is_err(),
        "native execution failure refuses before signing"
    );
    let mut f = r3_fixture(&float_wat(), false);
    let mut s = Signing::new();
    let a = s
        .sign(f.builder.compile(&f.request).unwrap(), || Some(100))
        .unwrap();
    let issuer = ed25519_dalek::SigningKey::from_bytes(&[32; 32]);
    let (raw, trust) = evidence_with_tools(
        tool_named("coglet-transform"),
        tool_named("coglet-transform-gate"),
    );
    let engine = tool_named("coglet-transform-gate");
    let r3 = R3Profile::new(
        &raw,
        &trust,
        issuer.clone(),
        issuer.verifying_key().to_bytes(),
        engine.clone(),
        &hash(&engine),
    )
    .unwrap();
    let profile = Profile::integer_wat(
        "r3-reviewed",
        hash(&fs::read(f.dir.path().join("guest.wat")).unwrap()),
        hash(&fs::read(f.dir.path().join("manifest.json")).unwrap()),
        a.provenance["rewritten_sha256"].as_str().unwrap().into(),
    );
    let target = Target {
        repository: f.dir.path().into(),
        coglet: s.provision.signed["statement"]["coglet"]
            .as_str()
            .unwrap()
            .into(),
        device: s.provision.signed["statement"]["device"]
            .as_str()
            .unwrap()
            .into(),
    };
    f.builder = Builder::new_r3(target, profile, r3).unwrap();
    assert!(
        matches!(
            s.sign(f.builder.compile(&f.request).unwrap(), || Some(100)),
            Err(Error::Conflict(_))
        ),
        "changed protected issuer is changed intent, not silent artifact retrieval"
    );
    assert_eq!(
        s.store
            .build_artifact(s.request.uid, &s.request.operation_id)
            .unwrap()
            .unwrap(),
        a
    );
}
#[test]
fn r3_authority_races_and_retention_failure_leave_no_signed_artifact() {
    let f = r3_fixture(&float_wat(), false);
    for case in ["revoke", "expiry", "changed-delegation", "retention"] {
        let mut s = Signing::new();
        let authority = s.authority.clone();
        let hub = s.hub.public();
        let coglet = s.provision.signed["statement"]["coglet"]
            .as_str()
            .unwrap()
            .to_owned();
        let db = rusqlite::Connection::open(s.root.path().join("kernel.db")).unwrap();
        if case == "retention" {
            db.execute_batch("CREATE TRIGGER fail_evidence BEFORE INSERT ON build_evidence BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
        }
        let calls = std::cell::Cell::new(0);
        let result = s.sign(f.builder.compile(&f.request).unwrap(),|| {
            let call=calls.get()+1; calls.set(call);
            if call==3 && case=="revoke" { authority.revoke_delegation(hub,&coglet).unwrap(); }
            if call==3 && case=="changed-delegation" {
                let key=iroh::SecretKey::from_bytes(&[7;32]);
                let statement=json!({"purpose":"delegation","hub":hub.to_string(),"coglet":coglet,"device":iroh::SecretKey::from_bytes(&[9;32]).public().to_string(),"generation":2,"expires":1000,"methods":["hub.git.push"]});
                let signature=key.sign(&coglet_proto::signing::preimage("delegation",&statement).unwrap());
                let signed=json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(signature.to_bytes())});
                authority.replace_delegation(hub,iroh::SecretKey::from_bytes(&[9;32]).public(),&coglet,&signed,||Some(100)).unwrap();
            }
            Some(if call>=4 && case=="expiry" {1000} else {100})
        });
        assert!(result.is_err(), "{case}");
        s.absent();
        let count: i64 = db
            .query_row("SELECT count(*) FROM build_evidence", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{case}: evidence must roll back with artifacts");
    }
}
