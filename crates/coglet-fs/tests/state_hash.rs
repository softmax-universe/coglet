use coglet_fs::{Snapshot, snapshot_tree_hash};
use std::{collections::BTreeMap, path::Path, process::Command};

fn git(repo: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("GIT_") {
            cmd.env_remove(key);
        }
    }
    let out = cmd
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git required for state hash oracle");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[test]
fn state_hash_matches_git_sha256_tree() {
    let cases: Vec<Vec<(&str, Vec<u8>)>> = vec![
        vec![],
        vec![("/data/count", b"1".to_vec())],
        vec![
            ("/code/bin/step.wasm", b"wasm".to_vec()),
            ("/data/a/b/c", b"nested".to_vec()),
            ("/logs/empty", vec![]),
        ],
        vec![
            ("/data/foo.bar", vec![1]),
            ("/data/foo/a", vec![2]),
            ("/data/foo0", vec![3]),
            ("/data/foo-/z", vec![4]),
            ("/data/é", vec![5]),
        ],
        vec![("/data/binary", (0..=255).collect())],
        vec![
            ("/data/a-c", vec![1]),
            ("/data/a.b", vec![2]),
            ("/data/a/x", vec![3]),
        ],
    ];
    for case in cases {
        let files: BTreeMap<String, Vec<u8>> =
            case.into_iter().map(|(p, b)| (p.into(), b)).collect();
        let repo = tempfile::tempdir().unwrap();
        git(repo.path(), &["init", "--object-format=sha256"]);
        for (path, bytes) in &files {
            let path = repo.path().join(&path[1..]);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, bytes).unwrap();
        }
        git(
            repo.path(),
            &[
                "-c",
                "core.autocrlf=false",
                "-c",
                "core.filemode=false",
                "add",
                "--all",
            ],
        );
        let expected = git(repo.path(), &["write-tree"]);
        let snapshot = Snapshot::from_files(files).unwrap();
        assert_eq!(snapshot.tree_hash(), expected);
        assert_eq!(snapshot_tree_hash(&snapshot.export()).unwrap(), expected);
    }
}

#[test]
fn invalid_snapshot_is_not_hashed() {
    for files in [
        BTreeMap::from([("/data/a".into(), "GG".into())]),
        BTreeMap::from([("/data/a".into(), "aB".into())]),
        BTreeMap::from([("/data/a".into(), "0".into())]),
        BTreeMap::from([("data/a".into(), "00".into())]),
        BTreeMap::from([
            ("/data/a".into(), "00".into()),
            ("/data/a/b".into(), "00".into()),
        ]),
    ] {
        assert!(snapshot_tree_hash(&files).is_err());
    }
}

#[test]
fn state_hash_rejects_unmappable_paths() {
    for path in [
        "/data/.git",
        "/data/.GiT/x",
        "/code/a/.GIT/b",
        "/logs/.git/x",
    ] {
        let files = BTreeMap::from([(path.to_owned(), "00".to_owned())]);
        assert!(snapshot_tree_hash(&files).is_err(), "{path}");
        assert!(Snapshot::from_files(BTreeMap::from([(path.to_owned(), vec![0])])).is_err());
    }
    for (file, child) in [("/data/a", "/data/a/b"), ("/code/a", "/code/a/b")] {
        assert!(
            snapshot_tree_hash(&BTreeMap::from([
                (file.to_owned(), "00".to_owned()),
                (child.to_owned(), "00".to_owned()),
            ]))
            .is_err()
        );
    }
    // Matching is component-exact; these remain valid ordinary names.
    for path in [
        "/data/.gitignore",
        "/code/git",
        "/logs/x.git",
        "/data/.gitx/x",
    ] {
        assert!(snapshot_tree_hash(&BTreeMap::from([(path.to_owned(), "00".to_owned())])).is_ok());
    }
}
