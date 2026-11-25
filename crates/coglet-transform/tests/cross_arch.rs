//! Native architecture aggregation is fed only same-run CI artifacts by rust.yml.
//! Local caller-supplied files are not production qualification credentials.
#[test]
fn pinned_transform_cross_arch_replay() {
    let pair = std::env::var("COGLET_TRANSFORM_NATIVE_PAIR")
        .expect("both actually executed native architecture artifacts are required");
    let output = std::process::Command::new("python3")
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/check.py"))
        .arg("--aggregate")
        .arg(pair)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
