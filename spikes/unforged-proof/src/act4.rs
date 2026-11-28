//! Act 4 — Stage 4 of unforged on the counter coglet: `/net/<adapter>` mounts with
//! `fd 3` `0x20 OP_STAGE_ACTION` staging and `refs/notes/dispatched` idempotency, a
//! `policy` patch that pauses on an ungranted capability (`PAUSED_CONFLICT`) and then
//! merges cleanly with a deterministic `data/count` migration, and a vault secret lease
//! that never reaches a git object.
//!
//! Everything runs on the shared `GecaosOs` under a coglet id of this act's own
//! (`<coglet>-adapters`), seeded from the counter tip's tree, so the refs Act 3 moved
//! are never touched.
use crate::report::{ActReport, CounterState};
use gecaos::coglet_adapters::{
    ActionDispatchItemOutcome, AdapterDef, AdapterIpcSession, AdapterKind, CogletPatchRequest,
    CogletStepIpcContext, DISPATCHED_NOTES_REF, DispatchApprovalSet, MemoryVaultBackend,
    PatchConflictKind, PatchMergeOutcome, SecretKind, SecretRecord, SecretState, StagedAction,
    ToolApproval, ToolDef, ToolEffect, VAULT_AUDIT_REF, VAULT_POLICY_REF, VaultLeaseError,
    VaultLeaseService, VaultSecretBackend, adapter_catalog_ref, apply_coglet_patch,
    coglet_grants_ref, coglet_policy_ref, coglet_state_ref, commit_vault_policy_raw,
    dispatch_staged_actions, get_coglet_paused_state, publish_adapter_catalog,
    publish_coglet_grants, publish_secret_record, read_and_validate_coglet_strict_tree,
    read_dispatched_action_ids, read_vault_audit_log, resolve_paused_coglet, scrub_leased_secrets,
    write_coglet_strict_tree_from_blob_oids,
};
use gecaos::git::GitEnv;
use gecaos::ipc::IpcServer;
use gecaos::os::GecaosOs;
use gecaos::proc::{DESC_TAG_CONFLICT, DESC_TAG_DURABLE, decode_oid_descriptor};
use std::collections::BTreeMap;
use std::fmt::Display;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Arc;
use unforged_gitobj::{CommitSpec, IdentSpec, LooseObjectStore, Oid256};

const ADAPTER: &str = "github";
const CAP_ISSUE: &str = "mcp.github.create_issue";
const CAP_DELETE: &str = "mcp.github.delete_repo";
/// The capability the patched manifest asks for before it is granted.
const CAP_CLOSE: &str = "mcp.github.close_issue";
const ISSUE_SCHEMA: &str =
    r#"{"type":"object","properties":{"title":{"type":"string"}},"required":["title"]}"#;
const ACTION_ID: &str = "42";
const ACTION_PAYLOAD: &[u8] = br#"{"title":"counter reached 6"}"#;
const SECRET_ID: &str = "github-token";
const SECRET_STORE: &str = "mem:github-token";
/// Synthetic token, only ever held by `MemoryVaultBackend` and the in-memory lease.
const SECRET: &[u8] = b"tok_act4_synthetic_lease_9f2c71d04be3";
/// Deterministic `data/**` migration: `count` is rescaled x10 and a schema marker added.
/// Run by `apply_coglet_patch` as `migrate <base-data> <ours-data>`.
const MIGRATE: &[u8] = b"#!/bin/sh\nset -eu\nn=$(cat \"$2/count\")\nprintf '%s' \"$((n * 10))\" > \"$2/count.next\"\nmv \"$2/count.next\" \"$2/count\"\nprintf '2' > \"$2/schema\"\n";
const LOGICAL_T0: u64 = 1_700_000_400;

pub fn run(cs: &CounterState) -> Result<ActReport, String> {
    let mut r = ActReport::new(
        "4",
        "Stage 4: /net/<adapter> fd 3 staging, PAUSED_CONFLICT policy patch & vault secret lease",
    );
    let os = cs.os;
    let env = GitEnv::default();
    let cid = format!("{}-adapters", cs.coglet_id);

    // Setup: the counter tip's CogletStrict tree as `path -> blob oid`.
    let tip_tree = rev_parse(os, &format!("{}^{{tree}}", cs.tip))?;
    let tip_files = read_and_validate_coglet_strict_tree(&os.git, &tip_tree)
        .map_err(|e| format!("counter tip tree is not CogletStrict: {e}"))?;
    let tip_count = tip_files
        .get("data/count")
        .ok_or("counter tip has no data/count")
        .and_then(|oid| os.git.read_blob(oid, &env).map_err(|_| "read data/count"))?;
    r.check_eq(
        "seed: counter tip data/count",
        String::from_utf8_lossy(&tip_count).into_owned(),
        "6".to_string(),
    );
    let tip_wasm = tip_files
        .get("code/bin/step.wasm")
        .and_then(|oid| os.git.read_blob(oid, &env).ok());
    r.check(
        "seed: tip code/bin/step.wasm is the counter wasm",
        tip_wasm.as_deref() == Some(cs.step_wasm),
        format!("{} bytes", cs.step_wasm.len()),
    );
    r.note(format!(
        "coglet id `{cid}` seeded from tip {} (tree {tip_tree})",
        cs.tip
    ));
    let shared_os = Arc::new(os.clone());

    let staged_state = net_adapter_staging(&mut r, cs, &shared_os, &cid, &tip_files);
    // The `/net` projection registry is process-global per git dir: never leave it bound.
    os.clear_coglet_net_context();
    match staged_state {
        Some(state) => policy_patch(&mut r, os, &cid, &tip_files, cs.step_wasm, &state),
        None => {
            r.check(
                "patch: skipped",
                false,
                "no staged state commit to patch onto",
            );
        }
    }
    vault_lease(&mut r, &shared_os);
    Ok(r.finish())
}

