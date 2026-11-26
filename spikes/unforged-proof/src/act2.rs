//! Act 2 — Stage 2: a 100-step real-counter chain replayed by unforged's stock
//! `replay_step_chain_in_store_with_engine` (coglet-wamr-step over `--batch-stdio`:
//! one engine process, zero `git` spawns), plus ingest #2 (two SHA-1 maintainer signatures over one
//! counter policy tree, translated to SHA-256 and verified).
use crate::counter::{Files, context, echo_events};
use crate::report::ActReport;
use crate::unf::{HotChain, hot_run, pin};
use gecaos::coglet::{
    CommitSpec, ExternalCommandStepEngine, ExternalStepProtocol, IdentSpec, LooseObjectStore,
    ObjectId, Oid256, RECEIPTS_NOTES_REF, StepContext, StepEngine, build_coglet_resolver,
    build_counter_coglet_wasm, collect_recorded_chain_inproc, import_sha1_commit_with_envelope,
    load_strict_tree_inproc, replay_step_chain_in_store, replay_step_chain_in_store_with_engine,
    step_coglet_hot_inproc, verify_translated_commit_signature,
};
use gecaos::git::{GitEnv, git_spawn_count, reset_git_spawn_count};
use gecaos::os::GecaosOs;
use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub const CHAIN_LEN: u64 = 100;

struct Replay {
    commits: Vec<String>,
    notes_commit: String,
    spawns: u64,
    engine_spawns: u64,
    engine_steps: u64,
    micros: u128,
    final_count: String,
}

/// The 100-step chain replayed by unforged's STOCK
/// `replay_step_chain_in_store_with_engine` with an `ExternalCommandStepEngine` in
/// `--batch-stdio` mode. The engine is materialized in-process from the resolver's own
/// `engines/coglet-engine` blob (the exact `coglet-wamr-step` bytes Act 1 pinned), so
/// the `Engine:` trailer, the resolver pin and the bytes that ran are one object.
/// unforged reads/writes every object, validates CogletStrict, re-derives every
/// StepCommit and builds the `refs/notes/receipts` commit; the helper only computes.
fn replay_chain_wamr(
    os: &GecaosOs,
    coglet_id: &str,
    state_0: &str,
    tip: &str,
    resolver: &str,
    prev_notes: Option<&str>,
) -> Result<Replay, String> {
    let git_dir = &os.git.git_dir;
    reset_git_spawn_count();
    let started = Instant::now();
    let steps = collect_recorded_chain_inproc(git_dir, state_0, tip)?;
    let mut engine = ExternalCommandStepEngine::from_resolver(
        git_dir,
        resolver,
        ExternalStepProtocol::BatchStdio,
    )?;
    let rep = replay_step_chain_in_store_with_engine(
        git_dir,
        coglet_id,
        state_0,
        &steps,
        resolver,
        prev_notes,
        &mut engine,
    )?;
    engine.finish()?;
    let micros = started.elapsed().as_micros();
    let spawns = git_spawn_count();
    let store = LooseObjectStore::new(git_dir);
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    let tree = store
        .read_commit(&Oid256::from_hex_str(&rep.final_commit_oid).map_err(e)?)
        .map_err(e)?
        .tree;
    let mut cache = HashMap::new();
    let files: Files = load_strict_tree_inproc(&store, None, &tree, &mut cache)?;
    Ok(Replay {
        commits: rep.step_commits,
        notes_commit: rep.receipts_notes_commit_oid,
        spawns,
        engine_spawns: engine.spawn_count(),
        engine_steps: engine.steps_executed(),
        micros,
        final_count: String::from_utf8_lossy(files.get("data/count").map_or(&[][..], |v| v))
            .into_owned(),
    })
}

