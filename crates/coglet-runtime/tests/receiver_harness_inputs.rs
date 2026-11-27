//! Guard for the frozen receiver qualification harness inputs.
//!
//! `run_receiver_conformance.py` is not run by a workflow, so a tree change that
//! removes one of its inputs would otherwise stay green. This default-feature
//! test runs in the required workspace job and fails when any file the fixture
//! peer or the oracle reads is missing, unparsable, or outside this crate and
//! the frozen `conformance/` tree.
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const SNAPSHOT: &str = "examples/support/receiver-fetch-logs-snapshot.json";
/// Every source file of the harness that opens repository files.
const HARNESS_SOURCES: &[&str] = &[
    "run_receiver_conformance.py",
    "receiver_oracles.py",
    "receiver_peer_driver.py",
    "check_receiver_evidence.py",
    "hub-receiver-conformance.toml",
    "examples/runtime_receiver_peer.rs",
    "examples/support/receiver_fixture.rs",
];

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn repo_root() -> PathBuf {
    crate_dir().join("../..")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The quoted names inside the first `NAME = (` / `NAME: &[&str] = &[` block.
fn quoted_block(source: &str, open: &str, close: &str) -> Vec<String> {
    let start = source
        .find(open)
        .unwrap_or_else(|| panic!("{open} missing"))
        + open.len();
    let end = start + source[start..].find(close).expect("unterminated case list");
    source[start..end]
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

fn runner_cases() -> Vec<String> {
    quoted_block(
        &read(&crate_dir().join("run_receiver_conformance.py")),
        "CASES = (",
        ")",
    )
}

#[test]
fn fetch_logs_snapshot_is_crate_owned_and_self_consistent() {
    let snapshot: Value = serde_json::from_str(&read(&crate_dir().join(SNAPSHOT))).unwrap();
    assert_eq!(snapshot["ref"], "refs/heads/logs");
    let statement = &snapshot["update"]["statement"];
    assert_eq!(statement["ref"], snapshot["ref"]);
    assert_eq!(statement["purpose"], "logs-ref");
    for field in ["signer", "signature"] {
        let value = snapshot["update"][field].as_str().expect(field);
        assert!(hex::decode(value).is_ok(), "{field} is not hex");
    }
    let bundle = hex::decode(snapshot["bundle_hex"].as_str().expect("bundle_hex")).unwrap();
    assert!(!bundle.is_empty());
    assert_eq!(snapshot["size"].as_u64(), Some(bundle.len() as u64));
    assert_eq!(
        snapshot["sha256"].as_str(),
        Some(hex::encode(Sha256::digest(&bundle)).as_str())
    );
    // The frozen positive case must request exactly the ref this input serves.
    let case: Value = serde_json::from_str(&read(
        &repo_root().join("conformance/hub/fetch-scoped-allowed.json"),
    ))
    .unwrap();
    let requested = case["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find_map(|step| step["send"]["params"]["ref"].as_str())
        .expect("frozen fetch request names a ref");
    assert_eq!(snapshot["ref"], requested);
}

#[test]
fn every_frozen_receiver_case_input_exists_and_parses() {
    let cases = runner_cases();
    assert_eq!(cases.len(), 25, "frozen receiver subset is 25 cases");
    let fixture_cases = quoted_block(
        &read(&crate_dir().join("examples/support/receiver_fixture.rs")),
        "pub const CASES: &[&str] = &[",
        "]",
    );
    assert_eq!(cases, fixture_cases, "runner and fixture case lists differ");
    let registry = read(&crate_dir().join("hub-receiver-conformance.toml"));
    for case in &cases {
        let path = repo_root().join(format!("conformance/hub/{case}.json"));
        let frozen: Value = serde_json::from_str(&read(&path)).unwrap();
        assert!(
            frozen["steps"].as_array().is_some_and(|s| !s.is_empty()),
            "{case} has no steps"
        );
        assert!(
            registry.contains(&format!("[implementations.fixtures.\"hub.{case}\"]")),
            "{case} is not registered"
        );
    }
    for tool in ["conformance/run", "conformance/wire.py"] {
        assert!(repo_root().join(tool).is_file(), "{tool} missing");
    }
}

#[test]
fn harness_reads_no_spec_tree_file() {
    // The spec tree is owned by ADRs and may drop fixtures; the harness may
    // depend only on this crate and the frozen `conformance/` tree.
    for source in HARNESS_SOURCES {
        let text = read(&crate_dir().join(source));
        assert!(
            !text.contains("spec/"),
            "{source} references the spec tree; keep harness inputs under crates/coglet-runtime"
        );
    }
    let fixture = read(&crate_dir().join("examples/support/receiver_fixture.rs"));
    assert!(fixture.contains("include_str!(\"receiver-fetch-logs-snapshot.json\")"));
    let oracle = read(&crate_dir().join("receiver_oracles.py"));
    assert!(oracle.contains(SNAPSHOT));
}
