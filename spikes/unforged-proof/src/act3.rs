//! Act 3: Stage 3 router. Signed hub-ref statements, a private `data/`-omitted
//! `.promisor` push, the read-grant gate, and compaction, all on a real
//! unforged hub.
//!
//! `cs.os` plays the device and is only read. The hub is a fresh router
//! `GecaosOs` under `cs.work_dir`, so Act 3 never moves a ref, writes config or
//! prunes an object in the shared store that later acts read.
//!
//! The order is the one the real API allows. `verify_and_apply_ref_statement`
//! requires the target commit to be present on the router already, and
//! `push_coglet_state_without_data` always CAS-moves the ref it is given. So the
//! device first pushes the chain (data omitted) to a node-scoped upload ref, and
//! the signed statements are what move `.../state`. Compaction runs last.
use crate::report::{ActReport, CounterState};
use coglet_types_hub0::Value as HubValue;
use gecaos::auth::{NamespaceTier, allowed_signers_path, generate_signing_identity_with_tier};
use gecaos::coglet_router::{
    CborValue, CogletGrantRecord, RefStatement, STATEMENTS_NOTES_REF, SignedRefStatement,
    compact_coglet_history, encode_deterministic_cbor, has_valid_data_read_grant,
    iroh_pubkey_to_openssh, iroh_pubkey_to_openssh_str, issue_signed_coglet_grant,
    push_coglet_state_without_data, read_coglet_state_path_gated, read_ref_statements_for_commit,
    register_iroh_allowed_signer, sign_ref_statement, verify_and_apply_ref_statement,
    verify_ref_statement_signature, write_ref_permissions,
};
use gecaos::git::GitEnv;
use gecaos::os::GecaosOs;
use russh::keys::PublicKey;
use russh::keys::ssh_key::{LineEnding, PrivateKey, private::Ed25519Keypair};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::process::Command;

/// Fixed seeds, so node keys and statement signatures are reproducible (notes,
/// grant and checkpoint commits carry wall-clock times and are not).
const NODE_SEED: [u8; 32] = [0x31; 32];
const HUB_SEED: [u8; 32] = [0x48; 32];
const ROGUE_SEED: [u8; 32] = [0x99; 32];
/// The node key may move refs only under its own runtime namespace.
const NODE_REFS: &str = "refs/namespaces/node-1/**";
const KEEP_RECENT: usize = 2;
/// coglet's own golden `hub-ref-statement` signatures (same tree as this spike).
const HUB0_FIXTURES: &str =
    include_str!("../../../crates/coglet-types-hub0/fixtures/signatures.json");