fn sh(cmd: &mut Command, stdin: Option<&[u8]>) -> Result<String, String> {
    let mut child = cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(b) = stdin {
        child
            .stdin
            .take()
            .ok_or("stdin")?
            .write_all(b)
            .map_err(|e| e.to_string())?;
    }
    drop(child.stdin.take());
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "{cmd:?}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Ingest #2: two maintainers sign two SHA-1 commits over the identical counter
/// policy tree; both translate to SHA-256 and verify to distinct principals.
fn ingest2(
    r: &mut ActReport,
    os: &GecaosOs,
    coglet_id: &str,
    policy: &Files,
    work: &Path,
) -> Result<(), String> {
    let keys = work.join("keys");
    std::fs::create_dir_all(&keys).map_err(|e| e.to_string())?;
    let mut signers = String::new();
    let mut key_paths = Vec::new();
    for name in ["alice", "bob"] {
        let key = keys.join(name);
        sh(
            Command::new("ssh-keygen")
                .args(["-q", "-t", "ed25519", "-N", "", "-C", name, "-f"])
                .arg(&key),
            None,
        )?;
        let public =
            std::fs::read_to_string(key.with_extension("pub")).map_err(|e| e.to_string())?;
        let two: Vec<&str> = public.split_whitespace().take(2).collect();
        signers.push_str(&format!("{name}@maintainers.example {}\n", two.join(" ")));
        key_paths.push((name, key));
    }
    let signers_file = work.join("allowed_signers");
    std::fs::write(&signers_file, &signers).map_err(|e| e.to_string())?;

    // Upstream SHA-1 repository holding the counter policy tree (code/**).
    let sha1 = work.join("upstream-sha1.git");
    sh(
        Command::new("git")
            .args(["init", "-q", "--bare", "--object-format=sha1"])
            .arg(&sha1),
        None,
    )?;
    let index = work.join("sha1.index");
    let g = |args: &[&str], stdin: Option<&[u8]>| {
        sh(
            Command::new("git")
                .arg("--git-dir")
                .arg(&sha1)
                .args(args)
                .env("GIT_INDEX_FILE", &index),
            stdin,
        )
    };
    for (path, bytes) in policy {
        let oid = g(&["hash-object", "-w", "--stdin"], Some(bytes))?;
        g(
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                &format!("100644,{oid},{path}"),
            ],
            None,
        )?;
    }
    let tree_sha1 = g(&["write-tree"], None)?;
    let local_tree = LooseObjectStore::new(&os.git.git_dir)
        .write_snapshot(policy.iter())
        .map_err(|e| e.to_string())?
        .to_hex();
    r.hash("counter policy tree (SHA-1, upstream)", tree_sha1.clone());
    r.hash(
        "counter policy tree (SHA-256, unforged)",
        local_tree.clone(),
    );

    let policy_ref = format!("refs/coglets/{coglet_id}/policy");
    let mut verified = Vec::new();
    for (name, key) in &key_paths {
        let commit = sh(
            Command::new("git")
                .arg("--git-dir")
                .arg(&sha1)
                .args([
                    "-c",
                    "gpg.format=ssh",
                    "-c",
                    &format!("user.signingkey={}", key.display()),
                ])
                .args([
                    "-c",
                    &format!("user.name={name}"),
                    "-c",
                    &format!("user.email={name}@maintainers.example"),
                ])
                .args([
                    "commit-tree",
                    "-S",
                    &tree_sha1,
                    "-m",
                    &format!("counter policy: {name} endorses"),
                ])
                .env("GIT_AUTHOR_DATE", "1700001000 +0000")
                .env("GIT_COMMITTER_DATE", "1700001000 +0000"),
            None,
        )?;
        let payload = Command::new("git")
            .arg("--git-dir")
            .arg(&sha1)
            .args(["cat-file", "commit", &commit])
            .output()
            .map_err(|e| e.to_string())?
            .stdout;
        let (src, local) = import_sha1_commit_with_envelope(
            os,
            &commit,
            &payload,
            &local_tree,
            Some(&policy_ref),
            &format!("ingest counter policy from {name}"),
            None,
        )?;
        let v = verify_translated_commit_signature(os, &local, Some(&signers_file));
        r.check(
            format!("ingest #2: {name}'s SHA-1 commit translates and verifies"),
            v.as_ref().is_ok_and(|v| {
                v.principal == format!("{name}@maintainers.example")
                    && v.source_commit_sha1 == src
                    && v.source_tree_sha1 == tree_sha1
                    && v.local_tree_sha256 == local_tree
            }),
            match &v {
                Ok(v) => format!(
                    "principal={} sha1:{} -> {}",
                    v.principal, v.source_commit_sha1, v.local_commit_sha256
                ),
                Err(e) => e.clone(),
            },
        );
        r.hash(
            format!("{name}: translated SHA-256 commit (Source-Commit trailer)"),
            local.clone(),
        );
        verified.push((src, local));
    }
    r.check(
        "ingest #2: distinct SHA-1 sources -> distinct SHA-256 commits over one tree",
        verified.len() == 2 && verified[0].0 != verified[1].0 && verified[0].1 != verified[1].1,
        String::new(),
    );
    // Negative control: same signed envelope over a tampered SHA-256 tree.
    let mut tampered = policy.clone();
    tampered.insert("code/manifest.json".into(), b"{}".to_vec());
    let bad_tree = LooseObjectStore::new(&os.git.git_dir)
        .write_snapshot(tampered.iter())
        .map_err(|e| e.to_string())?
        .to_hex();
    let src_payload = Command::new("git")
        .arg("--git-dir")
        .arg(&sha1)
        .args(["cat-file", "commit", &verified[0].0])
        .output()
        .map_err(|e| e.to_string())?
        .stdout;
    let (_, forged) = import_sha1_commit_with_envelope(
        os,
        &verified[0].0,
        &src_payload,
        &bad_tree,
        None,
        "forged",
        None,
    )?;
    let err = verify_translated_commit_signature(os, &forged, Some(&signers_file));
    r.check(
        "ingest #2: tampered SHA-256 tree fails the SHA-1 <-> SHA-256 bijection",
        err.as_ref().is_err_and(|e| e.contains("bijection")),
        err.err().unwrap_or_else(|| "accepted".into()),
    );
    Ok(())
}

