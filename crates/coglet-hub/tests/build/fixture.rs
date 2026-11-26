use coglet_hub::build::{BuildRequest, Builder, Profile, Target};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{fs, path::Path, process::Command};
use tempfile::TempDir;

pub fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn git(path: &Path, args: &[&str]) -> String {
    let out = Command::new("/usr/bin/git")
        .current_dir(path)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_AUTHOR_NAME", "Fixture")
        .env("GIT_AUTHOR_EMAIL", "fixture@example.invalid")
        .env("GIT_COMMITTER_NAME", "Fixture")
        .env("GIT_COMMITTER_EMAIL", "fixture@example.invalid")
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
pub const WAT: &str = "(module (memory (export \"memory\") 1 2) (func (export \"abi_alloc\") (param i32) (result i32) i32.const 0) (func (export \"handle\") (param i32 i32 i64 i64 i64) (result i32) i32.const 0))";
pub struct Fixture {
    pub dir: TempDir,
    pub builder: Builder,
    pub request: BuildRequest,
}
pub fn fixture_with_version(wat: &str, output_pin: Option<&str>, version: &str) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let coglet = format!("coglet:{}", iroh::SecretKey::from_bytes(&[7; 32]).public());
    let manifest = json!({"version":version,"id":coglet,"subscriptions":[],"required_capabilities":[],"exclusive_outputs":[],"budgets":{"compute":10000,"compute_mode":"wamr.interpreter","memory_pages":2,"wall_time_us":100000,"max_events":16,"host_bytes":65536}});
    fs::write(dir.path().join("guest.wat"), wat).unwrap();
    fs::write(
        dir.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    git(dir.path(), &["init", "--initial-branch=source"]);
    git(dir.path(), &["add", "."]);
    git(dir.path(), &["commit", "-m", "pinned source"]);
    let source_commit = git(dir.path(), &["rev-parse", "HEAD"]);
    let output_hash = hash(&wat::parse_str(wat).unwrap());
    let profile = Profile::integer_wat(
        "integer-v1",
        hash(wat.as_bytes()),
        hash(&serde_json::to_vec(&manifest).unwrap()),
        output_pin.unwrap_or(&output_hash).into(),
    );
    let target = Target {
        repository: dir.path().into(),
        coglet,
        device: iroh::SecretKey::from_bytes(&[9; 32]).public().to_string(),
    };
    let builder = Builder::new(target, profile).unwrap();
    Fixture {
        dir,
        builder,
        request: BuildRequest {
            source_commit,
            profile_id: "integer-v1".into(),
        },
    }
}
