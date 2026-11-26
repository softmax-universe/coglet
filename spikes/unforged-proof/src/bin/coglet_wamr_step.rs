#![deny(warnings)]
//! `coglet-wamr-step`: the cold-path engine pinned in the unforged resolver tree.
//!
//! unforged's `execute_coglet_step` stages `{.coglet_step_ours/, .coglet_step_theirs/}`
//! in a sandbox worktree and runs `/run/resolver/drivers/coglet-step`, which execs
//! this binary from `/run/resolver/bin/`. It runs exactly one step of the real
//! counter guest through coglet's `Runtime::step` + WAMR (the same function the hot
//! path calls) and replaces the worktree with the `{ status, state/ }` envelope.
use coglet_unforged_proof::counter::{Files, decode_input, wamr_step};
use std::path::Path;

fn read_dir_strict(root: &Path, cur: &Path, out: &mut Files) -> Result<(), String> {
    if !cur.exists() {
        return Ok(());
    }
    let mut items: Vec<_> = std::fs::read_dir(cur)
        .map_err(|e| e.to_string())?
        .map(|e| e.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    items.sort();
    for path in items {
        let rel = path
            .strip_prefix(root)
            .map_err(|e| e.to_string())?
            .to_string_lossy()
            .into_owned();
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() {
            return Err(format!("CogletStrict rejects symlink at `{rel}`"));
        }
        if meta.is_dir() {
            read_dir_strict(root, &path, out)?;
        } else {
            unforged_gitobj::parse_snapshot_path(&rel).map_err(|e| e.to_string())?;
            out.insert(rel, std::fs::read(&path).map_err(|e| e.to_string())?);
        }
    }
    Ok(())
}

fn step_worktree(wt: &Path) -> Result<(), String> {
    let mut ours = Files::new();
    let ours_dir = wt.join(".coglet_step_ours");
    read_dir_strict(&ours_dir, &ours_dir, &mut ours)?;
    let mut theirs = Files::new();
    let theirs_dir = wt.join(".coglet_step_theirs");
    read_dir_strict(&theirs_dir, &theirs_dir, &mut theirs)?;
    let (ctx, events, boundary) = decode_input(&theirs)?;
    let outcome = wamr_step(&ours, &ctx, &events, &boundary)?;

    for entry in std::fs::read_dir(wt).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name() == ".git" {
            continue;
        }
        let p = entry.path();
        let meta = std::fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        if meta.is_dir() {
            std::fs::remove_dir_all(&p).map_err(|e| e.to_string())?;
        } else {
            std::fs::remove_file(&p).map_err(|e| e.to_string())?;
        }
    }
    let status = if outcome.aborted {
        "aborted\n"
    } else {
        "committed\n"
    };
    std::fs::write(wt.join("status"), status).map_err(|e| e.to_string())?;
    for (rel, bytes) in &outcome.files {
        let dst = wt.join("state").join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&dst, bytes).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let result = match args.get(1).map(String::as_str) {
        Some("step-worktree") => step_worktree(Path::new(args.get(2).map_or(".", String::as_str))),
        _ => Err("usage: coglet-wamr-step step-worktree <dir>".to_string()),
    };
    if let Err(e) = result {
        eprintln!("coglet-wamr-step: {e}");
        std::process::exit(1);
    }
}
