use std::collections::BTreeMap;

#[test]
fn state_hash_single_implementation() {
    for files in [
        BTreeMap::new(),
        BTreeMap::from([
            ("/data/count".into(), b"1".to_vec()),
            ("/data/a/x".into(), vec![0, 255, 10]),
            ("/data/a.txt".into(), Vec::new()),
            ("/code/bin/step.wasm".into(), b"wasm".to_vec()),
            ("/logs/out".into(), b"hi\n".to_vec()),
        ]),
    ] {
        let fs = coglet_fs::Snapshot::from_files(files).unwrap();
        let runtime = coglet_runtime::Snapshot::new(fs.export()).unwrap();
        assert_eq!(fs.tree_hash(), runtime.tree_hash().unwrap());
        assert_eq!(
            fs.tree_hash(),
            coglet_fs::snapshot_tree_hash(runtime.files()).unwrap()
        );
    }
}