pub fn run(cs: &CounterState) -> Result<ActReport, String> {
    let mut r = ActReport::new(
        "3",
        "Stage 3: router hub-ref-statement, private data/-omitted .promisor push, read-grant gate & compaction",
    );
    let env = GitEnv::default();
    let id = cs.coglet_id;
    if cs.step_commits.len() < KEEP_RECENT + 2 {
        return Err(format!(
            "act3 needs at least {} step commits, got {}",
            KEEP_RECENT + 2,
            cs.step_commits.len()
        ));
    }
    let n = cs.step_commits.len();
    let tip = cs.tip;
    let prev = cs.step_commits[n - 2].as_str();
    let state_ref = format!("refs/namespaces/node-1/refs/coglets/{id}/state");
    let upload_ref = format!("refs/namespaces/node-1/refs/coglets/{id}/upload");
    r.hash("device tip StepCommit", tip);

    // Device-side facts every later check compares against (read-only on cs.os).
    let data_count_oid = cs
        .os
        .git
        .rev_parse_opt(&format!("{tip}:data/count"), &env)
        .map_err(|e| e.to_string())?
        .ok_or("tip has no data/count")?;
    let device_count = cs
        .os
        .git
        .read_blob(&data_count_oid, &env)
        .map_err(|e| e.to_string())?;
    let device_events = cs
        .os
        .git
        .read_blob(&format!("{tip}:logs/events.jsonl"), &env)
        .map_err(|e| e.to_string())?;
    let device_tip_tree = rev_tree(cs.os, tip)?;
    r.hash("data/count blob (private)", data_count_oid.as_str());

    // ---- Hub setup: router repo, node key, grant issuer, reader ----
    let work = cs.work_dir.join("act3");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let router = GecaosOs::init(work.join("router"))?;
    let gitdir = router.git.git_dir.clone();

    let node_sk = iroh::SecretKey::from_bytes(&NODE_SEED);
    let node_pk = node_sk.public();
    let hub_pk = iroh::SecretKey::from_bytes(&HUB_SEED).public();
    let rogue_sk = iroh::SecretKey::from_bytes(&ROGUE_SEED);
    let node_ssh = iroh_pubkey_to_openssh(&node_pk)?;
    r.note(format!("node-1 iroh key {node_pk}; hub {hub_pk}"));

    let entry = register_iroh_allowed_signer(
        &router,
        "node-1",
        &["runtime"],
        NamespaceTier::GitOnly,
        &node_pk,
        &[NODE_REFS],
        &[],
    )?;
    let grant_admin = generate_signing_identity_with_tier(
        &router,
        "grant-admin",
        &["wheel"],
        NamespaceTier::GitShell,
    )?;
    let auditor = generate_signing_identity_with_tier(
        &router,
        "auditor",
        &["inspectors"],
        NamespaceTier::GitOnly,
    )?;
    write_ref_permissions(
        &router,
        &format!("mode router\nnode-1      allow {NODE_REFS}\ngrant-admin allow refs/grants/*\n"),
    )?;
    let auditor_pub = std::fs::read_to_string(&auditor.public_key).map_err(|e| e.to_string())?;
    let auditor_pk = PublicKey::from_openssh(auditor_pub.trim()).map_err(|e| e.to_string())?;

    // ---- 1a. hub-ref-statement encoding: unforged vs coglet-types-hub0 ----
    let fixtures: serde_json::Value =
        serde_json::from_str(HUB0_FIXTURES).map_err(|e| format!("hub0 fixtures: {e}"))?;
    let mut vectors = 0;
    for fx in fixtures.as_array().ok_or("hub0 fixtures not an array")? {
        let st = &fx["statement"];
        if st.get("ref").is_none() {
            continue; // delegation statements are a different rule
        }
        vectors += 1;
        let purpose = st["purpose"].as_str().unwrap_or("?").to_string();
        let stmt = statement_from_json(st)?;
        let ours = encode_deterministic_cbor(&stmt.to_cbor_value())?;
        let theirs = coglet_types_hub0::encode("hub-ref-statement", &json_to_hub(st)?);
        r.check(
            format!("1. hub0 `{purpose}` golden: unforged CBOR == coglet_types_hub0::encode(\"hub-ref-statement\")"),
            theirs.as_ref() == Ok(&ours),
            match &theirs {
                Ok(b) if *b == ours => format!("{} bytes byte-exact", ours.len()),
                Ok(b) => format!("unforged={} hub0={}", hex::encode(&ours), hex::encode(b)),
                Err(e) => format!("hub0 refused: {e}"),
            },
        );
        let want_pre = hex::decode(fx["preimage_hex"].as_str().unwrap_or("")).unwrap_or_default();
        let got_pre = stmt.signing_preimage();
        r.check(
            format!("1. hub0 `{purpose}` golden: signing preimage `coglet/hub/0:<purpose>\\n<cbor>` matches"),
            got_pre.as_ref() == Ok(&want_pre),
            format!("{} bytes", want_pre.len()),
        );
        let golden = SignedRefStatement {
            statement: stmt,
            signer: fx["signer"].as_str().unwrap_or("").to_string(),
            signature: fx["signature"].as_str().unwrap_or("").to_string(),
        };
        let v = verify_ref_statement_signature(&golden, None);
        r.check(
            format!("1. hub0 `{purpose}` golden: coglet's Ed25519 signature verifies in unforged"),
            v.is_ok(),
            v.map(|k| format!("signer {k}")).unwrap_or_else(|e| e),
        );
    }
    r.check(
        "1. coglet-types-hub0 fixtures carry hub-ref-statement vectors",
        vectors >= 1,
        format!("{vectors} vectors"),
    );

    // The live statements: node-1 moves its own coglet's state ref, two generations.
    let live =
        |generation, old: Option<&str>, new: &str, refname: &str, purpose: &str| RefStatement {
            purpose: purpose.to_string(),
            hub: Some(hub_pk.to_string()),
            coglet: Some(format!("coglet:{node_pk}")),
            generation,
            refname: refname.to_string(),
            old_oid: old.map(str::to_string),
            new_oid: new.to_string(),
            archive_sha256: None,
        };
    let s1 = sign_ref_statement(&node_sk, live(1, None, prev, &state_ref, "state-ref"))?;
    let s2 = sign_ref_statement(&node_sk, live(2, Some(prev), tip, &state_ref, "state-ref"))?;
    let live_val = s2.statement.to_cbor_value();
    let ours = encode_deterministic_cbor(&live_val)?;
    let theirs = coglet_types_hub0::encode_profile(&cbor_to_hub(&live_val)?);
    r.check(
        "1. live SHA-256 state-ref statement: unforged CBOR == coglet_types_hub0::encode_profile bytes",
        theirs.as_ref() == Ok(&ours),
        format!("{} bytes", ours.len()),
    );
    match coglet_types_hub0::validate("hub-ref-statement", &cbor_to_hub(&live_val)?) {
        Err(e) => r.note(format!(
            "gap: hub0 `hub-ref-statement` schema rejects the live statement ({e}). Its `ref` is the closed set {{refs/heads/hub, refs/published/main, refs/heads/logs}} and `new` is 40-hex SHA-1, so a SHA-256 `refs/namespaces/<node>/refs/coglets/<id>/state` move is unforged's `state-ref` extension under the same `coglet/hub/0:` preimage framing and CBOR profile"
        )),
        Ok(()) => r.note("hub0 `hub-ref-statement` schema accepts the live state-ref statement"),
    }
    let pinned = verify_ref_statement_signature(&s1, Some(&node_pk));
    r.check(
        "1. verify_ref_statement_signature pinned to the node key",
        pinned.is_ok(),
        pinned.map(|k| k.to_string()).unwrap_or_else(|e| e),
    );
    r.check(
        "1. same signature pinned to a different key is refused",
        verify_ref_statement_signature(&s1, Some(&hub_pk)).is_err(),
        "",
    );

    // allowed_signers: the node key with its `refs=` comment is plain OpenSSH.
    let as_path = allowed_signers_path(&router);
    let as_txt = std::fs::read_to_string(&as_path).map_err(|e| e.to_string())?;
    let node_ssh_str = iroh_pubkey_to_openssh_str(&node_pk)?;
    let key_body = node_ssh_str.split_whitespace().nth(1).unwrap_or("");
    let node_line = as_txt
        .lines()
        .find(|l| l.contains(key_body))
        .unwrap_or("")
        .to_string();
    r.check(
        "1. info/allowed_signers has node-1's iroh key with refs=refs/namespaces/node-1/**",
        node_line
            .split_whitespace()
            .next()
            .is_some_and(|p| p.split(',').any(|x| x == "node-1"))
            && node_line.contains(&format!("refs={NODE_REFS}")),
        node_line.clone(),
    );
    r.check_eq(
        "1. upsert entry ref patterns",
        entry.ref_permission_patterns(),
        Ok((vec![NODE_REFS.to_string()], Vec::new())),
    );
    let (ssh_ok, ssh_detail) = ssh_keygen_roundtrip(&work, &as_path, key_body)?;
    r.check(
        "1. ssh-keygen -Y sign (node seed) / -Y verify / -Y find-principals accept allowed_signers",
        ssh_ok,
        ssh_detail,
    );

    // ---- 2. Private data/-omitted push into the .promisor store ----
    let push = push_coglet_state_without_data(
        &router,
        &cs.os.git,
        "node-1",
        Some(&node_ssh),
        Some(NamespaceTier::GitOnly),
        None,
        &upload_ref,
        None,
        tip,
    );
    let push = match push {
        Ok(p) => {
            r.check(
                "2. push_coglet_state_without_data (device -> router, data/ omitted)",
                true,
                format!(
                    "{} objects uploaded, {} data blobs omitted, ref {}",
                    p.uploaded_object_count,
                    p.omitted_data_blob_oids.len(),
                    p.full_ref
                ),
            );
            p
        }
        Err(e) => {
            r.check(
                "2. push_coglet_state_without_data (device -> router, data/ omitted)",
                false,
                e,
            );
            return Ok(r.finish());
        }
    };
    r.hash("promisor pack", push.pack_hash.as_str());
    let pack_dir = gitdir.join("objects").join("pack");
    let pack_files: Vec<String> = ["pack", "idx", "promisor"]
        .iter()
        .filter(|ext| {
            pack_dir
                .join(format!("pack-{}.{ext}", push.pack_hash))
                .is_file()
        })
        .map(|ext| ext.to_string())
        .collect();
    r.check_eq(
        "2. pack-<sha256>.{pack,idx,promisor} written",
        pack_files,
        vec![
            "pack".to_string(),
            "idx".to_string(),
            "promisor".to_string(),
        ],
    );
    r.check_eq(
        "2. device data/count bytes",
        device_count.as_slice(),
        b"6".as_slice(),
    );
    r.check(
        "2. data/count blob is in the omitted set",
        push.omitted_data_blob_oids.contains(&data_count_oid),
        format!("omitted {:?}", push.omitted_data_blob_oids),
    );
    let present: Vec<&String> = push
        .omitted_data_blob_oids
        .iter()
        .filter(|o| has_object(&router, o))
        .collect();
    r.check(
        "2. /data/count (and every other omitted data/ blob) absent from the router object store",
        !has_object(&router, &data_count_oid) && present.is_empty(),
        format!("present={present:?}"),
    );
    r.check_eq(
        "2. CogletTree SHA-256 intact on router (tip^{tree} identical)",
        rev_tree(&router, tip).ok(),
        Some(device_tip_tree.clone()),
    );
    r.check_eq(
        "2. public logs/events.jsonl blob present and identical",
        router
            .git
            .read_blob(&format!("{tip}:logs/events.jsonl"), &env)
            .ok()
            .map(text),
        Some(text(device_events.clone())),
    );
    let (ok, detail) = fsck_full(&gitdir);
    r.check("2. git --git-dir <router> fsck --full passes", ok, detail);

    // ---- 1b. Signed statements move the state ref (atomic CAS + notes) ----
    let rogue = sign_ref_statement(&rogue_sk, s1.statement.clone())?;
    let e = verify_and_apply_ref_statement(&router, &rogue, None);
    r.check(
        "1. statement signed by an unregistered iroh key REFUSED",
        e.is_err(),
        e.err().unwrap_or_default(),
    );
    match verify_and_apply_ref_statement(&router, &s1, None) {
        Ok(a) => {
            r.check(
                "1. verify_and_apply_ref_statement gen=1 (old=zero -> prev step)",
                !a.idempotent_retry && a.full_ref == state_ref && a.signer_principal == "node-1",
                format!("{} by {}", a.full_ref, a.signer_principal),
            );
        }
        Err(e) => {
            r.check(
                "1. verify_and_apply_ref_statement gen=1 (old=zero -> prev step)",
                false,
                e,
            );
        }
    }
    match verify_and_apply_ref_statement(&router, &s2, None) {
        Ok(a) => {
            let notes_tip = rev(&router, STATEMENTS_NOTES_REF);
            r.check(
                "1. verify_and_apply_ref_statement gen=2 (prev step -> tip)",
                !a.idempotent_retry && a.old_oid.as_deref() == Some(prev),
                format!("{} gen {}", a.full_ref, a.generation),
            );
            r.check_eq(
                "1. refs/notes/statements moved in the same reftable tx",
                notes_tip,
                Some(a.notes_commit_oid.clone()),
            );
            r.hash("refs/notes/statements", a.notes_commit_oid.as_str());
        }
        Err(e) => {
            r.check(
                "1. verify_and_apply_ref_statement gen=2 (prev step -> tip)",
                false,
                e,
            );
        }
    }
    r.check_eq(
        "1. router state ref == tip",
        rev(&router, &state_ref),
        Some(tip.to_string()),
    );
    r.check_eq(
        "1. refs/notes/statements entry on prev step == signed gen=1 statement",
        read_ref_statements_for_commit(&router, prev),
        Ok(vec![s1.clone()]),
    );
    r.check_eq(
        "1. refs/notes/statements entry on tip == signed gen=2 statement",
        read_ref_statements_for_commit(&router, tip),
        Ok(vec![s2.clone()]),
    );
    let e = verify_and_apply_ref_statement(&router, &s1, None);
    r.check(
        "1. replayed gen=1 statement (stale generation) REFUSED",
        e.as_ref().is_err_and(|m| m.contains("stale")),
        e.err().unwrap_or_default(),
    );
    let fork = sign_ref_statement(
        &node_sk,
        live(2, Some(tip), &cs.step_commits[0], &state_ref, "state-ref"),
    )?;
    let e = verify_and_apply_ref_statement(&router, &fork, None);
    r.check(
        "1. different statement reusing gen=2 REFUSED",
        e.as_ref().is_err_and(|m| m.contains("conflict")),
        e.err().unwrap_or_default(),
    );
    let retry = verify_and_apply_ref_statement(&router, &s2, None);
    r.check(
        "1. byte-identical gen=2 retry is an idempotent no-op (no duplicate note)",
        retry.as_ref().is_ok_and(|a| a.idempotent_retry)
            && read_ref_statements_for_commit(&router, tip).map(|v| v.len()) == Ok(1),
        retry.err().unwrap_or_default(),
    );
    r.check_eq(
        "1. state ref still tip after refused statements",
        rev(&router, &state_ref),
        Some(tip.to_string()),
    );
    for (label, refname, purpose) in [
        (
            "node-2's namespace",
            format!("refs/namespaces/node-2/refs/coglets/{id}/state"),
            "state-ref",
        ),
        (
            "refs/published/main",
            "refs/published/main".to_string(),
            "published-ref",
        ),
    ] {
        let st = sign_ref_statement(&node_sk, live(1, None, tip, &refname, purpose))?;
        let e = verify_and_apply_ref_statement(&router, &st, None);
        r.check(
            format!("1. node-1 statement for {label} (outside {NODE_REFS}) REFUSED"),
            e.as_ref().is_err_and(|m| m.contains("permission_denied"))
                && rev(&router, &refname).is_none(),
            e.err().unwrap_or_default(),
        );
    }
    router
        .git
        .delete_ref(&upload_ref, &env)
        .map_err(|e| e.to_string())?;
    r.note(format!(
        "flow forced by the API: objects arrive via the data-omitted push to `{upload_ref}` (deleted once consumed); the signed gen=1/gen=2 statements are what move `{state_ref}`"
    ));

    // ---- 3. Read-grant gate ----
    let gated = |path: &str| {
        read_coglet_state_path_gated(&router, "auditor", Some(&auditor_pk), id, &state_ref, path)
    };
    let e = gated("data/count");
    r.check(
        "3. ungranted gated read of data/count REFUSED",
        e.as_ref().is_err_and(|m| m.contains("permission_denied")),
        e.err().unwrap_or_default(),
    );
    r.check_eq(
        "3. ungranted gated read of logs/events.jsonl succeeds",
        gated("logs/events.jsonl").ok().map(text),
        Some(text(device_events.clone())),
    );
    r.check_eq(
        "3. has_valid_data_read_grant before grant",
        has_valid_data_read_grant(&router, "auditor", Some(&auditor_pk), id),
        Ok(false),
    );
    let grant = issue_signed_coglet_grant(
        &router,
        &grant_admin,
        &CogletGrantRecord {
            grantee: "auditor".into(),
            coglet: id.to_string(),
            grants: vec!["data.read".into()],
            expires_at: None,
            revoked: false,
        },
    );
    r.check(
        "3. issue_signed_coglet_grant data.read (SSH-signed by grant-admin)",
        grant.is_ok(),
        grant.clone().unwrap_or_else(|e| e),
    );
    if let Ok(g) = &grant {
        r.hash(format!("refs/grants/{id}"), g.as_str());
    }
    r.check_eq(
        "3. has_valid_data_read_grant after grant",
        has_valid_data_read_grant(&router, "auditor", Some(&auditor_pk), id),
        Ok(true),
    );
    // Granted, but the bytes are still only on the device: proves the push really omitted them.
    let e = gated("data/count");
    r.check(
        "3. granted read on the .promisor store still fails until the blob is released",
        e.is_err(),
        e.err().unwrap_or_default(),
    );
    let released = router
        .git
        .write_blob(&device_count)
        .map_err(|e| e.to_string())?;
    r.check_eq(
        "3. device releases the granted data/count blob (oid-verified)",
        released.as_str(),
        data_count_oid.as_str(),
    );
    r.note(
        "gap: unforged has no API to fill one `.promisor` blob from the device after a grant; the spike copies exactly the granted blob (oid checked) where a lazy promisor fetch would land",
    );
    r.check_eq(
        "3. granted gated read of data/count == \"6\"",
        gated("data/count").ok().map(text),
        Some("6".to_string()),
    );

    // ---- 4. Compaction (last) ----
    match compact_coglet_history(&router, &state_ref, KEEP_RECENT) {
        Ok(rep) => {
            let want: Vec<String> = cs
                .step_commits
                .iter()
                .rev()
                .take(KEEP_RECENT)
                .cloned()
                .collect();
            r.check_eq(
                format!(
                    "4. compact K={KEEP_RECENT}: latest {KEEP_RECENT} step SHA-256 ids unchanged"
                ),
                rep.kept_step_oids.clone(),
                want.clone(),
            );
            r.check_eq(
                "4. state ref still tip",
                rev(&router, &state_ref),
                Some(tip.to_string()),
            );
            let fp = router
                .git
                .run(&["log", "--first-parent", "--format=%H", &state_ref])
                .map(|s| s.lines().map(str::to_string).collect::<Vec<_>>());
            r.check_eq(
                "4. first-parent history == kept steps",
                fp.ok(),
                Some(want.clone()),
            );
            let chk_ref = rep.checkpoint_ref.clone().unwrap_or_default();
            r.check(
                "4. checkpoint ref present",
                !chk_ref.is_empty() && rev(&router, &chk_ref) == rep.checkpoint_oid,
                format!("{chk_ref} range {:?}", rep.compacted_steps_range),
            );
            if let Some(c) = &rep.checkpoint_oid {
                r.hash(chk_ref.clone(), c.as_str());
                r.check_eq(
                    "4. checkpoint keeps the last compacted step's CogletTree",
                    rev_tree(&router, c).ok(),
                    rev_tree(cs.os, &cs.step_commits[n - 1 - KEEP_RECENT]).ok(),
                );
            }
            let old_gone = cs.step_commits[..n - KEEP_RECENT]
                .iter()
                .all(|c| !has_object(&router, c));
            r.check(
                "4. older step commits pruned, kept ones present",
                old_gone && want.iter().all(|c| has_object(&router, c)),
                format!("pruned {}", rep.pruned_step_oids.len()),
            );
            r.check_eq(
                "4. tip statement note survives compaction",
                read_ref_statements_for_commit(&router, tip),
                Ok(vec![s2.clone()]),
            );
            r.check_eq(
                "4. grant still valid and data/count still readable",
                gated("data/count").ok().map(text),
                Some("6".to_string()),
            );
        }
        Err(e) => {
            r.check(format!("4. compact K={KEEP_RECENT}"), false, e);
        }
    }
    let (ok, detail) = fsck_full(&gitdir);
    r.check(
        "4. git --git-dir <router> fsck --full passes after compaction",
        ok,
        detail,
    );
    Ok(r.finish())
}

