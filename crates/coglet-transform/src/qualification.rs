//! Exact-run GitHub qualification. TrustRoot is protected operator configuration,
//! never publication input. Only the verifier constructs CiQualification.
use crate::sha256_hex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};

pub const REPO: &str = "softmax-universe/coglet";
pub const REPO_ID: u64 = 1194342256;
pub const WORKFLOW: &str = ".github/workflows/rust.yml";
pub const WORKFLOW_ID: u64 = 427267446;
const MAX_BLOB: u64 = 512 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiQualification {
    repo: String,
    workflow_path: String,
    head_sha: String,
    run_id: u64,
    run_attempt: u64,
    jobs: Vec<(String, String)>,
    artifacts: Vec<(String, String)>,
    transform_exe_sha256: String,
    corpus_sha256: String,
}
impl CiQualification {
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn workflow_path(&self) -> &str {
        &self.workflow_path
    }
    pub fn head_sha(&self) -> &str {
        &self.head_sha
    }
    pub fn run_id(&self) -> u64 {
        self.run_id
    }
    pub fn run_attempt(&self) -> u64 {
        self.run_attempt
    }
    pub fn jobs(&self) -> &[(String, String)] {
        &self.jobs
    }
    pub fn artifacts(&self) -> &[(String, String)] {
        &self.artifacts
    }
    pub fn transform_exe_sha256(&self) -> &str {
        &self.transform_exe_sha256
    }
    pub fn corpus_sha256(&self) -> &str {
        &self.corpus_sha256
    }
}

