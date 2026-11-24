//! A validator/process launcher can fork while a WAL probe owns its flock.
//! CLOEXEC does not release the inherited lock until the child reaches exec.
#![cfg(target_os = "linux")]

#[test]
fn retired_wal_owner_releases_lock_while_fork_child_has_not_execed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.wal");
    let store = coglet_runtime::DurableStore::open(&path).unwrap();
    let mut pipe = [-1; 2];
    // The child performs only async-signal-safe libc calls before _exit.
    assert_eq!(unsafe { libc::pipe(pipe.as_mut_ptr()) }, 0);
    let child = unsafe { libc::fork() };
    assert!(child >= 0);
    if child == 0 {
        unsafe {
            libc::close(pipe[1]);
            let mut byte = 0u8;
            libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
            libc::_exit(0);
        }
    }
    unsafe {
        libc::close(pipe[0]);
    }
    // A second opener must still refuse while the real owner is alive.
    let live_owner_refused = coglet_runtime::DurableStore::open(&path)
        .err()
        .is_some_and(|error| error.starts_with("WAL already owned:"));
    drop(store);
    // Child is still blocked, retaining the inherited open-file description.
    let reopened = coglet_runtime::DurableStore::open(&path);
    unsafe {
        libc::write(pipe[1], [1u8].as_ptr().cast(), 1);
        libc::close(pipe[1]);
        let mut status = 0;
        assert_eq!(libc::waitpid(child, &mut status, 0), child);
        assert_eq!(status, 0);
    }
    assert!(
        live_owner_refused,
        "live owner's exclusive custody was bypassed"
    );
    assert!(
        reopened.is_ok(),
        "retired owner's inherited flock: {:?}",
        reopened.err()
    );
}
