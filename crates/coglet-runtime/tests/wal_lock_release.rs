//! The WAL's exclusive lock belongs to the open file description. A process
//! spawned concurrently by any thread shares that description until its `exec`
//! closes the inherited descriptor, so releasing the lock by `close` alone left
//! the WAL "already owned" for that window. `cogletd` spawns validator
//! subprocesses while it reopens WALs, so the next open could be refused.
#![cfg(unix)]

use coglet_runtime::DurableStore;
use std::{
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[test]
fn wal_lock_release_is_not_delayed_by_concurrent_process_spawn() {
    let dir = tempfile::tempdir().unwrap();
    let wal = dir.path().join("state.wal");

    // Exclusive ownership itself is unchanged: a second open is refused for
    // exactly as long as the first store lives.
    let owner = DurableStore::open(&wal).unwrap();
    let refused = DurableStore::open(&wal).err().expect("second owner");
    assert!(refused.starts_with("WAL already owned:"), "{refused}");
    drop(owner);

    let stop = Arc::new(AtomicBool::new(false));
    let spawners: Vec<_> = (0..2)
        .map(|_| {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut spawned = 0u32;
                while !stop.load(Ordering::Relaxed) {
                    Command::new("true").status().expect("spawn `true`");
                    spawned += 1;
                }
                spawned
            })
        })
        .collect();
    let failures: Vec<String> = (0..300)
        .filter_map(|_| DurableStore::open(&wal).map(drop).err())
        .collect();
    stop.store(true, Ordering::Relaxed);
    let spawned: u32 = spawners.into_iter().map(|t| t.join().unwrap()).sum();
    // The fixture must really have overlapped process spawns with the opens.
    assert!(spawned > 0, "no process was spawned");
    assert!(
        failures.is_empty(),
        "{} of 300 reopen attempts failed, first: {}",
        failures.len(),
        failures[0]
    );
}