/// Control: an external engine whose bytes differ from the resolver's pinned
/// `engines/coglet-engine` blob is refused before any step runs.
fn pin_mismatch_control(
    r: &mut ActReport,
    os: &GecaosOs,
    coglet_id: &str,
    state_0: &str,
    tip: &str,
    resolver: &str,
    work: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let impostor = work.join("impostor-engine");
    std::fs::write(&impostor, b"#!/bin/sh\nexit 0\n").map_err(|e| e.to_string())?;
    let steps = collect_recorded_chain_inproc(&os.git.git_dir, state_0, tip)?;
    let mut engine = ExternalCommandStepEngine::batch_stdio(&impostor);
    let res = replay_step_chain_in_store_with_engine(
        &os.git.git_dir,
        coglet_id,
        state_0,
        &steps,
        resolver,
        None,
        &mut engine,
    );
    r.check(
        "control: engine bytes != resolver's pinned engine blob -> replay refused, 0 engine spawns",
        res.as_ref()
            .is_err_and(|e| e.contains("engine pin mismatch"))
            && engine.spawn_count() == 0,
        res.err().unwrap_or_else(|| "accepted".into()),
    );
    Ok(())
}

/// Control: unforged's own `replay_step_chain_in_store` on its built-in interpreter module.
fn builtin_control(r: &mut ActReport, os: &GecaosOs) -> Result<(), String> {
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    let resolver = build_coglet_resolver(os, "unforged-builtin-interpreter", true, true, None)?;
    let store = LooseObjectStore::new(&os.git.git_dir);
    let wasm = build_counter_coglet_wasm();
    let genesis = store
        .write_snapshot([("code/bin/step.wasm", wasm.as_slice())])
        .map_err(e)?;
    let ident = IdentSpec::new("counter-builtin", "coglet@unforged", 0);
    let state_0 = store
        .write_commit(&CommitSpec::unsigned(
            genesis,
            Vec::new(),
            ident.clone(),
            ident,
            "genesis",
        ))
        .map_err(e)?
        .to_hex();
    let mut state = state_0.clone();
    for s in 1..=CHAIN_LEN {
        let ctx: StepContext = context(s);
        let input_files = crate::counter::encode_input(&ctx, &echo_events(s, 1), &[]);
        let tree = store.write_snapshot(input_files.iter()).map_err(e)?;
        let id = IdentSpec::new("coglet-input", "input@unforged", s);
        let input = store
            .write_commit(&CommitSpec::unsigned(
                tree,
                Vec::new(),
                id.clone(),
                id,
                format!("input {s}"),
            ))
            .map_err(e)?
            .to_hex();
        state = step_coglet_hot_inproc(
            &os.git.git_dir,
            "counter-builtin",
            &state,
            &input,
            &resolver,
            None,
        )?
        .step_commit_oid;
    }
    let env = GitEnv::default();
    let prev = os
        .git
        .rev_parse_opt(RECEIPTS_NOTES_REF, &env)
        .map_err(|e| e.to_string())?;
    let steps = collect_recorded_chain_inproc(&os.git.git_dir, &state_0, &state)?;
    reset_git_spawn_count();
    let rep = replay_step_chain_in_store(
        &os.git.git_dir,
        "counter-builtin",
        &state_0,
        &steps,
        &resolver,
        prev.as_deref(),
    )?;
    let spawns = git_spawn_count();
    pin(os, RECEIPTS_NOTES_REF, &rep.receipts_notes_commit_oid)?;
    pin(os, "refs/coglets/counter-builtin/state", &state)?;
    r.check(
        "control: unforged replay_step_chain_in_store (built-in interpreter) 100/100, 0 git spawns",
        rep.steps_replayed == CHAIN_LEN as usize && rep.final_commit_oid == state && spawns == 0,
        format!(
            "final={} spawns={spawns} wall_us={}",
            &rep.final_commit_oid[..12],
            rep.wall_us
        ),
    );
    Ok(())
}

