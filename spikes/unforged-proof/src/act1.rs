//! Act 1 — Stage 1, David's "first test" (`docs/cogent-design/unforged.md#first-test`)
//! on the real `coglets/counter` guest.
use crate::counter::{Files, context, echo_events};
use crate::report::ActReport;
use crate::unf::{
    HotChain, StepInput, build_wamr_resolver, cold_replay, engine_pin, hot_run, is_hex64,
    resolver_file,
};
use gecaos::coglet::{
    BoundaryChange, COGLET_ENGINE_RESOLVER_PATH, CogletStepResult, LooseObjectStore, ObjectId,
    Oid256, StepReceipt, StepUsage, decode_commit, parse_engine_pin_blob_oid,
    read_latest_step_receipt, read_step_receipts, record_step_receipt, record_step_usage,
};
use gecaos::driver::{CachePolicy, DriverSource, ResolverConfig};
use gecaos::os::GecaosOs;
use std::collections::BTreeMap;

pub struct Act1Output {
    pub report: ActReport,
    pub chain: HotChain,
    pub resolver: String,
}

fn trailer(os: &GecaosOs, commit: &str, key: &str) -> Option<String> {
    let store = LooseObjectStore::new(&os.git.git_dir);
    let oid = Oid256::from_hex_str(commit).ok()?;
    let (_, payload) = store.read_object(&oid).ok()?;
    let dec = decode_commit(&payload).ok()?;
    dec.trailers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.trim().to_string())
}

