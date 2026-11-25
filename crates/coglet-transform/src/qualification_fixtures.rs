//! Synthetic hermetic CI data. This module is not a production trust source.
//! Downstream crates enable `test-fixtures` ONLY in dev-dependencies.
use super::*;
use serde_json::json;
use std::io::Write;

fn fixture() -> (RawCiEvidence, TrustRoot) {
    fixture_with_executable(None)
}

/// Synthetic CI envelope with real selected-host executable bytes, for tests
/// that execute that exact tool. The other architecture remains a stub: this
/// never claims real CI custody or cross-architecture execution.
pub fn evidence_with_executable(bytes: Vec<u8>) -> (RawCiEvidence, TrustRoot) {
    let (raw, trust) = fixture_with_executable(Some(bytes));
    verify_ci_qualification(&raw, &trust).unwrap();
    (raw, trust)
}

fn fixture_with_executable(executable: Option<Vec<u8>>) -> (RawCiEvidence, TrustRoot) {
    let head = "a".repeat(40);
    let repo = json!({"id": REPO_ID, "full_name": REPO, "fork":false});
    let run = json!({"repository":repo,"head_repository":repo,"head_sha":head,"path":WORKFLOW,"workflow_id":WORKFLOW_ID,"id":7,"run_attempt":2,"status":"completed","conclusion":"success","event":"push"});
    let jobs = vec![
        json!({"name":"rust","run_id":7,"run_attempt":2,"head_sha":head,"status":"completed","conclusion":"success"}),
        json!({"name":"transform-native (ubuntu-24.04, x86_64)","run_id":7,"run_attempt":2,"head_sha":head,"status":"completed","conclusion":"success"}),
        json!({"name":"transform-native (ubuntu-24.04-arm, aarch64)","run_id":7,"run_attempt":2,"head_sha":head,"status":"completed","conclusion":"success"}),
    ];
    let mut artifacts = Vec::new();
    let mut pins = BTreeMap::new();
    let mut blobs = BTreeMap::new();
    for arch in ["x86_64", "aarch64"] {
        let mut exe = vec![0u8; 64];
        exe[..6].copy_from_slice(b"\x7fELF\x02\x01");
        exe[18..20].copy_from_slice(if arch == "x86_64" {
            &[62, 0]
        } else {
            &[183, 0]
        });
        if arch == std::env::consts::ARCH
            && let Some(bytes) = &executable
        {
            exe = bytes.clone();
        }
        let mut files = BTreeMap::from([
            ("transform.source", crate::transform_source_bytes().to_vec()),
            ("coglet-transform", exe),
            ("Cargo.lock", b"fixture lock".to_vec()),
            ("rustc.txt", b"fixture compiler".to_vec()),
            ("fuzz.corpus", b"fixture corpus".to_vec()),
            ("fuzz.results", vec![0; 960 * 80]),
        ]);
        let hashes: BTreeMap<_, _> = files.iter().map(|(n, b)| (*n, sha256_hex(b))).collect();
        let manifest = json!({"actual_checkout":head,"github_run":"7","github_attempt":"2","arch":arch,"mode":"native","production_tool":"coglet-transform","case_count":960,"files":hashes});
        files.insert("manifest.json", serde_json::to_vec(&manifest).unwrap());
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, data) in files {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(&data).unwrap();
        }
        let bytes = zip.finish().unwrap().into_inner();
        let archive_sha256 = sha256_hex(&bytes);
        let name = format!("transform-native-{arch}-normal");
        pins.insert(
            name.clone(),
            ArtifactPin {
                archive_sha256: archive_sha256.clone(),
                arch: arch.into(),
                transform_exe_sha256: hashes["coglet-transform"].clone(),
                corpus_sha256: hashes["fuzz.corpus"].clone(),
                results_sha256: hashes["fuzz.results"].clone(),
                lock_sha256: hashes["Cargo.lock"].clone(),
                compiler_sha256: hashes["rustc.txt"].clone(),
            },
        );
        artifacts.push(json!({"id":artifacts.len()+1,"name":name,"expired":false,"digest":format!("sha256:{archive_sha256}"),"workflow_run":{"id":7,"head_sha":head,"repository_id":REPO_ID,"head_repository_id":REPO_ID}}));
        blobs.insert(format!("artifact/{name}.zip"), bytes);
    }
    let trust = TrustRoot {
        github_client: PathBuf::from("/fixture/not-a-production-client"),
        head_sha: head,
        required_jobs: jobs
            .iter()
            .map(|j| j["name"].as_str().unwrap().to_owned())
            .collect(),
        artifacts: pins,
        target_artifact: format!("transform-native-{}-normal", std::env::consts::ARCH),
    };
    (
        RawCiEvidence {
            run,
            jobs,
            artifacts,
            blobs,
        },
        trust,
    )
}
/// Hermetic unit-fixture qualification, not usable as a production trust root.
/// Created only by exercising the same verifier, not a struct literal bypass.
pub fn evidence() -> (RawCiEvidence, TrustRoot) {
    fixture()
}
pub fn qualified() -> CiQualification {
    let (raw, trust) = fixture();
    verify_ci_qualification(&raw, &trust).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ci_qualification_bridge_accepts_trusted_exact_run() {
        let (raw, trust) = fixture();
        let q = verify_ci_qualification(&raw, &trust).unwrap();
        assert_eq!(q.repo(), REPO);
        assert_eq!(q.workflow_path(), WORKFLOW);
        assert_eq!((q.run_id(), q.run_attempt()), (7, 2));
        assert_eq!(q.jobs().len(), 3);
        assert_eq!(q.artifacts().len(), 2);
        assert_eq!(
            q.transform_exe_sha256(),
            trust.artifacts[&trust.target_artifact].transform_exe_sha256
        );
        assert!(!raw.retained_blobs().is_empty());
    }
    #[test]
    fn ci_qualification_rejects_fork_wrong_head_missing_or_skipped_job() {
        let (raw, trust) = fixture();
        for (field, value) in [
            ("head_sha", json!("b".repeat(40))),
            ("workflow_id", json!(1)),
            ("path", json!(".github/workflows/evil.yml")),
            ("run_attempt", json!(3)),
            ("conclusion", json!("failure")),
            ("event", json!("pull_request_target")),
        ] {
            let mut bad = raw.clone();
            bad.run[field] = value;
            assert!(verify_ci_qualification(&bad, &trust).is_err(), "{field}");
        }
        // A different successful attempt cannot use stale job/artifact evidence.
        for key in ["repository", "head_repository"] {
            for (field, value) in [
                ("fork", json!(true)),
                ("id", json!(3)),
                ("full_name", json!("attacker/coglet")),
            ] {
                let mut bad = raw.clone();
                bad.run[key][field] = value;
                assert!(verify_ci_qualification(&bad, &trust).is_err());
            }
        }
        let mut bad = raw.clone();
        bad.jobs.pop();
        assert_eq!(
            verify_ci_qualification(&bad, &trust),
            Err(QualificationError::Jobs)
        );
        let mut bad = raw.clone();
        bad.jobs.push(bad.jobs[0].clone());
        assert!(verify_ci_qualification(&bad, &trust).is_err());
        for (field, value) in [
            ("conclusion", json!("skipped")),
            ("conclusion", json!("neutral")),
            ("run_attempt", json!(1)),
            ("head_sha", json!("b".repeat(40))),
            ("run_id", json!(8)),
        ] {
            let mut bad = raw.clone();
            bad.jobs[0][field] = value;
            assert!(verify_ci_qualification(&bad, &trust).is_err(), "{field}");
        }
    }
    #[test]
    fn ci_qualification_rejects_wrong_artifact_digest_or_mode() {
        let (raw, trust) = fixture();
        let mut bad = raw.clone();
        bad.artifacts[0]["digest"] = json!(format!("sha256:{}", "0".repeat(64)));
        assert!(verify_ci_qualification(&bad, &trust).is_err());
        let mut bad = raw.clone();
        bad.artifacts[0]["expired"] = json!(true);
        assert!(verify_ci_qualification(&bad, &trust).is_err());
        let mut bad = raw.clone();
        bad.artifacts.push(bad.artifacts[0].clone());
        assert!(verify_ci_qualification(&bad, &trust).is_err());
        let mut bad = raw.clone();
        bad.blobs.values_mut().next().unwrap()[0] ^= 1;
        assert!(verify_ci_qualification(&bad, &trust).is_err());
        for field in [
            "transform_exe_sha256",
            "corpus_sha256",
            "results_sha256",
            "lock_sha256",
            "compiler_sha256",
        ] {
            let mut value = serde_json::to_value(&trust).unwrap();
            value["artifacts"][&trust.target_artifact][field] = json!("0".repeat(64));
            let bad: TrustRoot = serde_json::from_value(value).unwrap();
            assert!(verify_ci_qualification(&raw, &bad).is_err(), "{field}");
        }
        // Repack with a self-consistent digest to reach manifest binding checks.
        for (field, value) in [
            ("mode", json!("emulated")),
            (
                "arch",
                json!(if std::env::consts::ARCH == "x86_64" {
                    "aarch64"
                } else {
                    "x86_64"
                }),
            ),
            ("github_attempt", json!("1")),
            ("case_count", json!(0)),
            ("production_tool", json!("qualification-test")),
        ] {
            let mut bad = raw.clone();
            let mut t = trust.clone();
            let name = t.target_artifact.clone();
            let key = format!("artifact/{name}.zip");
            let mut z = zip::ZipArchive::new(Cursor::new(&bad.blobs[&key])).unwrap();
            let mut files = BTreeMap::new();
            for i in 0..z.len() {
                let mut f = z.by_index(i).unwrap();
                let n = f.name().to_owned();
                let mut b = Vec::new();
                f.read_to_end(&mut b).unwrap();
                files.insert(n, b);
            }
            drop(z);
            let mut m: Value = serde_json::from_slice(&files["manifest.json"]).unwrap();
            m[field] = value;
            files.insert("manifest.json".into(), serde_json::to_vec(&m).unwrap());
            let mut z = zip::ZipWriter::new(Cursor::new(Vec::new()));
            for (n, b) in files {
                z.start_file(n, zip::write::SimpleFileOptions::default())
                    .unwrap();
                z.write_all(&b).unwrap();
            }
            let bytes = z.finish().unwrap().into_inner();
            let digest = sha256_hex(&bytes);
            bad.blobs.insert(key, bytes);
            t.artifacts.get_mut(&name).unwrap().archive_sha256 = digest.clone();
            for a in &mut bad.artifacts {
                if a["name"] == name {
                    a["digest"] = json!(format!("sha256:{digest}"));
                }
            }
            assert!(verify_ci_qualification(&bad, &t).is_err(), "{field}");
        }
    }
}