/// One bounded native artifact. Every byte pin must be explicitly provisioned
/// from reviewed CI, not inferred from an untrusted publication's manifest.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactPin {
    pub archive_sha256: String,
    pub arch: String,
    pub transform_exe_sha256: String,
    pub corpus_sha256: String,
    pub results_sha256: String,
    pub lock_sha256: String,
    pub compiler_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustRoot {
    /// Absolute path to the trusted installed gh client. Its auth/configuration
    /// and TLS roots are part of protected local configuration, not request data.
    pub github_client: PathBuf,
    pub head_sha: String,
    pub required_jobs: BTreeSet<String>,
    pub artifacts: BTreeMap<String, ArtifactPin>,
    /// Executable selected for this publication host; both native legs required.
    pub target_artifact: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QualificationError {
    Transport,
    Identity,
    Run,
    Jobs,
    Artifact,
    Mode,
    Limit,
}
impl std::fmt::Display for QualificationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for QualificationError {}
type Result<T> = std::result::Result<T, QualificationError>;

/// Private custody: no Deserialize and no public constructor from caller JSON.
/// Retain these exact authenticated responses and archives in the router Store.
#[derive(Clone, Debug)]
pub struct RawCiEvidence {
    run: Value,
    jobs: Vec<Value>,
    artifacts: Vec<Value>,
    blobs: BTreeMap<String, Vec<u8>>,
}
impl RawCiEvidence {
    pub fn retained_blobs(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.blobs
    }
    /// Verified selected-native-leg files for the existing evidence Store.
    /// Archive bytes plus GitHub responses remain separately retained above.
    pub fn verified_files(&self, trust: &TrustRoot) -> Result<BTreeMap<String, Vec<u8>>> {
        verify_ci_qualification(self, trust)?;
        archive_files(&self.blobs[&format!("artifact/{}.zip", trust.target_artifact)])
    }
    pub fn fetch(trust: &TrustRoot, run_id: u64, run_attempt: u64) -> Result<Self> {
        validate_trust(trust)?;
        if run_id == 0 || run_attempt == 0 {
            return Err(QualificationError::Run);
        }
        let prefix = format!("repos/{REPO}/actions/runs/{run_id}");
        let mut blobs = BTreeMap::new();
        let mut get = |path: &str| -> Result<Value> {
            let bytes = github(trust, path)?;
            let value =
                serde_json::from_slice(&bytes).map_err(|_| QualificationError::Transport)?;
            blobs.insert(path.to_owned(), bytes);
            Ok(value)
        };
        let run = get(&format!("{prefix}/attempts/{run_attempt}"))?;
        // Refuse foreign identities before downloading any large artifacts.
        verify_run(&run, trust, run_id, run_attempt)?;
        let mut jobs = Vec::new();
        for page in 1..=100 {
            let value = get(&format!(
                "{prefix}/attempts/{run_attempt}/jobs?per_page=100&page={page}"
            ))?;
            let rows = value["jobs"].as_array().ok_or(QualificationError::Jobs)?;
            jobs.extend(rows.iter().cloned());
            if jobs.len() as u64
                == value["total_count"]
                    .as_u64()
                    .ok_or(QualificationError::Jobs)?
            {
                break;
            }
            if rows.is_empty() || page == 100 {
                return Err(QualificationError::Limit);
            }
        }
        let mut artifacts = Vec::new();
        for page in 1..=100 {
            let value = get(&format!("{prefix}/artifacts?per_page=100&page={page}"))?;
            let rows = value["artifacts"]
                .as_array()
                .ok_or(QualificationError::Artifact)?;
            artifacts.extend(rows.iter().cloned());
            if artifacts.len() as u64
                == value["total_count"]
                    .as_u64()
                    .ok_or(QualificationError::Artifact)?
            {
                break;
            }
            if rows.is_empty() || page == 100 {
                return Err(QualificationError::Limit);
            }
        }
        for name in trust.artifacts.keys() {
            let rows: Vec<_> = artifacts
                .iter()
                .filter(|a| a["name"].as_str() == Some(name))
                .collect();
            if rows.len() != 1 {
                return Err(QualificationError::Artifact);
            }
            let id = rows[0]["id"].as_u64().ok_or(QualificationError::Artifact)?;
            let bytes = github(trust, &format!("repos/{REPO}/actions/artifacts/{id}/zip"))?;
            blobs.insert(format!("artifact/{name}.zip"), bytes);
        }
        Ok(Self {
            run,
            jobs,
            artifacts,
            blobs,
        })
    }
}

fn github(trust: &TrustRoot, path: &str) -> Result<Vec<u8>> {
    let mut child = Command::new(&trust.github_client)
        .args([
            "api",
            "--hostname",
            "github.com",
            "--method",
            "GET",
            "-H",
            "Accept: application/vnd.github+json",
            "-H",
            "X-GitHub-Api-Version: 2022-11-28",
            path,
        ])
        .env_remove("GH_HOST")
        .env_remove("GH_REPO")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| QualificationError::Transport)?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .ok_or(QualificationError::Transport)?
        .take(MAX_BLOB + 1)
        .read_to_end(&mut bytes);
    if read.is_err() || bytes.len() as u64 > MAX_BLOB {
        let _ = child.kill();
        let _ = child.wait();
        return Err(QualificationError::Limit);
    }
    if !child
        .wait()
        .map_err(|_| QualificationError::Transport)?
        .success()
    {
        return Err(QualificationError::Transport);
    }
    Ok(bytes)
}
fn hex_pin(s: &str, len: usize) -> bool {
    s.len() == len
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn validate_trust(t: &TrustRoot) -> Result<()> {
    if !t.github_client.is_absolute()
        || !hex_pin(&t.head_sha, 40)
        || t.required_jobs.is_empty()
        || t.artifacts.len() != 2
        || !t.artifacts.contains_key(&t.target_artifact)
    {
        return Err(QualificationError::Identity);
    }
    let required = BTreeSet::from([
        "rust".to_owned(),
        "transform-native (ubuntu-24.04, x86_64)".to_owned(),
        "transform-native (ubuntu-24.04-arm, aarch64)".to_owned(),
    ]);
    if !required.is_subset(&t.required_jobs)
        || t.artifacts.keys().cloned().collect::<BTreeSet<_>>()
            != BTreeSet::from([
                "transform-native-x86_64-normal".to_owned(),
                "transform-native-aarch64-normal".to_owned(),
            ])
    {
        return Err(QualificationError::Jobs);
    }
    let arches: BTreeSet<_> = t.artifacts.values().map(|p| p.arch.as_str()).collect();
    if arches != BTreeSet::from(["x86_64", "aarch64"]) {
        return Err(QualificationError::Mode);
    }
    let mut corpora = BTreeSet::new();
    let mut results = BTreeSet::new();
    let mut locks = BTreeSet::new();
    for p in t.artifacts.values() {
        for pin in [
            &p.archive_sha256,
            &p.transform_exe_sha256,
            &p.corpus_sha256,
            &p.results_sha256,
            &p.lock_sha256,
            &p.compiler_sha256,
        ] {
            if !hex_pin(pin, 64) {
                return Err(QualificationError::Artifact);
            }
        }
        corpora.insert(&p.corpus_sha256);
        results.insert(&p.results_sha256);
        locks.insert(&p.lock_sha256);
    }
    if corpora.len() != 1 || results.len() != 1 || locks.len() != 1 {
        return Err(QualificationError::Artifact);
    }
    Ok(())
}
fn verify_run(r: &Value, t: &TrustRoot, id: u64, attempt: u64) -> Result<()> {
    for repo in ["repository", "head_repository"] {
        if r[repo]["id"].as_u64() != Some(REPO_ID)
            || r[repo]["full_name"] != REPO
            || r[repo]["fork"] != false
        {
            return Err(QualificationError::Identity);
        }
    }
    if r["workflow_id"].as_u64() != Some(WORKFLOW_ID)
        || r["path"] != WORKFLOW
        || r["head_sha"] != t.head_sha
    {
        return Err(QualificationError::Identity);
    }
    if id == 0
        || attempt == 0
        || r["id"].as_u64() != Some(id)
        || r["run_attempt"].as_u64() != Some(attempt)
        || r["status"] != "completed"
        || r["conclusion"] != "success"
        || !matches!(
            r["event"].as_str(),
            Some("push" | "pull_request" | "merge_group")
        )
    {
        return Err(QualificationError::Run);
    }
    Ok(())
}

pub fn verify_ci_qualification(raw: &RawCiEvidence, trust: &TrustRoot) -> Result<CiQualification> {
    validate_trust(trust)?;
    let id = raw.run["id"].as_u64().ok_or(QualificationError::Run)?;
    let attempt = raw.run["run_attempt"]
        .as_u64()
        .ok_or(QualificationError::Run)?;
    verify_run(&raw.run, trust, id, attempt)?;
    let mut jobs = BTreeMap::new();
    for j in &raw.jobs {
        let name = j["name"].as_str().ok_or(QualificationError::Jobs)?;
        if !trust.required_jobs.contains(name) {
            continue;
        }
        if j["run_id"].as_u64() != Some(id)
            || j["run_attempt"].as_u64() != Some(attempt)
            || j["head_sha"] != trust.head_sha
            || j["status"] != "completed"
            || j["conclusion"] != "success"
            || jobs.insert(name.to_owned(), "success".to_owned()).is_some()
        {
            return Err(QualificationError::Jobs);
        }
    }
    if jobs.keys().cloned().collect::<BTreeSet<_>>() != trust.required_jobs {
        return Err(QualificationError::Jobs);
    }
    let mut artifacts = Vec::new();
    for (name, pin) in &trust.artifacts {
        let rows: Vec<_> = raw
            .artifacts
            .iter()
            .filter(|a| a["name"].as_str() == Some(name))
            .collect();
        if rows.len() != 1 {
            return Err(QualificationError::Artifact);
        }
        let a = rows[0];
        if a["expired"] != false
            || a["digest"] != format!("sha256:{}", pin.archive_sha256)
            || a["workflow_run"]["id"].as_u64() != Some(id)
            || a["workflow_run"]["head_sha"] != trust.head_sha
            || a["workflow_run"]["repository_id"].as_u64() != Some(REPO_ID)
            || a["workflow_run"]["head_repository_id"].as_u64() != Some(REPO_ID)
        {
            return Err(QualificationError::Artifact);
        }
        let bytes = raw
            .blobs
            .get(&format!("artifact/{name}.zip"))
            .ok_or(QualificationError::Artifact)?;
        if sha256_hex(bytes) != pin.archive_sha256 {
            return Err(QualificationError::Artifact);
        }
        verify_archive(bytes, pin, &trust.head_sha, id, attempt)?;
        artifacts.push((name.clone(), pin.archive_sha256.clone()));
    }
    let target = &trust.artifacts[&trust.target_artifact];
    Ok(CiQualification {
        repo: REPO.into(),
        workflow_path: WORKFLOW.into(),
        head_sha: trust.head_sha.clone(),
        run_id: id,
        run_attempt: attempt,
        jobs: jobs.into_iter().collect(),
        artifacts,
        transform_exe_sha256: target.transform_exe_sha256.clone(),
        corpus_sha256: target.corpus_sha256.clone(),
    })
}
fn archive_files(bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut zip =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| QualificationError::Artifact)?;
    let mut files = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|_| QualificationError::Artifact)?;
        let name = f.name().to_owned();
        if name.contains('/')
            || name.contains('\\')
            || f.is_dir()
            || f.is_symlink()
            || f.size() > MAX_BLOB
        {
            return Err(QualificationError::Artifact);
        }
        total = total
            .checked_add(f.size())
            .ok_or(QualificationError::Limit)?;
        if total > MAX_BLOB {
            return Err(QualificationError::Limit);
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data)
            .map_err(|_| QualificationError::Artifact)?;
        if files.insert(name, data).is_some() {
            return Err(QualificationError::Artifact);
        }
    }
    Ok(files)
}
fn verify_archive(
    bytes: &[u8],
    pin: &ArtifactPin,
    head: &str,
    id: u64,
    attempt: u64,
) -> Result<()> {
    let files = archive_files(bytes)?;
    let m: Value = serde_json::from_slice(
        files
            .get("manifest.json")
            .ok_or(QualificationError::Artifact)?,
    )
    .map_err(|_| QualificationError::Artifact)?;
    if m["mode"] != "native" || m["arch"] != pin.arch {
        return Err(QualificationError::Mode);
    }
    if m["actual_checkout"] != head
        || m["github_run"].as_str() != Some(id.to_string().as_str())
        || m["github_attempt"].as_str() != Some(attempt.to_string().as_str())
        || m["production_tool"] != "coglet-transform"
        || m["case_count"] != 960
    {
        return Err(QualificationError::Artifact);
    }
    let manifest_files = m["files"].as_object().ok_or(QualificationError::Artifact)?;
    if files.len() != manifest_files.len() + 1 {
        return Err(QualificationError::Artifact);
    }
    for (name, digest) in manifest_files {
        let data = files.get(name).ok_or(QualificationError::Artifact)?;
        if data.is_empty() || digest.as_str() != Some(sha256_hex(data).as_str()) {
            return Err(QualificationError::Artifact);
        }
    }
    for (name, digest) in [
        ("coglet-transform", &pin.transform_exe_sha256),
        ("fuzz.corpus", &pin.corpus_sha256),
        ("fuzz.results", &pin.results_sha256),
        ("Cargo.lock", &pin.lock_sha256),
        ("rustc.txt", &pin.compiler_sha256),
    ] {
        if manifest_files.get(name).and_then(Value::as_str) != Some(digest.as_str()) {
            return Err(QualificationError::Artifact);
        }
    }
    let exe = files
        .get("coglet-transform")
        .ok_or(QualificationError::Artifact)?;
    let machine: &[u8] = if pin.arch == "x86_64" {
        &[62, 0]
    } else {
        &[183, 0]
    };
    if exe.get(..6) != Some(b"\x7fELF\x02\x01") || exe.get(18..20) != Some(machine) {
        return Err(QualificationError::Artifact);
    }
    if files.get("fuzz.results").map(Vec::len) != Some(960 * 80) {
        return Err(QualificationError::Artifact);
    }
    Ok(())
}

#[cfg(any(test, feature = "test-fixtures"))]
#[path = "qualification_fixtures.rs"]
pub mod test_fixtures;
