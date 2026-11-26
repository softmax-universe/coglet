//! Act 2 — Stage 2: a 100-step real-counter chain replayed in one in-process job
//! with zero `git` spawns, plus ingest #2 (two SHA-1 maintainer signatures over one
//! counter policy tree, translated to SHA-256 and verified).
use crate::counter::{Files, context, decode_input, echo_events, wamr_step};
use crate::report::ActReport;
use crate::unf::{HotChain, hot_run, pin};
use gecaos::coglet::{
    CommitSpec, IdentSpec, LooseObjectStore, MemoryObjectBatch, ObjectId, ObjectKind, Oid256,
    RECEIPTS_NOTES_REF, StepCommitSpec, StepContext, StepReceipt, TreeEntry, TreeProfile,
    build_coglet_resolver, build_counter_coglet_wasm, collect_recorded_chain_inproc,
    import_sha1_commit_with_envelope, load_strict_tree_inproc, replay_step_chain_in_store,
    step_coglet_hot_inproc, verify_translated_commit_signature,
};
use gecaos::git::{GitEnv, git_spawn_count, reset_git_spawn_count};
use gecaos::os::GecaosOs;
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

pub const CHAIN_LEN: u64 = 100;

struct Replay {
    commits: Vec<String>,
    notes_commit: String,
    spawns: u64,
    micros: u128,
    final_count: String,
}

/// Read every note in a notes tree (flat or fanned out) as `target -> body`.
fn read_notes(
    store: &LooseObjectStore,
    tree: &Oid256,
    prefix: &str,
    out: &mut BTreeMap<String, String>,
) -> Result<(), String> {
    for e in store
        .read_tree(tree, TreeProfile::Standard)
        .map_err(|e| e.to_string())?
    {
        let name = format!("{prefix}{}", e.name);
        if e.is_dir() {
            read_notes(store, &e.oid, &name, out)?;
        } else {
            let body = store.read_blob(&e.oid).map_err(|e| e.to_string())?;
            out.insert(name, String::from_utf8_lossy(&body).trim().to_string());
        }
    }
    Ok(())
}

/// Single-job chain replay of the REAL counter through coglet WAMR, built from
/// unforged's in-process primitives (`collect_recorded_chain_inproc`,
/// `load_strict_tree_inproc`, `MemoryObjectBatch`, `StepCommitSpec`). unforged's own
/// `replay_step_chain` is wired to its built-in interpreter, so the engine call is
/// the only piece swapped. The `refs/notes/receipts` commit is built in the batch.
fn replay_chain_wamr(
    os: &GecaosOs,
    coglet_id: &str,
    state_0: &str,
    tip: &str,
    resolver: &str,
    prev_notes: Option<&str>,
) -> Result<Replay, String> {
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    reset_git_spawn_count();
    let started = Instant::now();
    let git_dir = &os.git.git_dir;
    let store = LooseObjectStore::new(git_dir);
    let steps = collect_recorded_chain_inproc(git_dir, state_0, tip)?;
    let mut batch = MemoryObjectBatch::new();
    let mut cache: HashMap<Oid256, (ObjectKind, Vec<u8>)> = HashMap::new();
    let mut cur_commit = Oid256::from_hex_str(state_0).map_err(e)?;
    let state_tree = store.read_commit(&cur_commit).map_err(e)?.tree;
    let mut cur: Files = load_strict_tree_inproc(&store, Some(&batch), &state_tree, &mut cache)?;
    let mut commits = Vec::new();
    let mut receipts = Vec::new();
    for item in &steps {
        let input = Oid256::from_hex_str(&item.input_commit_oid).map_err(e)?;
        let input_tree = store.read_commit(&input).map_err(e)?.tree;
        let theirs = load_strict_tree_inproc(&store, Some(&batch), &input_tree, &mut cache)?;
        let (ctx, events, boundary) = decode_input(&theirs)?;
        let outcome = wamr_step(&cur, &ctx, &events, &boundary)?;
        let tree = batch.stage_snapshot_tree(outcome.files.iter()).map_err(e)?;
        let spec = StepCommitSpec::new(
            coglet_id,
            ctx.logical_time,
            ctx.step,
            tree,
            vec![cur_commit, input],
            resolver,
            outcome.aborted,
        );
        let commit = batch.stage_step_commit(&spec).map_err(e)?;
        let hex = commit.to_hex();
        if let Some(want) = &item.expected_step_commit_oid
            && *want != hex
        {
            return Err(format!(
                "chain replay mismatch at step {}: {want} != {hex}",
                ctx.step
            ));
        }
        receipts.push(StepReceipt {
            runner: "unforged".into(),
            worker_image: None,
            executed: true,
            cache: "none".into(),
            commit: hex.clone(),
        });
        commits.push(hex);
        cur_commit = commit;
        cur = outcome.files;
    }
    let mut notes = BTreeMap::new();
    let mut parents = Vec::new();
    if let Some(prev) = prev_notes {
        let prev = Oid256::from_hex_str(prev).map_err(e)?;
        read_notes(
            &store,
            &store.read_commit(&prev).map_err(e)?.tree,
            "",
            &mut notes,
        )?;
        parents.push(prev);
    }
    for rc in &receipts {
        let line = rc.to_json();
        notes
            .entry(rc.commit.clone())
            .and_modify(|b: &mut String| {
                b.push('\n');
                b.push_str(&line);
            })
            .or_insert(line);
    }
    let entries: Vec<TreeEntry> = notes
        .iter()
        .map(|(t, body)| {
            TreeEntry::file(t.clone(), batch.stage_blob(format!("{body}\n").as_bytes()))
        })
        .collect();
    let notes_tree = batch
        .stage_tree(&entries, TreeProfile::Standard)
        .map_err(e)?;
    let ident = IdentSpec::new("unforged", "receipts@unforged", 0);
    let notes_commit = batch
        .stage_commit(&CommitSpec::unsigned(
            notes_tree,
            parents,
            ident.clone(),
            ident,
            format!(
                "coglet-unforged-proof: WAMR chain replay receipts ({} steps)",
                steps.len()
            ),
        ))
        .map_err(e)?;
    batch.flush_to(&store).map_err(e)?;
    Ok(Replay {
        commits,
        notes_commit: notes_commit.to_hex(),
        spawns: git_spawn_count(),
        micros: started.elapsed().as_micros(),
        final_count: String::from_utf8_lossy(cur.get("data/count").map_or(&[][..], |v| v))
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
    r.check_eq("chain replay: one job, 0 git spawns", rep.spawns, 0);
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

    builtin_control(&mut r, os)?;

    let policy: Files = genesis
        .iter()
        .filter(|(p, _)| p.starts_with("code/"))
        .map(|(p, b)| (p.clone(), b.clone()))
        .collect();
    ingest2(&mut r, os, "counter", &policy, work)?;
    Ok(r.finish())
}
