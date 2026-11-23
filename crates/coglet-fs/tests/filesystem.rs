use coglet_fs::{EVENTS, Error, HEARD, INBOX, Snapshot};
use serde_json::Value;
use std::collections::BTreeMap;
fn seeded() -> Snapshot {
    Snapshot::from_files(BTreeMap::from([
        ("/code/bin/step.wasm".into(), b"wasm".to_vec()),
        ("/data/state".into(), b"old".to_vec()),
        ("/logs/output".into(), b"old\n".to_vec()),
    ]))
    .unwrap()
}
#[test]
fn cow_commit_and_abort_isolate_all_mounts() {
    let old = seeded();
    let hash = old.tree_hash();
    let mut guest = old.transaction();
    guest.write("/data/state", b"new").unwrap();
    guest.append("/logs/output", b"new\n").unwrap();
    assert_eq!(guest.snapshot().read("/data/state").unwrap(), b"new");
    assert_eq!(old.tree_hash(), hash);
    let new = guest.commit();
    assert_ne!(new.tree_hash(), hash);
    let mut abort = new.transaction();
    abort.write("/data/state", b"discard").unwrap();
    drop(abort);
    assert_eq!(new.read("/data/state").unwrap(), b"new");
    assert_eq!(old.read("/logs/output").unwrap(), b"old\n");
}
fn deny_mutations(snapshot: &Snapshot, path: &str, deny_append: bool) {
    let mut tx = snapshot.transaction();
    let before = tx.snapshot().tree_hash();
    assert_eq!(tx.write(path, b"bad"), Err(Error::PermissionDenied));
    assert_eq!(tx.write_at(path, 0, b"bad"), Err(Error::PermissionDenied));
    assert_eq!(tx.truncate(path, 0), Err(Error::PermissionDenied));
    assert_eq!(tx.unlink(path), Err(Error::PermissionDenied));
    assert_eq!(tx.rename(path, "/data/other"), Err(Error::PermissionDenied));
    assert_eq!(tx.rename("/data/state", path), Err(Error::PermissionDenied));
    if deny_append {
        assert_eq!(tx.create(path), Err(Error::PermissionDenied));
        assert_eq!(tx.append(path, b"bad"), Err(Error::PermissionDenied));
    }
    assert_eq!(tx.snapshot().tree_hash(), before);
}
#[test]
fn code_is_read_only_existing_and_absent() {
    for path in ["/code/bin/step.wasm", "/code/new"] {
        deny_mutations(&seeded(), path, true);
    }
}
#[test]
fn logs_allow_only_create_append_never_overwrite_or_move() {
    let snapshot = seeded();
    for path in ["/logs/output", "/logs/new"] {
        deny_mutations(&snapshot, path, false);
    }
    let mut tx = snapshot.transaction();
    tx.create("/logs/new").unwrap();
    tx.append("/logs/new", b"a").unwrap();
    tx.append("/logs/new", b"b").unwrap();
    assert_eq!(tx.create("/logs/new"), Err(Error::Conflict));
    assert_eq!(tx.snapshot().read("/logs/new").unwrap(), b"ab");
    tx.append("/logs/implicit", b"created").unwrap();
}
#[test]
fn reserved_inbox_heard_events_reject_guest_create_write_rename_unlink() {
    let mut tx = seeded().transaction();
    tx.host_append_inbox(b"{\"example\":true}\n").unwrap();
    tx.host_replace_heard(b"\xa1eitems\x80").unwrap();
    tx.host_append_log(b"{\"message\":\"hello\"}\n").unwrap();
    for snapshot in [seeded(), tx.commit()] {
        for path in [INBOX, HEARD, EVENTS] {
            deny_mutations(&snapshot, path, true);
        }
    }
}
#[test]
fn runtime_heard_boundary_is_readable_hashed_and_survives_guest_abort() {
    let old = seeded();
    let mut host = old.transaction();
    let bytes = b"\xa1eitems\x80";
    host.host_replace_heard(bytes).unwrap();
    let input = host.commit();
    assert_eq!(input.read(HEARD).unwrap(), bytes);
    assert_ne!(input.tree_hash(), old.tree_hash());
    let mut guest = input.transaction();
    guest.write("/data/state", b"abort").unwrap();
    drop(guest);
    assert_eq!(input.read(HEARD).unwrap(), bytes);
    let mut debug = input.transaction();
    assert_eq!(
        debug.debug_write(HEARD, b"bad"),
        Err(Error::PermissionDenied)
    );
    assert_eq!(
        debug.debug_write(INBOX, b"bad"),
        Err(Error::PermissionDenied)
    );
    assert_eq!(
        debug.debug_write(EVENTS, b"bad"),
        Err(Error::PermissionDenied)
    );
}
#[test]
fn host_appends_preserve_exact_bytes_and_debug_cannot_change_code_or_logs() {
    let mut tx = seeded().transaction();
    tx.host_append_inbox(b"a\n").unwrap();
    tx.host_append_inbox(b"b\n").unwrap();
    tx.host_append_log(b"a\n").unwrap();
    tx.host_append_log(b"b\n").unwrap();
    assert_eq!(tx.snapshot().read(INBOX).unwrap(), b"a\nb\n");
    assert_eq!(tx.snapshot().read(EVENTS).unwrap(), b"a\nb\n");
    assert_eq!(
        tx.debug_write("/code/new", b"bad"),
        Err(Error::PermissionDenied)
    );
    assert_eq!(
        tx.debug_write("/logs/new", b"bad"),
        Err(Error::PermissionDenied)
    );
    tx.debug_write("/data/state", b"debug").unwrap();
}
#[test]
fn every_frozen_path_forbidden_fixture_through_real_guest_and_boundary_apis() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../spec/fixtures/abi/cases.json")).unwrap();
    let mut count = 0;
    for case in cases
        .iter()
        .filter(|c| c["name"].as_str().unwrap().starts_with("path-forbidden-"))
    {
        let path = case["value"].as_str().unwrap();
        count += 1;
        deny_mutations(&seeded(), path, true);
        let mut tx = seeded().transaction();
        assert_eq!(
            tx.snapshot().read(path),
            Err(Error::PermissionDenied),
            "{}",
            case["name"]
        );
        assert_eq!(tx.debug_write(path, b"bad"), Err(Error::PermissionDenied));
    }
    assert!(count > 0);
    println!("frozen path-forbidden fixtures rejected: {count}");
}
#[test]
fn all_c0_del_backslash_escape_roots_and_dot_components_are_denied() {
    for byte in (0u8..=31).chain([127, b'\\']) {
        deny_mutations(&seeded(), &format!("/data/a{}b", byte as char), true);
    }
    for p in [
        "/data",
        "/data/",
        "/data//a",
        "/data/./a",
        "/data/../code/a",
        "data/a",
        "/other/a",
        "/datax/a",
    ] {
        deny_mutations(&seeded(), p, true);
    }
}
#[test]
fn no_unicode_normalization_and_directory_entries_are_utf8_sorted() {
    let mut tx = seeded().transaction();
    tx.write("/data/café", b"one").unwrap();
    tx.write("/data/cafe\u{301}", b"two").unwrap();
    assert_eq!(tx.snapshot().read("/data/café").unwrap(), b"one");
    assert_eq!(tx.snapshot().read("/data/cafe\u{301}").unwrap(), b"two");
    let paths: Vec<_> = tx.snapshot().files().map(|(p, _)| p).collect();
    assert!(paths.windows(2).all(|w| w[0].as_bytes() < w[1].as_bytes()));
}
#[test]
fn regular_files_only_and_failed_rename_is_atomic() {
    let mut tx = seeded().transaction();
    let hash = tx.snapshot().tree_hash();
    assert_eq!(tx.write("/data/state/child", b"no"), Err(Error::Conflict));
    assert_eq!(tx.write("/code", b"no"), Err(Error::PermissionDenied));
    assert_eq!(
        tx.rename("/data/state", "/code/new"),
        Err(Error::PermissionDenied)
    );
    assert_eq!(
        tx.rename("/data/state", "/data/state/child"),
        Err(Error::Conflict)
    );
    assert_eq!(tx.snapshot().tree_hash(), hash);
    tx.write("/data/dest", b"replace").unwrap();
    tx.rename("/data/state", "/data/dest").unwrap();
    assert_eq!(tx.snapshot().read("/data/state"), Err(Error::NotFound));
    assert_eq!(tx.snapshot().read("/data/dest").unwrap(), b"old");
}
#[test]
fn guest_data_write_at_truncate_and_unlink() {
    let mut tx = seeded().transaction();
    let before = tx.snapshot().tree_hash();
    assert_eq!(
        tx.write_at("/data/state", usize::MAX, b"x"),
        Err(Error::InvalidArgument)
    );
    assert_eq!(tx.snapshot().tree_hash(), before);
    tx.write_at("/data/state", 1, b"X").unwrap();
    assert_eq!(tx.snapshot().read("/data/state").unwrap(), b"oXd");
    tx.truncate("/data/state", 5).unwrap();
    assert_eq!(tx.snapshot().read("/data/state").unwrap(), b"oXd\0\0");
    tx.unlink("/data/state").unwrap();
    tx.write("/data/sparse", b"a").unwrap();
    tx.write_at("/data/sparse", 3, b"b").unwrap();
    assert_eq!(tx.snapshot().read("/data/sparse").unwrap(), b"a\0\0b");
    assert_eq!(tx.unlink("/data/state"), Err(Error::NotFound));
}
#[test]
fn abi_tree_hash_matches_frozen_trace_snapshot_goldens() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../spec/fixtures/abi/trace-cases.fixture"
    ))
    .unwrap();
    let mut count = 0;
    // Only explicitly valid cases; invalid variants intentionally forge hashes.
    for case in cases.iter().filter(|c| c["valid"].as_bool() == Some(true)) {
        let trace = &case["value"];
        let Some(files) = trace["initial_files"].as_object() else {
            continue;
        };
        let files = files
            .iter()
            .map(|(p, b)| {
                let s = b.as_str().unwrap();
                let bytes = (0..s.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
                    .collect();
                (p.clone(), bytes)
            })
            .collect();
        assert_eq!(
            Snapshot::from_files(files).unwrap().tree_hash(),
            trace["initial_hash"].as_str().unwrap(),
            "{}",
            case["name"]
        );
        count += 1;
    }
    assert!(count > 0);
    println!("ABI golden initial tree hashes: {count}");
}
