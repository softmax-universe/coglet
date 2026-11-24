//! Building this target on Darwin is the regression for Linux-only waitid imports.
//! Unsupported hosts must still run the real helper and refuse before parsing or spawning.
#[cfg(not(target_os = "linux"))]
#[test]
fn unsupported_validator_helper_refuses_before_processing_arguments() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_coglet-git-validator"))
        .args(["/must-not-be-opened", "invalid job"])
        .output()
        .expect("run compiled validator helper");
    assert_eq!(output.status.code(), Some(22));
    assert_eq!(output.stdout, b"validator unavailable: Linux required\n");
    assert!(output.stderr.is_empty());
}
