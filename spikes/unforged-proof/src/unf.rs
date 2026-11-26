//! Thin helpers over real unforged APIs shared by the acts: ref pins, the
//! WAMR resolver commit, the in-process hot path and the cold-path replay.
use crate::counter::{Files, StepOutcome, encode_input, wamr_step};
use gecaos::coglet::{
    BoundaryChange, CogletEvent, CogletStepOptions, CogletStepResult, CommitSpec, IdentSpec,
    LooseObjectStore, MemoryObjectBatch, Oid256, StepCommitSpec, StepContext, TreeEntry,
    TreeProfile, execute_coglet_step,
};
use gecaos::driver::{CachePolicy, LocalDriverRunner};
use gecaos::git::{GitEnv, git_spawn_count, reset_git_spawn_count};
use gecaos::os::GecaosOs;
use std::process::Command;
use std::time::Instant;

pub type StepInput = (StepContext, Vec<CogletEvent>, Vec<BoundaryChange>);

/// Point `refname` at `oid` with a compare-and-swap against its current value.
pub fn pin(os: &GecaosOs, refname: &str, oid: &str) -> Result<(), String> {
    let env = GitEnv::default();
    let prev = os
        .git
        .rev_parse_opt(refname, &env)
        .map_err(|e| e.to_string())?;
    if prev.as_deref() == Some(oid) {
        return Ok(());
    }
    os.git
        .update_ref_cas(refname, oid, prev.as_deref(), "coglet-unforged-proof", &env)
        .map_err(|e| e.to_string())
}

pub fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `git fsck --full` on the unforged repository; returns (ok, combined output).
pub fn fsck(os: &GecaosOs) -> (bool, String) {
    match Command::new("git")
        .arg("--git-dir")
        .arg(&os.git.git_dir)
        .args(["fsck", "--full", "--no-reflogs"])
        .output()
    {
        Ok(o) => (
            o.status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
            .trim()
            .to_string(),
        ),
        Err(e) => (false, e.to_string()),
    }
}

/// Engine pin naming the exact helper bytes and WAMR build.
pub fn engine_pin(helper: &[u8], suffix: &str) -> String {
    format!(
        "coglet-engine-wamr@{}+coglet-wamr-step-sha256:{}{suffix}",
        coglet_engine_wamr::BUILD,
        gecaos::coglet::sha256_hex(helper)
    )
}

/// The driver script: exec the pinned helper from the read-only resolver mount.
pub const DRIVER_SCRIPT: &str = "#!/bin/sh\n\
# coglet-step driver: real coglet WAMR engine pinned at /run/resolver/bin/coglet-wamr-step\n\
exec \"${UNFORGED_RESOLVER_DIR:-/run/resolver}/bin/coglet-wamr-step\" step-worktree \"$PWD\"\n";

/// Build a `coglet-step` resolver commit whose tree pins the real WAMR helper:
/// `strategy`, `drivers/coglet-step` (100755) and `bin/coglet-wamr-step` (100755).
pub fn build_wamr_resolver(
    os: &GecaosOs,
    helper: &[u8],
    engine_pin: &str,
    deterministic: bool,
) -> Result<String, String> {
    let store = LooseObjectStore::new(&os.git.git_dir);
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    let strategy = format!(
        "driver = coglet-step\ndeterministic = {deterministic}\nengine = {engine_pin}\nvalidation = coglet\n"
    );
    let strategy_oid = store.write_blob(strategy.as_bytes()).map_err(e)?;
    let driver_oid = store.write_blob(DRIVER_SCRIPT.as_bytes()).map_err(e)?;
    let helper_oid = store.write_blob(helper).map_err(e)?;
    let bin = store
        .write_tree(
            &[TreeEntry::executable("coglet-wamr-step", helper_oid)],
            TreeProfile::Standard,
        )
        .map_err(e)?;
    let drivers = store
        .write_tree(
            &[TreeEntry::executable("coglet-step", driver_oid)],
            TreeProfile::Standard,
        )
        .map_err(e)?;
    let root = store
        .write_tree(
            &[
                TreeEntry::dir("bin", bin),
                TreeEntry::dir("drivers", drivers),
                TreeEntry::file("strategy", strategy_oid),
            ],
            TreeProfile::Standard,
        )
        .map_err(e)?;
    let ident = IdentSpec::new("unforged-resolver", "resolver@unforged", 0);
    let commit = store
        .write_commit(&CommitSpec::unsigned(
            root,
            Vec::new(),
            ident.clone(),
            ident,
            format!("coglet resolver (engine={engine_pin})"),
        ))
        .map_err(e)?
        .to_hex();
    pin(
        os,
        &format!("refs/gecaos/coglet-resolvers/{commit}"),
        &commit,
    )?;
    Ok(commit)
}

