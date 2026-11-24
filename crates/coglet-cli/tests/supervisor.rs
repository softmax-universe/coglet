#![cfg(unix)]
use std::{
    fs,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
fn shell(path: &Path, body: &str) -> Command {
    fs::write(path, body).unwrap();
    let mut c = Command::new("/bin/sh");
    c.arg(path);
    c
}
#[test]
fn child_supervisor_times_out_and_bounds_both_output_streams() {
    let d = tempfile::tempdir().unwrap();
    let start = Instant::now();
    let c = shell(&d.path().join("hang.sh"), "while :; do :; done\n");
    let error = coglet_cli::process::run(c, vec![], Duration::from_millis(50), 1024).unwrap_err();
    assert!(error.to_string().contains("timeout"));
    assert!(start.elapsed() < Duration::from_secs(3));
    for redirect in ["", " >&2"] {
        let c = shell(
            &d.path().join("flood.sh"),
            &format!("while :; do printf 'xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx'{redirect}; done\n"),
        );
        let error = coglet_cli::process::run(c, vec![], Duration::from_secs(3), 1024).unwrap_err();
        assert!(error.to_string().contains("output limit"), "{error:#}");
    }
    let mut c = shell(
        &d.path().join("env.sh"),
        "printf '%s' \"${COGLET_SECRET_TEST-unset}\"\n",
    );
    c.env("COGLET_SECRET_TEST", "should-not-reach-child");
    assert_eq!(
        coglet_cli::process::run(c, vec![], Duration::from_secs(3), 1024).unwrap(),
        b"unset"
    );
}
#[test]
fn file_reader_refuses_fifo_symlink_and_world_readable_secrets() {
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::{PermissionsExt, symlink},
        },
    };
    let d = tempfile::tempdir().unwrap();
    let real = d.path().join("real");
    fs::write(&real, b"secret").unwrap();
    assert!(coglet_cli::read_private(&real, 32).is_err());
    fs::set_permissions(&real, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(coglet_cli::read_private(&real, 32).unwrap(), b"secret");
    let link = d.path().join("link");
    symlink(&real, &link).unwrap();
    assert!(coglet_cli::read(&link, 32).is_err());
    let fifo = d.path().join("fifo");
    let raw = CString::new(fifo.as_os_str().as_bytes()).unwrap();
    assert_eq!(unsafe { libc::mkfifo(raw.as_ptr(), 0o600) }, 0);
    assert!(coglet_cli::read(&fifo, 32).is_err());
}