pub fn run(
    os: &GecaosOs,
    genesis: &Files,
    resolver: &str,
    work: &Path,
) -> Result<ActReport, String> {
    let mut r = ActReport::new(
        "2",
        "Stage 2: 100-step chain replay (0 git spawns) + SHA-1 signatures",
    );
    let coglet_id = "counter-chain";
    let steps: Vec<_> = (1..=CHAIN_LEN)
        .map(|s| (context(s), echo_events(s, 1), Vec::new()))
        .collect();
    let chain: HotChain = hot_run(os, coglet_id, genesis, &steps, resolver)?;
    r.check_eq(
        "hot path: 100 WAMR steps, 0 git spawns",
        chain.git_spawns,
        0,
    );
    r.check(
        "hot path: final /data/count == \"100\"",
        chain
            .outcomes
            .last()
            .and_then(|o| o.files.get("data/count"))
            .map(Vec::as_slice)
            == Some(b"100"),
        format!("{} ms", chain.micros / 1000),
    );
    let tip = chain.commits.last().cloned().unwrap_or_default();
    r.hash("hot tip StepCommit (step 100)", tip.clone());

    let env = GitEnv::default();
    let prev = os
        .git
        .rev_parse_opt(RECEIPTS_NOTES_REF, &env)
        .map_err(|e| e.to_string())?;
    let rep = replay_chain_wamr(
        os,
        coglet_id,
        &chain.state_0,
        &tip,
        resolver,
        prev.as_deref(),
    )?;
    r.check_eq(
        "chain replay (stock replay_step_chain_in_store_with_engine): 0 git spawns",
        rep.spawns,
        0,
    );
    r.check(
        "chain replay: ExternalCommandStepEngine --batch-stdio spawned coglet-wamr-step once for 100 steps",
        rep.engine_spawns == 1 && rep.engine_steps == CHAIN_LEN,
        format!("spawns={} steps={}", rep.engine_spawns, rep.engine_steps),
    );
    let matched = rep
        .commits
        .iter()
        .zip(&chain.commits)
        .filter(|(a, b)| a == b)
        .count();
    r.check(
        "chain replay: 100/100 StepCommit ids == hot path",
        matched == CHAIN_LEN as usize && rep.commits.len() == CHAIN_LEN as usize,
        format!(
            "{matched}/{} in {} ms",
            rep.commits.len(),
            rep.micros / 1000
        ),
    );
    r.check_eq(
        "chain replay: final /data/count",
        rep.final_count.as_str(),
        "100",
    );
    pin(os, RECEIPTS_NOTES_REF, &rep.notes_commit)?;
    let receipt = gecaos::coglet::read_latest_step_receipt(&os.git, &tip)
        .ok()
        .flatten();
    r.check(
        "chain replay: tip receipt in refs/notes/receipts executed=true cache=none",
        receipt
            .as_ref()
            .is_some_and(|x| x.executed && x.cache == "none" && x.commit == tip),
        receipt
            .map(|x| x.to_json())
            .unwrap_or_else(|| "missing".into()),
    );
    r.hash(
        "refs/notes/receipts after chain replay",
        rep.notes_commit.clone(),
    );

    pin_mismatch_control(&mut r, os, coglet_id, &chain.state_0, &tip, resolver, work)?;
    builtin_control(&mut r, os)?;

    let policy: Files = genesis
        .iter()
        .filter(|(p, _)| p.starts_with("code/"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    ingest2(&mut r, os, "counter", &policy, work)?;
    Ok(r.finish())
}