fn text(b: Vec<u8>) -> String {
    String::from_utf8_lossy(&b).into_owned()
}

fn rev(os: &GecaosOs, rev: &str) -> Option<String> {
    os.git.rev_parse_opt(rev, &GitEnv::default()).ok().flatten()
}

fn rev_tree(os: &GecaosOs, commit: &str) -> Result<String, String> {
    os.git
        .run(&["rev-parse", &format!("{commit}^{{tree}}")])
        .map(|s| s.trim().to_string())
        .map_err(|e| e.to_string())
}

fn has_object(os: &GecaosOs, oid: &str) -> bool {
    os.git
        .run_raw(&["cat-file", "-e", oid], &GitEnv::default(), None)
        .is_ok_and(|o| o.status.success())
}

fn fsck_full(git_dir: &Path) -> (bool, String) {
    match Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .args(["fsck", "--full"])
        .output()
    {
        Ok(o) => {
            let all = format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            );
            let last = all.lines().last().unwrap_or("").to_string();
            (
                o.status.success(),
                format!("exit {:?} {last}", o.status.code()),
            )
        }
        Err(e) => (false, e.to_string()),
    }
}

/// Sign with the node's fixed seed via `ssh-keygen -Y sign`, then have OpenSSH
/// verify it against the router's `allowed_signers` (which carries `refs=...`).
fn ssh_keygen_roundtrip(
    work: &Path,
    as_path: &Path,
    key_body: &str,
) -> Result<(bool, String), String> {
    let sk = PrivateKey::from(Ed25519Keypair::from_seed(&NODE_SEED));
    let derived = sk.public_key().to_openssh().map_err(|e| e.to_string())?;
    if derived.split_whitespace().nth(1) != Some(key_body) {
        return Ok((false, "seed-derived OpenSSH key != iroh node key".into()));
    }
    let key_path = work.join("node-1.key");
    let pem = sk.to_openssh(LineEnding::LF).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&key_path)
        .and_then(|mut f| f.write_all(pem.as_bytes()))
        .map_err(|e| e.to_string())?;
    let msg = work.join("payload.txt");
    std::fs::write(&msg, b"coglet act3 allowed_signers probe\n").map_err(|e| e.to_string())?;
    let sig = work.join("payload.txt.sig");
    let out = |c: &mut Command| c.output().map_err(|e| e.to_string());
    let sign = out(Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "git", "-f"])
        .arg(&key_path)
        .arg(&msg))?;
    if !sign.status.success() {
        return Ok((
            false,
            format!("sign: {}", String::from_utf8_lossy(&sign.stderr)),
        ));
    }
    let stdin = std::fs::File::open(&msg).map_err(|e| e.to_string())?;
    let verify = out(Command::new("ssh-keygen")
        .args(["-Y", "verify", "-n", "git", "-I", "node-1", "-f"])
        .arg(as_path)
        .arg("-s")
        .arg(&sig)
        .stdin(stdin))?;
    let find = out(Command::new("ssh-keygen")
        .args(["-Y", "find-principals", "-f"])
        .arg(as_path)
        .arg("-s")
        .arg(&sig))?;
    // The principals field is `node-1,@runtime`, so find-principals lists both.
    let principals = String::from_utf8_lossy(&find.stdout)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",");
    let ok = verify.status.success()
        && find.status.success()
        && principals.split(',').any(|p| p == "node-1");
    Ok((
        ok,
        format!(
            "verify: {} | find-principals: {principals}",
            String::from_utf8_lossy(&verify.stdout).trim()
        ),
    ))
}