fn log_lines(files: &Files) -> Vec<String> {
    files
        .get("logs/events.jsonl")
        .map(|b| {
            String::from_utf8_lossy(b)
                .lines()
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Check one cold replay pass against the hot chain and the expected receipt.
fn check_pass(
    r: &mut ActReport,
    os: &GecaosOs,
    label: &str,
    hot: &HotChain,
    results: &[CogletStepResult],
    expect_commits_equal: bool,
    executed: bool,
    cache: &str,
) {
    for (i, res) in results.iter().enumerate() {
        let n = i + 1;
        if expect_commits_equal {
            r.check_eq(
                format!("{label}: step {n} StepCommit == hot path"),
                res.step_commit_oid.as_str(),
                hot.commits[i].as_str(),
            );
        } else {
            r.check(
                format!("{label}: step {n} StepCommit differs (Engine trailer moved)"),
                res.step_commit_oid != hot.commits[i],
                res.step_commit_oid.clone(),
            );
        }
        r.check_eq(
            format!("{label}: step {n} tree == hot path tree"),
            res.state_tree_oid.as_str(),
            hot.trees[i].as_str(),
        );
        let noted = read_latest_step_receipt(&os.git, &res.step_commit_oid)
            .ok()
            .flatten();
        let ok = noted.as_ref().is_some_and(|rc| {
            rc.executed == executed
                && rc.cache == cache
                && rc.runner == "unforged"
                && rc.commit == res.step_commit_oid
        });
        r.check(
            format!("{label}: step {n} refs/notes/receipts executed={executed} cache={cache}"),
            ok,
            noted
                .map(|rc| rc.to_json())
                .unwrap_or_else(|| "missing".into()),
        );
    }
}

pub fn steps() -> Vec<StepInput> {
    let mut v: Vec<StepInput> = [1usize, 3, 2]
        .iter()
        .enumerate()
        .map(|(i, &n)| {
            let s = i as u64 + 1;
            (context(s), echo_events(s, n), Vec::new())
        })
        .collect();
    // Step 4: a manage write sets data/count to u64::MAX, then one event: the guest's
    // checked_add overflows (ResourceExhausted) and the whole step must roll back.
    v.push((
        context(4),
        echo_events(4, 1),
        vec![BoundaryChange::Write {
            path: "data/count".into(),
            data: u64::MAX.to_string().into_bytes(),
        }],
    ));
    v
}

pub fn run(
    os: &GecaosOs,
    coglet_id: &str,
    genesis: &Files,
    helper: &[u8],
) -> Result<Act1Output, String> {
    let mut r = ActReport::new(
        "1",
        "Stage 1: first test on real coglets/counter (hot vs cold)",
    );
    let pin = engine_pin(helper, "");
    let resolver = build_wamr_resolver(os, helper, "", true)?;
    r.hash("resolver (deterministic, engine pinned)", resolver.clone());
    r.note(format!("engine = {pin}"));
    // The resolver is unforged's stock engine-bin resolver: the helper bytes live
    // in the resolver tree and unforged's own driver execs them.
    let engine = resolver_file(os, &resolver, COGLET_ENGINE_RESOLVER_PATH);
    r.check(
        "resolver: engines/coglet-engine is the coglet-wamr-step helper (100755, same bytes)",
        engine
            .as_ref()
            .is_ok_and(|(mode, bytes)| mode == "100755" && bytes.as_slice() == helper),
        match &engine {
            Ok((mode, bytes)) => format!("{mode} {} bytes", bytes.len()),
            Err(e) => e.clone(),
        },
    );
    let strategy = resolver_file(os, &resolver, "strategy")
        .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    r.check(
        "resolver: strategy pins engine = <label>@sha256:<engine blob oid>",
        strategy
            .lines()
            .any(|l| l.trim() == format!("engine = {pin}"))
            && parse_engine_pin_blob_oid(&pin).as_deref()
                == Some(unforged_gitobj::hash_blob(helper).to_hex().as_str()),
        strategy
            .lines()
            .find(|l| l.starts_with("engine"))
            .unwrap_or("")
            .to_string(),
    );
    let driver = resolver_file(os, &resolver, "drivers/coglet-step")
        .map(|(_, b)| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    r.check(
        "resolver: unforged's drivers/coglet-step execs engines/coglet-engine, no built-in fallback",
        driver.contains(COGLET_ENGINE_RESOLVER_PATH) && !driver.contains("coglet-step-worktree"),
        format!("{} bytes", driver.len()),
    );

    // ---- Hot path: in-process cogletd-style writer -------------------------------
    let chain = hot_run(os, coglet_id, genesis, &steps(), &resolver)?;
    r.check_eq("hot path: 0 git subprocess spawns", chain.git_spawns, 0);
    let expect = [("1", 1usize), ("4", 2), ("6", 3)];
    for (i, (count, lines)) in expect.iter().enumerate() {
        let o = &chain.outcomes[i];
        r.check(
            format!("hot step {}: committed, /data/count == {count:?}", i + 1),
            !o.aborted && o.files.get("data/count").map(Vec::as_slice) == Some(count.as_bytes()),
            format!(
                "count={:?} aborted={}",
                o.files
                    .get("data/count")
                    .map(|b| String::from_utf8_lossy(b).into_owned()),
                o.aborted
            ),
        );
        let logs = log_lines(&o.files);
        r.check(
            format!(
                "hot step {}: logs/events.jsonl has {lines} lines, last count={count}",
                i + 1
            ),
            logs.len() == *lines
                && logs
                    .last()
                    .is_some_and(|l| l.contains(&format!("count={count}"))),
            logs.last().cloned().unwrap_or_default(),
        );
    }
    let ab = &chain.outcomes[3];
    r.check(
        "hot step 4 (u64::MAX overflow): aborted",
        ab.aborted,
        ab.reason.clone().unwrap_or_default(),
    );
    r.check_eq(
        "hot step 4: tree == step 3 tree (atomic rollback, boundary undone)",
        chain.trees[3].as_str(),
        chain.trees[2].as_str(),
    );
    r.check(
        "hot step 4: out/ empty",
        !ab.files.keys().any(|p| p.starts_with("out/")),
        format!("{} paths", ab.files.len()),
    );
    for (i, c) in chain.commits.iter().enumerate() {
        let status = trailer(os, c, "Status").unwrap_or_default();
        let want = if i == 3 { "aborted" } else { "committed" };
        r.check(
            format!(
                "hot step {}: 64-hex StepCommit, Status: {want}, Engine: resolver",
                i + 1
            ),
            is_hex64(c)
                && status == want
                && trailer(os, c, "Engine").as_deref() == Some(resolver.as_str()),
            format!("Status: {status}"),
        );
        r.hash(format!("hot StepCommit {}", i + 1), c.clone());
    }

    // ---- Cold path: unforged coglet-step jobs through LocalDriverRunner ----------
    let cfg = ResolverConfig::load(&os.git, &resolver)?;
    let drv = cfg.resolve_driver_name("coglet-step", &BTreeMap::new());
    r.check(
        "resolver: coglet-step resolves to /run/resolver/drivers/coglet-step (not unforged's built-in)",
        cfg.deterministic
            && cfg.requires_coglet_strict()
            && matches!(&drv, Some((DriverSource::Resolver(p), oid)) if p == "drivers/coglet-step" && is_hex64(oid)),
        format!("{drv:?}"),
    );
    // Hot-path receipts and usage, as cogletd would record them (git notes; after the measured section).
    for (i, c) in chain.commits.iter().enumerate() {
        let rc = StepReceipt {
            runner: "cogletd".into(),
            worker_image: None,
            executed: true,
            cache: "none".into(),
            commit: c.clone(),
        };
        record_step_receipt(&os.git, c, &rc)?;
        let usage = StepUsage {
            compute: chain.outcomes[i].compute,
            wall_us: 0,
            mem_bytes: 0,
        };
        record_step_usage(&os.git, c, &usage)?;
    }
    let bypass = cold_replay(os, coglet_id, &chain, &resolver, CachePolicy::Bypass)?;
    check_pass(
        &mut r,
        os,
        "replay --no-cache",
        &chain,
        &bypass,
        true,
        true,
        "none",
    );

    let prime = cold_replay(os, coglet_id, &chain, &resolver, CachePolicy::Default)?;
    check_pass(
        &mut r,
        os,
        "cache-on run 1 (cold cache)",
        &chain,
        &prime,
        true,
        true,
        "none",
    );
    let cached = cold_replay(os, coglet_id, &chain, &resolver, CachePolicy::Default)?;
    check_pass(
        &mut r,
        os,
        "cache-on run 2",
        &chain,
        &cached,
        true,
        false,
        "local",
    );

    let bumped = build_wamr_resolver(os, helper, "+bump1", true)?;
    r.hash("resolver (engine pin bumped only)", bumped.clone());
    let bump = cold_replay(os, coglet_id, &chain, &bumped, CachePolicy::Default)?;
    check_pass(
        &mut r,
        os,
        "engine pin bumped",
        &chain,
        &bump,
        false,
        true,
        "none",
    );

    let nondet = build_wamr_resolver(os, helper, "", false)?;
    r.hash("resolver (deterministic = false)", nondet.clone());
    let _ = cold_replay(os, coglet_id, &chain, &nondet, CachePolicy::Default)?;
    let again = cold_replay(os, coglet_id, &chain, &nondet, CachePolicy::Default)?;
    for (i, res) in again.iter().enumerate() {
        r.check(
            format!(
                "deterministic=false: step {} never memoized (2nd cache-on run executed)",
                i + 1
            ),
            res.receipt.executed && res.receipt.cache == "none",
            res.receipt.to_json(),
        );
    }
    // Tamper control: a resolver pinning a helper that cannot run makes the job fail
    // (unforged maps the driver failure to an aborted step), so the matches above
    // came from the pinned WAMR helper actually executing.
    let broken = b"#!/bin/sh\nexit 3\n";
    let broken_res = build_wamr_resolver(os, broken, "", true)?;
    let tampered = cold_replay(os, coglet_id, &chain, &broken_res, CachePolicy::Bypass)?;
    r.check(
        "tamper control: helper that exits 3 -> step 1 aborted, tree != hot tree",
        tampered[0].aborted && tampered[0].state_tree_oid != chain.trees[0],
        format!(
            "aborted={} tree={}",
            tampered[0].aborted,
            &tampered[0].state_tree_oid[..12]
        ),
    );
    for c in &chain.commits {
        let all = read_step_receipts(&os.git, c).unwrap_or_default();
        r.note(format!(
            "{}: {}",
            &c[..12],
            all.iter()
                .map(|x| format!("{}/{}/{}", x.runner, x.executed, x.cache))
                .collect::<Vec<_>>()
                .join(" -> ")
        ));
    }
    Ok(Act1Output {
        report: r.finish(),
        chain,
        resolver,
    })
}
