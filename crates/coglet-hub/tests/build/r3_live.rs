//! Live authenticated receipt through protected configuration, never test_fixtures.
use super::*;
use coglet_hub::build::R3Config;
use serde_json::Value;
use std::{
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
};
#[path = "r3_guest.rs"]
mod guest;
#[path = "r3_profile.rs"]
mod qualification_profile;

fn profile(f: &Fixture, config: R3Config) -> (Target, Profile) {
    let source = fs::read(f.dir.path().join("guest.wat")).unwrap();
    let manifest = fs::read(f.dir.path().join("manifest.json")).unwrap();
    let output = coglet_transform::rewrite(
        &wat::parse_bytes(&source).unwrap(),
        &coglet_transform::Pins::default(),
    )
    .unwrap()
    .output;
    (
        Target {
            repository: f.dir.path().into(),
            coglet: format!("coglet:{}", iroh::SecretKey::from_bytes(&[7; 32]).public()),
            device: iroh::SecretKey::from_bytes(&[9; 32]).public().to_string(),
        },
        Profile::r3_wat(
            "r3-live-reviewed",
            hash(&source),
            hash(&manifest),
            hash(&output),
            config,
        ),
    )
}

#[test]
fn protected_r3_configuration_publishes_from_real_ci() {
    let out = std::path::PathBuf::from(
        std::env::var_os("COGLET_R3_PUBLICATION_DIR")
            .expect("required proof needs retained output directory"),
    );
    fs::create_dir_all(&out).unwrap();
    assert_eq!(
        std::env::consts::ARCH,
        "x86_64",
        "one native producer; same archive is replayed on both architectures"
    );
    let pins: Value = serde_json::from_str(include_str!("qualified-ci.json")).unwrap();
    let mut trust: coglet_transform::TrustRoot =
        serde_json::from_value(pins["trust"].clone()).unwrap();
    // Installed client path is protected runner configuration, never build input.
    if let Some(client) = std::env::var_os("COGLET_R3_GITHUB_CLIENT") {
        trust.github_client = client.into();
    }
    let mut f = fixture_with(&guest::wat(), None);
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(f.dir.path().join("manifest.json")).unwrap()).unwrap();
    manifest["budgets"]["compute_mode"] = json!("wamr-2.4.3-fast-cumulative-v1");
    // A new signed fixture profile, not a post-admission deadline override.
    manifest["budgets"]["wall_time_us"] = json!(qualification_profile::WALL_TIME_US);
    fs::write(
        f.dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    git(f.dir.path(), &["add", "manifest.json"]);
    git(
        f.dir.path(),
        &["commit", "-m", "protected R3 publication proof"],
    );
    f.request.source_commit = git(f.dir.path(), &["rev-parse", "HEAD"]);
    f.request.profile_id = "r3-live-reviewed".into();
    let secret = iroh::SecretKey::generate();
    let issuer = ed25519_dalek::SigningKey::from_bytes(&secret.to_bytes());
    let secret_path = f.dir.path().join("issuer.secret");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&secret_path)
        .unwrap();
    file.write_all(&secret.to_bytes()).unwrap();
    drop(file);
    let config = R3Config {
        trust,
        run_id: pins["run_id"].as_u64().unwrap(),
        run_attempt: 1,
        issuer_secret_file: secret_path.clone(),
        issuer: hex::encode(issuer.verifying_key().as_bytes()),
        engine_gate_sha256: pins["engine_gates"]["x86_64"].as_str().unwrap().into(),
    };
    let make = |config| {
        let (target, profile) = profile(&f, config);
        Builder::new(target, profile)
    };
    // Each file negative fails even with no usable CI client: path checks precede
    // acquisition, and no request can inject a trusted key/executable.
    let marker = f.dir.path().join("unexpected-client-call");
    let client_probe = f.dir.path().join("client-probe");
    fs::write(
        &client_probe,
        format!("#!/bin/sh\n: > '{}'\nexit 1\n", marker.display()),
    )
    .unwrap();
    fs::set_permissions(&client_probe, fs::Permissions::from_mode(0o500)).unwrap();
    for case in [
        "relative",
        "symlink",
        "mode",
        "length",
        "directory",
        "missing",
    ] {
        let mut bad = config.clone();
        bad.trust.github_client = client_probe.clone();
        let probe = f.dir.path().join(format!("bad-{case}"));
        match case {
            "relative" => bad.issuer_secret_file = "issuer.secret".into(),
            "symlink" => {
                std::os::unix::fs::symlink(&secret_path, &probe).unwrap();
                bad.issuer_secret_file = probe;
            }
            "mode" | "length" => {
                fs::write(
                    &probe,
                    if case == "length" {
                        vec![0; 31]
                    } else {
                        vec![0; 32]
                    },
                )
                .unwrap();
                fs::set_permissions(
                    &probe,
                    fs::Permissions::from_mode(if case == "mode" { 0o644 } else { 0o600 }),
                )
                .unwrap();
                bad.issuer_secret_file = probe;
            }
            "directory" => {
                fs::create_dir(&probe).unwrap();
                bad.issuer_secret_file = probe;
            }
            "missing" => bad.issuer_secret_file = probe,
            _ => unreachable!(),
        }
        assert!(make(bad).is_err(), "issuer file {case}");
        assert!(
            !marker.exists(),
            "issuer file {case} must refuse before any CI acquisition"
        );
    }
    let mut bad = config.clone();
    bad.trust.head_sha = "00".repeat(20);
    assert!(
        make(bad).is_err(),
        "real trusted GitHub response must refuse wrong head"
    );
    // Positive MUST traverse Profile::r3_wat -> Builder::new -> R3Config::load
    // -> live RawCiEvidence::fetch and both authenticated native archives.
    let builder = make(config.clone()).expect("live CI qualification and protected issuer");
    let candidate = builder.compile(&f.request).unwrap();
    let original = wat::parse_str(guest::wat()).unwrap();
    assert_ne!(candidate.wasm(), original);
    let output = candidate.wasm().to_vec();
    let mut s = Signing::new();
    let artifact = s.sign(candidate, || Some(100)).unwrap();
    let archive = admitted_archive(&s, &artifact);
    assert_eq!(
        fs::read(archive.path().join("bin/step.wasm")).unwrap(),
        output
    );
    let q = &artifact.reproduction["evidence"]["ci"];
    assert_eq!(q["run_id"], pins["run_id"]);
    assert_eq!(q["head_sha"], pins["trust"]["head_sha"]);
    fs::create_dir(out.join("evidence")).unwrap();
    for (name, digest) in artifact.reproduction["evidence"]["blobs"]
        .as_object()
        .unwrap()
    {
        let bytes = s
            .store
            .build_evidence(digest.as_str().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(hash(&bytes), digest.as_str().unwrap(), "retained {name}");
        fs::write(out.join("evidence").join(digest.as_str().unwrap()), bytes).unwrap();
    }
    // Reopen custody independently; retrieval neither refreshes nor resigns.
    let retained = Store::open(s.root.path().join("kernel.db"))
        .unwrap()
        .build_artifact(42, &s.request.operation_id)
        .unwrap()
        .unwrap();
    assert_eq!(artifact, retained);
    // Keep the real provenance and every admitted archive file unchanged in the
    // runtime fixture. No synthesized provenance or architecture-specific rewrite.
    let cases: Value =
        serde_json::from_str(include_str!("../../../../conformance/abi/cases.json")).unwrap();
    let mut setup = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone();
    setup["manifest"] = manifest.clone();
    setup["inputs"]["coglet"] = manifest["id"].clone();
    setup["inputs"]["budgets"] = manifest["budgets"].clone();
    setup["inputs"]["grants"] = json!([]);
    setup["initial_files"] =
        json!({"/data/payload.bin":hex::encode(0xfff8_0000_0000_0042u64.to_le_bytes())});
    for name in git(archive.path(), &["ls-tree", "-r", "--name-only", "HEAD"]).lines() {
        setup["initial_files"][format!("/code/{name}")] =
            json!(hex::encode(fs::read(archive.path().join(name)).unwrap()));
    }
    fs::write(
        out.join("runtime-setup.json"),
        serde_json::to_vec(&setup).unwrap(),
    )
    .unwrap();
    fs::write(
        out.join("publication.json"),
        serde_json::to_vec(&artifact).unwrap(),
    )
    .unwrap();
    fs::write(out.join("publication.bundle"), &artifact.bundle).unwrap();
    fs::write(out.join("step.wasm"), output).unwrap();
    // A foreign issuer still cannot authorize this opt-in. This calls the same
    // production entry path, not the synthetic helper used by component tests.
    let mut bad = config;
    bad.issuer = "00".repeat(32);
    assert!(make(bad).is_err(), "foreign admission issuer");
}