/// A recorded hot-path chain.
pub struct HotChain {
    pub state_0: String,
    pub inputs: Vec<String>,
    pub commits: Vec<String>,
    pub trees: Vec<String>,
    pub outcomes: Vec<StepOutcome>,
    pub git_spawns: u64,
    pub micros: u128,
}

impl HotChain {
    /// The state commit a step `i` started from.
    pub fn state_before(&self, i: usize) -> &str {
        if i == 0 {
            &self.state_0
        } else {
            &self.commits[i - 1]
        }
    }
}

/// `cogletd`-style hot path: every object (genesis, input commits, trees, step
/// commits) is staged in a `MemoryObjectBatch` and flushed once to the loose store.
/// No `git` process is started; `git_spawns` is unforged's own spawn counter.
pub fn hot_run(
    os: &GecaosOs,
    coglet_id: &str,
    genesis: &Files,
    steps: &[StepInput],
    resolver_oid: &str,
) -> Result<HotChain, String> {
    let e = |e: gecaos::coglet::GitObjError| e.to_string();
    reset_git_spawn_count();
    let started = Instant::now();
    let mut batch = MemoryObjectBatch::new();
    let genesis_tree = batch.stage_snapshot_tree(genesis.iter()).map_err(e)?;
    let ident = IdentSpec::new(coglet_id, "coglet@unforged", 0);
    let state_0 = batch
        .stage_commit(&CommitSpec::unsigned(
            genesis_tree,
            Vec::new(),
            ident.clone(),
            ident,
            "genesis",
        ))
        .map_err(e)?;
    let mut prev: Oid256 = state_0;
    let mut cur = genesis.clone();
    let mut chain = HotChain {
        state_0: state_0.to_hex(),
        inputs: Vec::new(),
        commits: Vec::new(),
        trees: Vec::new(),
        outcomes: Vec::new(),
        git_spawns: 0,
        micros: 0,
    };
    for (ctx, events, boundary) in steps {
        let input_files = encode_input(ctx, events, boundary);
        let input_tree = batch.stage_snapshot_tree(input_files.iter()).map_err(e)?;
        let in_ident = IdentSpec::new("coglet-input", "input@unforged", ctx.logical_time);
        let input = batch
            .stage_commit(&CommitSpec::unsigned(
                input_tree,
                Vec::new(),
                in_ident.clone(),
                in_ident,
                format!("input {}", ctx.step),
            ))
            .map_err(e)?;
        let outcome = wamr_step(&cur, ctx, events, boundary)?;
        let tree = batch.stage_snapshot_tree(outcome.files.iter()).map_err(e)?;
        let spec = StepCommitSpec::new(
            coglet_id,
            ctx.logical_time,
            ctx.step,
            tree,
            vec![prev, input],
            resolver_oid,
            outcome.aborted,
        );
        let commit = batch.stage_step_commit(&spec).map_err(e)?;
        chain.inputs.push(input.to_hex());
        chain.commits.push(commit.to_hex());
        chain.trees.push(tree.to_hex());
        cur = outcome.files.clone();
        chain.outcomes.push(outcome);
        prev = commit;
    }
    batch
        .flush_to(&LooseObjectStore::new(&os.git.git_dir))
        .map_err(e)?;
    chain.micros = started.elapsed().as_micros();
    chain.git_spawns = git_spawn_count();
    // Ref moves happen after the measured section (reftable writes go through git).
    pin(
        os,
        &format!("refs/coglets/{coglet_id}/state"),
        &prev.to_hex(),
    )?;
    if let Some(last) = chain.inputs.last() {
        pin(os, &format!("refs/coglets/{coglet_id}/inputs"), last)?;
    }
    Ok(chain)
}

/// Replay every recorded step of `chain` as one unforged `coglet-step` job each.
pub fn cold_replay(
    os: &GecaosOs,
    coglet_id: &str,
    chain: &HotChain,
    resolver_oid: &str,
    cache_policy: CachePolicy,
) -> Result<Vec<CogletStepResult>, String> {
    let runner = LocalDriverRunner::new();
    let opts = CogletStepOptions {
        coglet_id: coglet_id.to_string(),
        resolver_oid: resolver_oid.to_string(),
        cache_policy,
        advance_state_ref: false,
    };
    (0..chain.commits.len())
        .map(|i| execute_coglet_step(os, chain.state_before(i), &chain.inputs[i], &opts, &runner))
        .collect()
}