// ----------------------------------------------------------------------------
// 1. `/net/github` mount, `@xattr`, `fd 3` `OP_STAGE_ACTION`, dispatch idempotency
// ----------------------------------------------------------------------------

/// Returns the post-step state commit (holding `out/actions/42`) on success.
fn net_adapter_staging(
    r: &mut ActReport,
    cs: &CounterState,
    shared_os: &Arc<GecaosOs>,
    cid: &str,
    tip_files: &BTreeMap<String, String>,
) -> Option<String> {
    let os: &GecaosOs = shared_os;
    let env = GitEnv::default();
    let adapter_def = AdapterDef {
        kind: AdapterKind::Mcp,
        endpoint: "https://mcp.example.invalid/github".into(),
        credentials: vec![SECRET_ID.into()],
        accounts: Vec::new(),
    };
    let mut tools = BTreeMap::new();
    tools.insert(
        "create_issue".to_string(),
        ToolDef {
            name: CAP_ISSUE.into(),
            schema_json: ISSUE_SCHEMA.into(),
            effect: ToolEffect::Write,
            approval: ToolApproval::None,
        },
    );
    tools.insert(
        "delete_repo".to_string(),
        ToolDef {
            name: CAP_DELETE.into(),
            schema_json: r#"{"type":"object","properties":{"repo":{"type":"string"}}}"#.into(),
            effect: ToolEffect::Write,
            approval: ToolApproval::Person,
        },
    );
    let catalog = ok(
        r,
        "net: publish adapter catalog",
        publish_adapter_catalog(os, ADAPTER, &adapter_def, &tools, None),
    )?;
    r.hash("adapter catalog (refs/adapters/github/catalog)", &catalog);
    let catalog_ref = adapter_catalog_ref(ADAPTER).ok()?;
    r.check_eq(
        "net: catalog ref is refs/adapters/github/catalog",
        catalog_ref.as_str(),
        "refs/adapters/github/catalog",
    );
    r.check_eq(
        "net: catalog ref points at published commit",
        rev_parse_opt(os, &catalog_ref),
        Some(catalog.clone()),
    );
    let fstab = os
        .read_virtual_file("/etc/fstab")
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .unwrap_or_default();
    r.check(
        "net: /etc/fstab mounts /net/github as adapter",
        fstab.contains("/dev/refs/adapters/github/catalog\t/net/github\tadapter"),
        fstab
            .lines()
            .find(|l| l.contains("/net/github"))
            .unwrap_or("<no /net/github line>")
            .to_string(),
    );

    // Grant only `create_issue`; `delete_repo` stays ungranted.
    let grants = ok(
        r,
        "net: publish coglet grants",
        publish_coglet_grants(os, cid, &[CAP_ISSUE.into()], None),
    )?;
    r.hash(
        format!("grants ({})", coglet_grants_ref(cid).unwrap_or_default()),
        &grants,
    );

    let step_wt = cs.work_dir.join("act4-step-worktree");
    if let Err(e) = std::fs::create_dir_all(&step_wt) {
        r.check("net: step worktree", false, e.to_string());
        return None;
    }
    let step_ctx = CogletStepIpcContext::new(cid)
        .with_pinned_catalog(ADAPTER, &catalog)
        .with_pinned_grants_commit(&grants)
        .with_staging_worktree(&step_wt);
    os.bind_coglet_net_context(step_ctx.clone());

    r.check_eq(
        "net: ls /net",
        os.list_virtual_dir("/net"),
        Ok(vec![ADAPTER.to_string()]),
    );
    r.check_eq(
        "net: ls /net/github hides ungranted delete_repo",
        os.list_virtual_dir("/net/github"),
        Ok(vec!["create_issue".to_string()]),
    );
    let schema = os
        .read_virtual_file("/net/github/create_issue/@xattr/schema")
        .map(|b| String::from_utf8_lossy(&b).into_owned());
    r.check(
        "net: /net/github/create_issue/@xattr/schema",
        schema.as_ref().is_ok_and(|s| {
            s.contains("\"title\"")
                && serde_json::from_str::<serde_json::Value>(s).ok()
                    == serde_json::from_str::<serde_json::Value>(ISSUE_SCHEMA).ok()
        }),
        format!("{schema:?}"),
    );
    r.check_eq(
        "net: /net/github/create_issue/@xattr/effect",
        os.read_virtual_file("/net/github/create_issue/@xattr/effect")
            .map(|b| String::from_utf8_lossy(&b).into_owned()),
        Ok("write".to_string()),
    );
    let denied = os.read_virtual_file("/net/github/delete_repo/@xattr/schema");
    r.check(
        "net: ungranted tool xattr is EACCES",
        denied.as_ref().is_err_and(|e| e.contains("EACCES")),
        format!("{denied:?}"),
    );

    // Stage `42` over the `fd 3` wire protocol: opcode `0x20 OP_STAGE_ACTION`.
    let server = IpcServer::new(os.git.clone(), 16)
        .with_os_session(Arc::clone(shared_os), "root", "/root")
        .with_coglet_step_context(step_ctx.clone());
    let (guard, mut client) = ok(r, "net: fd 3 socket pair", server.spawn_client_pair())?;
    r.note(
        "fd 3 calls use IpcServer::spawn_client_pair (in-process UnixStream pair, same 0x20/0x21 \
         framing a child gets on fd 3 via run_child_with_fd3); no child process is spawned",
    );
    let ack = client.stage_action(ADAPTER, "create_issue", ACTION_ID, ACTION_PAYLOAD);
    let refused = client.stage_action(ADAPTER, "delete_repo", "99", br#"{"repo":"prod"}"#);
    drop(client);
    let joined = guard.join();
    r.check(
        "net: fd 3 server thread exits cleanly",
        joined.is_ok(),
        format!("{joined:?}"),
    );
    let ack = ok(r, "net: fd 3 0x20 OP_STAGE_ACTION create_issue", ack)?;
    r.check_eq(
        "net: ack action/effect/approval",
        (ack.action_name.as_str(), ack.effect, ack.approval),
        (CAP_ISSUE, ToolEffect::Write, ToolApproval::None),
    );
    r.check_eq(
        "net: ack staged_path",
        ack.staged_path.as_str(),
        "out/actions/42",
    );
    r.check(
        "net: out/actions/42 materialized in step worktree",
        step_wt.join("out/actions/42").is_file(),
        step_wt.join("out/actions/42").display().to_string(),
    );
    let refused_msg = refused.as_ref().err().map(|e| e.to_string());
    r.check(
        "net: fd 3 stage of ungranted delete_repo refused",
        refused_msg
            .as_deref()
            .is_some_and(|m| m.contains("EACCES") && m.contains(CAP_DELETE))
            && !step_wt.join("out/actions/99").exists(),
        refused_msg.unwrap_or_else(|| "accepted".into()),
    );

    // Produced tree: the counter tip + manifest + the staged `out/actions/*`.
    let mut files = tip_files.clone();
    let manifest = manifest_json(cid, &[CAP_ISSUE]);
    files.insert("manifest.json".into(), blob(os, &manifest).ok()?);
    for (path, bytes) in step_ctx.staged_out_tree_files() {
        files.insert(path, blob(os, &bytes).ok()?);
    }
    let tree = ok(
        r,
        "net: write produced CogletStrict tree",
        write_coglet_strict_tree_from_blob_oids(&os.git, &files),
    )?;
    let action_oid = files.get("out/actions/42").cloned().unwrap_or_default();
    r.check_eq(
        "net: produced tree has exactly out/actions/42",
        files
            .keys()
            .filter(|k| k.starts_with("out/"))
            .cloned()
            .collect::<Vec<_>>(),
        vec!["out/actions/42".to_string()],
    );
    let staged = os
        .git
        .read_blob(&action_oid, &env)
        .map_err(|e| e.to_string())
        .and_then(|b| StagedAction::decode(&b));
    r.check(
        "net: out/actions/42 decodes to the staged call",
        staged.as_ref().is_ok_and(|a| {
            a.id == ACTION_ID
                && a.action_name == CAP_ISSUE
                && a.catalog_commit == catalog
                && a.payload == ACTION_PAYLOAD
        }),
        format!("blob {action_oid}"),
    );
    r.check_eq(
        "net: ack blob_oid is the tree's out/actions/42 blob",
        ack.blob_oid.as_str(),
        action_oid.as_str(),
    );
    let state = ok(
        r,
        "net: commit post-step state",
        commit(os, &tree, &[cs.tip], "act4 step: stage create_issue 42", 1),
    )?;
    let state_ref = coglet_state_ref(cid).ok()?;
    ok(
        r,
        "net: set coglet state ref",
        os.git
            .update_ref_cas(&state_ref, &state, None, "act4 step", &env),
    )?;
    r.hash("produced tree (out/actions/42)", &tree);
    r.hash(format!("state commit ({state_ref})"), &state);

    // Dispatch once, then prove the second dispatch is a no-op.
    let mut sent: Vec<String> = Vec::new();
    let rep1 = ok(
        r,
        "net: dispatch_staged_actions #1",
        dispatch_staged_actions(
            os,
            cid,
            &state,
            &DispatchApprovalSet::default(),
            &[],
            |a: &StagedAction| {
                sent.push(a.id.clone());
                Ok(format!(r#"{{"ok":true,"issue":{}}}"#, a.id).into_bytes())
            },
        ),
    )?;
    r.check_eq(
        "net: dispatch #1 sends 42 once",
        rep1.outcomes.clone(),
        vec![ActionDispatchItemOutcome::Dispatched {
            id: ACTION_ID.into(),
            action_name: CAP_ISSUE.into(),
            scrubbed_outcome: br#"{"ok":true,"issue":42}"#.to_vec(),
        }],
    );
    let ids = read_dispatched_action_ids(os, &state);
    r.check(
        "net: refs/notes/dispatched records 42",
        ids.as_ref()
            .is_ok_and(|s| s.len() == 1 && s.contains(ACTION_ID)),
        format!("{ids:?}"),
    );
    let notes_1 = rev_parse_opt(os, DISPATCHED_NOTES_REF);
    if let Some(n) = &notes_1 {
        r.hash("refs/notes/dispatched after dispatch #1", n);
    }
    if let Ok(Some(raw)) = os.git.notes_show_raw(DISPATCHED_NOTES_REF, &state, &env) {
        r.note(format!(
            "refs/notes/dispatched note on {state}: {}",
            String::from_utf8_lossy(&raw).trim()
        ));
    }
    let rep2 = ok(
        r,
        "net: dispatch_staged_actions #2",
        dispatch_staged_actions(
            os,
            cid,
            &state,
            &DispatchApprovalSet::default(),
            &[],
            |a: &StagedAction| {
                sent.push(a.id.clone());
                Ok(Vec::new())
            },
        ),
    )?;
    r.check_eq(
        "net: dispatch #2 is AlreadyDispatched",
        rep2.outcomes,
        vec![ActionDispatchItemOutcome::AlreadyDispatched {
            id: ACTION_ID.into(),
        }],
    );
    r.check_eq(
        "net: handler invoked exactly once across both dispatches",
        sent,
        vec![ACTION_ID.to_string()],
    );
    r.check(
        "net: refs/notes/dispatched unmoved by dispatch #2 (no duplicate)",
        notes_1.is_some() && rev_parse_opt(os, DISPATCHED_NOTES_REF) == notes_1,
        format!("{notes_1:?}"),
    );
    Some(state)
}

// ----------------------------------------------------------------------------
// 2. `policy` patch: ungranted capability -> `PAUSED_CONFLICT`, then migrate & merge
// ----------------------------------------------------------------------------

fn policy_patch(
    r: &mut ActReport,
    os: &GecaosOs,
    cid: &str,
    tip_files: &BTreeMap<String, String>,
    step_wasm: &[u8],
    state_1: &str,
) {
    let _ = policy_patch_inner(r, os, cid, tip_files, step_wasm, state_1);
}

fn policy_patch_inner(
    r: &mut ActReport,
    os: &GecaosOs,
    cid: &str,
    tip_files: &BTreeMap<String, String>,
    step_wasm: &[u8],
    state_1: &str,
) -> Option<()> {
    let env = GitEnv::default();
    let state_ref = coglet_state_ref(cid).ok()?;
    let policy_ref = coglet_policy_ref(cid).ok()?;
    let code: BTreeMap<String, String> = tip_files
        .iter()
        .filter(|(k, _)| k.starts_with("code/"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let policy_tree = |r: &mut ActReport, name: &str, needs: &[&str], migrate: bool| {
        let mut f = code.clone();
        f.insert(
            "manifest.json".into(),
            blob(os, &manifest_json(cid, needs)).ok()?,
        );
        if migrate {
            f.insert("code/migrations/migrate".into(), blob(os, MIGRATE).ok()?);
        }
        ok(
            r,
            format!("patch: write {name} tree"),
            write_coglet_strict_tree_from_blob_oids(&os.git, &f),
        )
    };

    // policy_0 = the counter's code + a manifest that needs only granted capabilities.
    let p0_tree = policy_tree(r, "policy_0", &[CAP_ISSUE], false)?;
    let p0 = ok(
        r,
        "patch: commit policy_0",
        commit(os, &p0_tree, &[], "policy 0", 2),
    )?;
    ok(
        r,
        "patch: set policy ref",
        os.git
            .update_ref_cas(&policy_ref, &p0, None, "act4 policy 0", &env),
    )?;
    r.hash(format!("policy_0 ({policy_ref})"), &p0);

    // policy_1 asks for `close_issue`, which the coglet was never granted.
    let p1_tree = policy_tree(r, "policy_1", &[CAP_ISSUE, CAP_CLOSE], false)?;
    let p1 = ok(
        r,
        "patch: commit policy_1",
        commit(os, &p1_tree, &[&p0], "policy 1: needs close_issue", 3),
    )?;
    r.hash("policy_1 (ungranted manifest)", &p1);
    let mut req = CogletPatchRequest::new(&p0, &p1);
    req.logical_time = LOGICAL_T0;
    let out = ok(
        r,
        "patch: apply_coglet_patch(policy_1)",
        apply_coglet_patch(os, cid, &req),
    )?;
    match &out {
        PatchMergeOutcome::PausedConflict {
            conflict_tree_oid,
            paused_commit_oid,
            descriptor,
            kind,
            reason,
        } => {
            r.check_eq(
                "patch: conflict kind is UngrantedManifestCapabilities",
                kind.clone(),
                PatchConflictKind::UngrantedManifestCapabilities {
                    missing: vec![CAP_CLOSE.into()],
                },
            );
            r.check_eq(
                "patch: descriptor is 0x02 conflict tree",
                decode_oid_descriptor(descriptor),
                Ok((DESC_TAG_CONFLICT, conflict_tree_oid.clone())),
            );
            let names: Vec<String> = os
                .git
                .ls_tree(conflict_tree_oid, false, &env)
                .map(|v| v.into_iter().map(|e| e.path).collect())
                .unwrap_or_default();
            r.check_eq(
                "patch: conflict tree = base/conflict.info/ours/theirs",
                names,
                ["base", "conflict.info", "ours", "theirs"]
                    .map(String::from)
                    .to_vec(),
            );
            r.hash("0x02 conflict tree", conflict_tree_oid);
            r.hash("paused commit", paused_commit_oid);
            r.note(format!("PAUSED_CONFLICT reason: {reason}"));
        }
        other => {
            r.check(
                "patch: policy_1 pauses with a conflict",
                false,
                format!("{other:?}"),
            );
        }
    }
    let paused = get_coglet_paused_state(os, cid);
    r.check(
        "patch: get_coglet_paused_state = PAUSED_CONFLICT",
        paused.as_ref().is_ok_and(|p| {
            p.as_ref().is_some_and(|p| {
                p.status == "PAUSED_CONFLICT"
                    && p.conflict_info.contains("kind=ungranted_manifest")
                    && p.conflict_info
                        .contains(&format!("missing_grants={CAP_CLOSE}"))
            })
        }),
        format!("{paused:?}"),
    );
    r.check_eq(
        "patch: state ref unmoved by conflict",
        rev_parse_opt(os, &state_ref),
        Some(state_1.to_string()),
    );
    r.check_eq(
        "patch: policy ref unmoved by conflict",
        rev_parse_opt(os, &policy_ref),
        Some(p0.clone()),
    );

    // Resolve: grant `close_issue`, ship the migration, clear the pause and re-apply.
    let grants_2 = ok(
        r,
        "patch: grant close_issue",
        publish_coglet_grants(os, cid, &[CAP_ISSUE.into(), CAP_CLOSE.into()], None),
    )?;
    r.hash("grants after resolve", &grants_2);
    let p2_tree = policy_tree(r, "policy_2", &[CAP_ISSUE, CAP_CLOSE], true)?;
    let p2 = ok(
        r,
        "patch: commit policy_2",
        commit(os, &p2_tree, &[&p1], "policy 2: migrate data/count", 4),
    )?;
    r.hash("policy_2 (granted + migration)", &p2);
    ok(
        r,
        "patch: resolve_paused_coglet",
        resolve_paused_coglet(os, cid, state_1),
    )?;
    r.check(
        "patch: pause cleared",
        matches!(get_coglet_paused_state(os, cid), Ok(None)),
        "refs/coglets/<id>/paused deleted",
    );
    let mut req = CogletPatchRequest::new(&p0, &p2);
    req.logical_time = LOGICAL_T0 + 1;
    let out = ok(
        r,
        "patch: apply_coglet_patch(policy_2)",
        apply_coglet_patch(os, cid, &req),
    )?;
    let PatchMergeOutcome::Merged {
        state_commit_oid,
        tree_oid,
        migrated,
        descriptor,
    } = out
    else {
        r.check("patch: policy_2 merges cleanly", false, format!("{out:?}"));
        return None;
    };
    r.check("patch: policy_2 merges cleanly", true, &state_commit_oid);
    r.check("patch: migration ran", migrated, format!("{migrated}"));
    r.check_eq(
        "patch: descriptor is 0x00 durable tree",
        decode_oid_descriptor(&descriptor),
        Ok((DESC_TAG_DURABLE, tree_oid.clone())),
    );
    r.check_eq(
        "patch: state ref advanced to merge",
        rev_parse_opt(os, &state_ref),
        Some(state_commit_oid.clone()),
    );
    r.check_eq(
        "patch: merge parents = (state, policy_2)",
        (
            rev_parse_opt(os, &format!("{state_commit_oid}^1")),
            rev_parse_opt(os, &format!("{state_commit_oid}^2")),
        ),
        (Some(state_1.to_string()), Some(p2.clone())),
    );
    r.check_eq(
        "patch: policy ref advanced to policy_2",
        rev_parse_opt(os, &policy_ref),
        Some(p2.clone()),
    );
    let merged = match read_and_validate_coglet_strict_tree(&os.git, &tree_oid) {
        Ok(m) => m,
        Err(e) => {
            r.check("patch: merged tree is CogletStrict", false, e);
            return None;
        }
    };
    let read_bytes = |path: &str| -> Option<Vec<u8>> {
        merged
            .get(path)
            .and_then(|oid| os.git.read_blob(oid, &env).ok())
    };
    let read = |path: &str| read_bytes(path).map(|b| String::from_utf8_lossy(&b).into_owned());
    r.check_eq(
        "patch: migrated data/count = 6 x 10",
        read("data/count"),
        Some("60".to_string()),
    );
    r.check_eq(
        "patch: migration added data/schema",
        read("data/schema"),
        Some("2".to_string()),
    );
    r.check_eq(
        "patch: data/inbox.jsonl carried through migration",
        read("data/inbox.jsonl"),
        Some(String::new()),
    );
    r.check_eq(
        "patch: logs/events.jsonl kept (merge=ours)",
        merged.get("logs/events.jsonl"),
        tip_files.get("logs/events.jsonl"),
    );
    r.check(
        "patch: code/bin/step.wasm is the counter wasm (merge=theirs)",
        merged.get("code/bin/step.wasm") == tip_files.get("code/bin/step.wasm")
            && read_bytes("code/bin/step.wasm").as_deref() == Some(step_wasm),
        merged
            .get("code/bin/step.wasm")
            .cloned()
            .unwrap_or_default(),
    );
    r.check(
        "patch: out/ cleared at patch boundary",
        !merged.keys().any(|k| k.starts_with("out/")),
        merged.keys().cloned().collect::<Vec<_>>().join(","),
    );
    r.check_eq(
        "patch: merged manifest is policy_2's",
        read("manifest.json"),
        Some(String::from_utf8_lossy(&manifest_json(cid, &[CAP_ISSUE, CAP_CLOSE])).into_owned()),
    );
    r.hash("merged state commit", &state_commit_oid);
    r.hash("merged tree", &tree_oid);
    Some(())
}

// ----------------------------------------------------------------------------
// 3. Vault: `refs/vault/policy`, `0x21 OP_LEASE_SECRET`, audit, scrubbing, zero leaks
// ----------------------------------------------------------------------------

fn vault_lease(r: &mut ActReport, shared_os: &Arc<GecaosOs>) {
    let _ = vault_lease_inner(r, shared_os);
}

fn vault_lease_inner(r: &mut ActReport, shared_os: &Arc<GecaosOs>) -> Option<()> {
    let os: &GecaosOs = shared_os;
    let secret_str = std::str::from_utf8(SECRET).ok()?;
    let backend = Arc::new(MemoryVaultBackend::new());
    ok(
        r,
        "vault: store secret in MemoryVaultBackend",
        backend.put_secret(SECRET_STORE, SECRET),
    )?;

    let leaky = format!(
        r#"{{"id":"{SECRET_ID}","owner":"act4","kind":"api_key","store":"{SECRET_STORE}","value":"{secret_str}","may_lease":["github"],"state":"active"}}"#
    );
    r.check(
        "vault: policy entry carrying a raw value is refused",
        commit_vault_policy_raw(
            os,
            "vault/github-token.json",
            leaky.as_bytes(),
            Some(backend.as_ref()),
            None,
        )
        .is_err(),
        "commit_vault_policy_raw(.., \"value\": ..) -> Err",
    );
    let record = SecretRecord {
        id: SECRET_ID.into(),
        owner: "act4".into(),
        kind: SecretKind::ApiKey,
        store: SECRET_STORE.into(),
        may_lease: vec![ADAPTER.into()],
        lease_ttl_secs: 300,
        state: SecretState::Active,
    };
    let policy = ok(
        r,
        "vault: publish SecretRecord to refs/vault/policy",
        publish_secret_record(os, &record, Some(backend.as_ref()), None),
    )?;
    r.check_eq(
        "vault: refs/vault/policy points at the record commit",
        rev_parse_opt(os, VAULT_POLICY_REF),
        Some(policy.clone()),
    );
    r.hash("refs/vault/policy", &policy);
    let vault = VaultLeaseService::new(backend.clone());

    // A coglet step's `fd 3` is refused outright.
    let step_srv = IpcServer::new(os.git.clone(), 16)
        .with_os_session(Arc::clone(shared_os), "root", "/root")
        .with_coglet_step_context(CogletStepIpcContext::new("act4-step"));
    let (g, mut c) = ok(r, "vault: coglet fd 3 pair", step_srv.spawn_client_pair())?;
    let step_lease = c.lease_secret(SECRET_ID);
    drop(c);
    let _ = g.join();
    let msg = step_lease.as_ref().err().map(|e| e.to_string());
    r.check(
        "vault: coglet step fd 3 0x21 OP_LEASE_SECRET refused",
        msg.as_deref()
            .is_some_and(|m| m.contains("coglet sandboxes")),
        msg.unwrap_or_else(|| "LEASED".into()),
    );
    r.check(
        "vault: VaultLeaseService refuses a caller with no adapter id",
        matches!(
            vault.lease_secret(os, None, SECRET_ID, Some(7_000)),
            Err(VaultLeaseError::CogletSandboxForbidden)
        ),
        "lease_secret(os, None, ..) -> CogletSandboxForbidden",
    );

    // An adapter outside `may_lease` is refused too.
    let slack_srv = IpcServer::new(os.git.clone(), 16)
        .with_os_session(Arc::clone(shared_os), "root", "/root")
        .with_adapter_session(AdapterIpcSession::new("slack", vault.clone()));
    let (g, mut c) = ok(r, "vault: slack fd 3 pair", slack_srv.spawn_client_pair())?;
    let slack_lease = c.lease_secret_at(SECRET_ID, Some(7_000));
    drop(c);
    let _ = g.join();
    let msg = slack_lease.as_ref().err().map(|e| e.to_string());
    r.check(
        "vault: unauthorized adapter `slack` refused (may_lease)",
        msg.as_deref().is_some_and(|m| m.contains("may_lease")),
        msg.unwrap_or_else(|| "LEASED".into()),
    );

    // The `github` adapter session leases it over `fd 3`.
    let gh_srv = IpcServer::new(os.git.clone(), 16)
        .with_os_session(Arc::clone(shared_os), "root", "/root")
        .with_adapter_session(AdapterIpcSession::new(ADAPTER, vault.clone()));
    let (g, mut c) = ok(r, "vault: github fd 3 pair", gh_srv.spawn_client_pair())?;
    let lease = c.lease_secret_at(SECRET_ID, Some(7_000));
    drop(c);
    let _ = g.join();
    let lease = ok(r, "vault: github adapter fd 3 0x21 OP_LEASE_SECRET", lease)?;
    r.check(
        "vault: lease carries the secret, policy commit and TTL",
        lease.value == SECRET
            && lease.secret_id == SECRET_ID
            && lease.adapter_id == ADAPTER
            && lease.policy_commit == policy
            && lease.issued_at == 7_000
            && lease.expires_at == 7_300,
        format!("{lease:?}"),
    );

    let audit = ok(r, "vault: read_vault_audit_log", read_vault_audit_log(os))?;
    let mine: Vec<_> = audit.iter().filter(|a| a.secret_id == SECRET_ID).collect();
    r.check(
        "vault: refs/vault/audit records exactly the one lease",
        mine.len() == 1
            && mine[0].adapter_id == ADAPTER
            && mine[0].policy_commit == policy
            && mine[0].state == SecretState::Active
            && mine[0].issued_at == 7_000
            && mine[0].expires_at == 7_300,
        format!("{mine:?}"),
    );
    if let Some(a) = rev_parse_opt(os, VAULT_AUDIT_REF) {
        r.hash("refs/vault/audit", &a);
    }
    match scan_objects(os, &[VAULT_AUDIT_REF], SECRET) {
        Ok(s) => {
            r.check(
                "vault: refs/vault/audit history is value-free",
                s.objects > 0 && s.hits.is_empty(),
                format!("{} objects, hits {:?}", s.objects, s.hits),
            );
        }
        Err(e) => {
            r.check("vault: refs/vault/audit history is value-free", false, e);
        }
    }

    let outcome =
        format!(r#"{{"status":401,"error":"Authorization: Bearer {secret_str} rejected"}}"#);
    let scrubbed = scrub_leased_secrets(outcome.as_bytes(), std::slice::from_ref(&lease));
    r.check(
        "vault: scrub_leased_secrets removes the secret from outcome bytes",
        find(&scrubbed, SECRET).is_none() && find(&scrubbed, b"[REDACTED]").is_some(),
        String::from_utf8_lossy(&scrubbed).into_owned(),
    );

    // Zero copies of the secret anywhere in git.
    // Positive control: the same scanner does find a needle that IS in git.
    match scan_objects(os, &["--all"], CAP_ISSUE.as_bytes()) {
        Ok(s) => {
            r.check(
                "vault: scanner control finds a known needle (`mcp.github.create_issue`)",
                !s.hits.is_empty(),
                format!("{} of {} objects contain it", s.hits.len(), s.objects),
            );
        }
        Err(e) => {
            r.check("vault: scanner control finds a known needle", false, e);
        }
    }
    match scan_objects(os, &["--all"], SECRET) {
        Ok(s) => {
            r.check(
                "vault: rev-list --objects --all + cat-file --batch: 0 secret occurrences",
                s.objects > 0 && s.missing == 0 && s.hits.is_empty(),
                format!(
                    "{} objects scanned, {} missing, hits {:?}",
                    s.objects, s.missing, s.hits
                ),
            );
        }
        Err(e) => {
            r.check(
                "vault: rev-list --objects --all + cat-file --batch: 0 secret occurrences",
                false,
                e,
            );
        }
    }
    match scan_all_objects(os) {
        Ok(s) => {
            r.check(
                "vault: cat-file --batch-all-objects (incl. unreachable): 0 secret occurrences",
                s.objects > 0 && s.hits.is_empty(),
                format!("{} objects scanned, hits {:?}", s.objects, s.hits),
            );
        }
        Err(e) => {
            r.check(
                "vault: cat-file --batch-all-objects (incl. unreachable): 0 secret occurrences",
                false,
                e,
            );
        }
    }
    match git(os, &["fsck", "--full"], None) {
        Ok(out) => {
            let text = format!(
                "{}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let dangling = text.lines().filter(|l| l.starts_with("dangling")).count();
            r.check(
                "vault: git fsck --full passes",
                out.status.success() && !text.contains("error") && !text.contains("missing"),
                format!("exit {:?}, {dangling} dangling", out.status.code()),
            );
        }
        Err(e) => {
            r.check("vault: git fsck --full passes", false, e);
        }
    }
    Some(())
}

// ----------------------------------------------------------------------------
// Helpers
// ----------------------------------------------------------------------------

/// Record a fallible API call as a check; `None` on failure so callers can bail with `?`.
fn ok<T, E: Display>(r: &mut ActReport, name: impl Into<String>, res: Result<T, E>) -> Option<T> {
    match res {
        Ok(v) => {
            r.check(name, true, "ok");
            Some(v)
        }
        Err(e) => {
            r.check(name, false, e.to_string());
            None
        }
    }
}

fn manifest_json(cid: &str, needs: &[&str]) -> Vec<u8> {
    serde_json::json!({ "id": cid, "version": "1.0", "needs": needs })
        .to_string()
        .into_bytes()
}

fn blob(os: &GecaosOs, bytes: &[u8]) -> Result<String, String> {
    os.git.write_blob(bytes).map_err(|e| e.to_string())
}

fn rev_parse(os: &GecaosOs, rev: &str) -> Result<String, String> {
    os.git
        .rev_parse_opt(rev, &GitEnv::default())
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("`{rev}` does not resolve"))
}

fn rev_parse_opt(os: &GecaosOs, rev: &str) -> Option<String> {
    os.git.rev_parse_opt(rev, &GitEnv::default()).ok().flatten()
}

/// Deterministic unsigned commit written in-process (no wall clock, no git identity).
fn commit(
    os: &GecaosOs,
    tree: &str,
    parents: &[&str],
    msg: &str,
    tick: u64,
) -> Result<String, String> {
    let tree = Oid256::from_hex(tree).map_err(|e| e.to_string())?;
    let parents = parents
        .iter()
        .map(|p| Oid256::from_hex(p).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    let ident = IdentSpec::new("act4", "act4@unforged", LOGICAL_T0 - 100 + tick);
    let spec = CommitSpec::unsigned(tree, parents, ident.clone(), ident, msg.to_string());
    LooseObjectStore::new(&os.git.git_dir)
        .write_commit(&spec)
        .map(|oid| oid.to_hex())
        .map_err(|e| e.to_string())
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// `git --git-dir <gitdir> <args>` with the ambient git environment scrubbed.
fn git(
    os: &GecaosOs,
    args: &[&str],
    stdin: Option<Vec<u8>>,
) -> Result<std::process::Output, String> {
    let mut cmd = Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
    ] {
        cmd.env_remove(var);
    }
    cmd.arg("--git-dir")
        .arg(&os.git.git_dir)
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("git {args:?}: {e}"))?;
    let writer = stdin.and_then(|bytes| {
        child.stdin.take().map(|mut pipe| {
            std::thread::spawn(move || {
                let _ = pipe.write_all(&bytes);
            })
        })
    });
    let out = child
        .wait_with_output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if let Some(w) = writer {
        let _ = w.join();
    }
    Ok(out)
}

struct Scan {
    objects: usize,
    missing: usize,
    /// `oid type` of every object whose content contains the needle.
    hits: Vec<String>,
}

/// `rev-list --objects <revs>` piped through `cat-file --batch`, searching every object.
fn scan_objects(os: &GecaosOs, revs: &[&str], needle: &[u8]) -> Result<Scan, String> {
    let mut args = vec!["rev-list", "--objects"];
    args.extend_from_slice(revs);
    let listed = git(os, &args, None)?;
    if !listed.status.success() {
        return Err(format!(
            "rev-list failed: {}",
            String::from_utf8_lossy(&listed.stderr)
        ));
    }
    let mut oids = Vec::new();
    for line in String::from_utf8_lossy(&listed.stdout).lines() {
        if let Some(oid) = line.split_whitespace().next() {
            oids.extend_from_slice(oid.as_bytes());
            oids.push(b'\n');
        }
    }
    let batch = git(os, &["cat-file", "--batch"], Some(oids))?;
    if !batch.status.success() {
        return Err(format!(
            "cat-file --batch failed: {}",
            String::from_utf8_lossy(&batch.stderr)
        ));
    }
    parse_batch(&batch.stdout, needle)
}

fn scan_all_objects(os: &GecaosOs) -> Result<Scan, String> {
    let batch = git(
        os,
        &["cat-file", "--batch-all-objects", "--unordered", "--batch"],
        None,
    )?;
    if !batch.status.success() {
        return Err(format!(
            "cat-file --batch-all-objects failed: {}",
            String::from_utf8_lossy(&batch.stderr)
        ));
    }
    parse_batch(&batch.stdout, SECRET)
}

/// Parse `cat-file --batch` output (`<oid> <type> <size>\n<content>\n` or `<oid> missing\n`).
fn parse_batch(mut out: &[u8], needle: &[u8]) -> Result<Scan, String> {
    let mut scan = Scan {
        objects: 0,
        missing: 0,
        hits: Vec::new(),
    };
    while !out.is_empty() {
        let nl = find(out, b"\n").ok_or("truncated cat-file header")?;
        let header = String::from_utf8_lossy(&out[..nl]).into_owned();
        out = &out[nl + 1..];
        let parts: Vec<&str> = header.split(' ').collect();
        if parts.len() == 2 && parts[1] == "missing" {
            scan.missing += 1;
            continue;
        }
        let [oid, kind, size] = parts[..] else {
            return Err(format!("bad cat-file header `{header}`"));
        };
        let size: usize = size
            .parse()
            .map_err(|_| format!("bad size in `{header}`"))?;
        if out.len() < size + 1 {
            return Err(format!("truncated object {oid}"));
        }
        if find(&out[..size], needle).is_some() {
            scan.hits.push(format!("{oid} {kind}"));
        }
        scan.objects += 1;
        out = &out[size + 1..];
    }
    Ok(scan)
}
