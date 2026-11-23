use super::*;
#[test]
fn snapshots_share_tree_and_unchanged_content_until_write() {
    let old = Snapshot::from_files(BTreeMap::from([
        ("/data/a".into(), vec![1; 1024]),
        ("/data/b".into(), vec![2; 1024]),
    ]))
    .unwrap();
    let mut tx = old.transaction();
    assert!(Arc::ptr_eq(&old.files, &tx.view.files));
    tx.write("/data/a", b"new").unwrap();
    assert!(!Arc::ptr_eq(&old.files, &tx.view.files));
    assert!(Arc::ptr_eq(
        &old.files["/data/b"].bytes,
        &tx.view.files["/data/b"].bytes
    ));
    assert!(!Arc::ptr_eq(
        &old.files["/data/a"].bytes,
        &tx.view.files["/data/a"].bytes
    ));
}
