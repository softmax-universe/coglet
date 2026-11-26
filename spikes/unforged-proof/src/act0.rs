//! Act 0 — ADR-0035 state-hash parity across repositories:
//! `coglet_fs::Snapshot::tree_hash` == `coglet_runtime::Snapshot::tree_hash` ==
//! `unforged_gitobj::snapshot_tree_hash_sha256` == real `git` (`--object-format=sha256`).
use crate::counter::{Files, context, echo_events, wamr_step};
use crate::report::ActReport;
use gecaos::coglet::{
    LooseObjectStore, ObjectKind, Oid256, TreeEntry, TreeProfile, decode_tree, encode_tree,
    hash_blob, validate_local_tree_coglet_strict,
};
use gecaos::os::GecaosOs;
use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

fn git(repo: &Path, index: &Path, args: &[&str], stdin: Option<&[u8]>) -> Result<String, String> {
    let mut child = Command::new("git")
        .arg("--git-dir")
        .arg(repo)
        .args(args)
        .env("GIT_INDEX_FILE", index)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(bytes) = stdin {
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(bytes)
            .map_err(|e| e.to_string())?;
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Tree id real git computes for `files` (blobs via `hash-object -w`, index, `write-tree`).
fn git_tree(repo: &Path, index: &Path, files: &Files) -> Result<String, String> {
    let _ = std::fs::remove_file(index);
    for (path, bytes) in files {
        let oid = git(repo, index, &["hash-object", "-w", "--stdin"], Some(bytes))?;
        git(
            repo,
            index,
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{oid},{path}"),
            ],
            None,
        )?;
    }
    git(repo, index, &["write-tree"], None)
}

fn slash(files: &Files) -> BTreeMap<String, Vec<u8>> {
    files
        .iter()
        .map(|(p, b)| (format!("/{p}"), b.clone()))
        .collect()
}

fn hex_map(files: &Files) -> BTreeMap<String, String> {
    files
        .iter()
        .map(|(p, b)| (format!("/{p}"), hex::encode(b)))
        .collect()
}

fn files(pairs: &[(&str, &[u8])]) -> Files {
    pairs
        .iter()
        .map(|(p, b)| (p.to_string(), b.to_vec()))
        .collect()
}

pub fn run(os: &GecaosOs, genesis: &Files, work: &Path) -> Result<ActReport, String> {
    let mut r = ActReport::new(
        "0",
        "ADR-0035 state-hash parity: coglet-fs == unforged-gitobj == git",
    );
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let repo = work.join("parity.git");
    let index = work.join("parity.index");
    let init = Command::new("git")
        .args(["init", "-q", "--bare", "--object-format=sha256"])
        .arg(&repo)
        .output()
        .map_err(|e| e.to_string())?;
    if !init.status.success() {
        return Err(String::from_utf8_lossy(&init.stderr).into_owned());
    }

    // Real counter states: genesis and the first three wakes (1, 3, 2 events).
    let mut cases: Vec<(String, Files)> = vec![
        ("empty snapshot".into(), Files::new()),
        ("counter genesis".into(), genesis.clone()),
    ];
    let mut cur = genesis.clone();
    for (s, n) in [(1u64, 1usize), (2, 3), (3, 2)] {
        let o = wamr_step(&cur, &context(s), &echo_events(s, n), &[])?;
        cur = o.files;
        let count = String::from_utf8_lossy(&cur["data/count"]).into_owned();
        cases.push((
            format!("counter after wake {s} (count={count})"),
            cur.clone(),
        ));
    }
    cases.push((
        "sort edge: foo-bar < foo/ < foo0".into(),
        files(&[
            ("data/foo-bar", b"dash"),
            ("data/foo/x", b"dir"),
            ("data/foo0", b"zero"),
            ("data/foo.txt", b"dot"),
        ]),
    ));
    let all_bytes: Vec<u8> = (0..=255u8).collect();
    cases.push((
        "binary + deep nesting".into(),
        files(&[
            ("data/blob.bin", &all_bytes),
            ("logs/a/b/c/d.jsonl", b"{}\n"),
            ("code/bin/step.wasm", b"\0asm\x01\0\0\0"),
        ]),
    ));

    for (name, f) in &cases {
        let fs_hash = coglet_fs::Snapshot::from_files(slash(f))
            .map_err(|e| format!("{name}: coglet-fs {e}"))?
            .tree_hash();
        let rt_hash = coglet_runtime::Snapshot::new(hex_map(f))
            .and_then(|s| s.tree_hash())
            .map_err(|e| format!("{name}: coglet-runtime {e}"))?;
        let uf_hash = unforged_gitobj::snapshot_tree_hash_sha256(f.iter())
            .map_err(|e| format!("{name}: unforged-gitobj {e}"))?;
        let git_hash = git_tree(&repo, &index, f)?;
        let pass =
            fs_hash == rt_hash && rt_hash == uf_hash && uf_hash == git_hash && git_hash.len() == 64;
        r.check(
            format!("{name}: coglet-fs == coglet-runtime == unforged-gitobj == git"),
            pass,
            if pass {
                fs_hash.clone()
            } else {
                format!("fs={fs_hash} runtime={rt_hash} gitobj={uf_hash} git={git_hash}")
            },
        );
        r.hash(name.clone(), fs_hash);
    }
    let empty = r.hashes[0].1.clone();
    r.check_eq(
        "empty snapshot == SHA-256 empty tree",
        empty.as_str(),
        Oid256::EMPTY_TREE_HEX,
    );

    // Mutual rejection of unmappable snapshots.
    let dotgit = files(&[("data/.git/config", b"x")]);
    let collide = files(&[("data/a", b"file"), ("data/a/b", b"child")]);
    for (name, f) in [
        (".git component", &dotgit),
        ("file/directory alias", &collide),
    ] {
        let fs_err = coglet_fs::Snapshot::from_files(slash(f)).is_err();
        let rt_err = coglet_runtime::Snapshot::new(hex_map(f))
            .and_then(|s| s.tree_hash())
            .is_err();
        let uf_err = unforged_gitobj::snapshot_tree_hash_sha256(f.iter()).is_err();
        let git_err = git_tree(&repo, &index, f).is_err();
        r.check(
            format!("{name}: refused by coglet-fs, coglet-runtime, unforged-gitobj and git"),
            fs_err && rt_err && uf_err && git_err,
            format!("coglet-fs={fs_err} runtime={rt_err} gitobj={uf_err} git={git_err}"),
        );
    }

    // Tree shapes coglet-fs cannot even express are refused by unforged CogletStrict.
    let store = LooseObjectStore::new(&os.git.git_dir);
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    let blob = store.write_blob(b"target").map_err(e)?;
    let ok_file = TreeEntry::file("count", hash_blob(b"6"));
    store.write_blob(b"6").map_err(e)?;
    let empty_tree = store.write_tree(&[], TreeProfile::Standard).map_err(e)?;
    let shapes: Vec<(&str, Vec<TreeEntry>)> = vec![
        (
            "symlink (120000)",
            vec![ok_file.clone(), TreeEntry::symlink("link", blob)],
        ),
        (
            "executable (100755)",
            vec![ok_file.clone(), TreeEntry::executable("run", blob)],
        ),
        (
            "gitlink (160000)",
            vec![ok_file.clone(), TreeEntry::gitlink("sub", blob)],
        ),
        (
            "empty subtree",
            vec![ok_file.clone(), TreeEntry::dir("void", empty_tree)],
        ),
    ];
    for (name, entries) in shapes {
        let inner = store
            .write_tree(&entries, TreeProfile::Standard)
            .map_err(e)?;
        let root = store
            .write_tree(&[TreeEntry::dir("data", inner)], TreeProfile::Standard)
            .map_err(e)?;
        let strict_encode = encode_tree(&entries, TreeProfile::CogletStrict).is_err();
        let (_, payload) = store.read_object(&inner).map_err(e)?;
        let strict_decode =
            decode_tree(&payload, TreeProfile::CogletStrict).is_err() || name == "empty subtree";
        let walk = validate_local_tree_coglet_strict(&os.git, &root.to_hex());
        r.check(
            format!("{name}: refused by TreeProfile::CogletStrict"),
            walk.is_err() && (strict_encode || name == "empty subtree") && strict_decode,
            walk.err().unwrap_or_else(|| "accepted".into()),
        );
    }
    let _ = ObjectKind::Tree;
    let _ = std::fs::remove_dir_all(&repo);
    Ok(r.finish())
}