fn statement_from_json(st: &serde_json::Value) -> Result<RefStatement, String> {
    let s = |k: &str| st[k].as_str().map(str::to_string);
    Ok(RefStatement {
        purpose: s("purpose").ok_or("fixture purpose")?,
        hub: s("hub"),
        coglet: s("coglet"),
        generation: st["generation"].as_u64().ok_or("fixture generation")?,
        refname: s("ref").ok_or("fixture ref")?,
        old_oid: s("old"),
        new_oid: s("new").ok_or("fixture new")?,
        archive_sha256: s("archive_sha256"),
    })
}

fn json_to_hub(v: &serde_json::Value) -> Result<HubValue, String> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => HubValue::Null,
        J::Bool(b) => HubValue::Bool(*b),
        J::Number(n) => match (n.as_u64(), n.as_i64()) {
            (Some(u), _) => HubValue::Integer(u.into()),
            (None, Some(i)) => HubValue::Integer(i.into()),
            _ => return Err(format!("non-integer number {n}")),
        },
        J::String(s) => HubValue::Text(s.clone()),
        J::Array(a) => HubValue::Array(a.iter().map(json_to_hub).collect::<Result<_, _>>()?),
        J::Object(m) => HubValue::Map(
            m.iter()
                .map(|(k, x)| Ok((HubValue::Text(k.clone()), json_to_hub(x)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

fn cbor_to_hub(v: &CborValue) -> Result<HubValue, String> {
    Ok(match v {
        CborValue::Int(i) if *i >= 0 => {
            HubValue::Integer(u64::try_from(*i).map_err(|e| e.to_string())?.into())
        }
        CborValue::Int(i) => {
            HubValue::Integer(i64::try_from(*i).map_err(|e| e.to_string())?.into())
        }
        CborValue::Text(s) => HubValue::Text(s.clone()),
        CborValue::Bool(b) => HubValue::Bool(*b),
        CborValue::Null => HubValue::Null,
        CborValue::Array(a) => {
            HubValue::Array(a.iter().map(cbor_to_hub).collect::<Result<_, _>>()?)
        }
        CborValue::Map(m) => HubValue::Map(
            m.iter()
                .map(|(k, x)| Ok((HubValue::Text(k.clone()), cbor_to_hub(x)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}
