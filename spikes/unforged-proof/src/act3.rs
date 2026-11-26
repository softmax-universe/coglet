//! Act 3: Stage 3 router on a real unforged hub. hub0 statement encoding, a
//! private `data/`-omitted `.promisor` push, `coglet-ref-statement/v1` statements
//! over ADR-0040 `refs/users/<coglet>/...` (unforged#9 atomic generations and
//! receipts, #10 repository/namespace/intent binding and role separation), per-user
//! read isolation (#11: advertisement, OID wants, path reads, lazy promisor fetch,
//! grant scoping and revocation), and compaction.
//!
//! `cs.os` plays the device and is only read. The hub is a fresh router
//! `GecaosOs` under `cs.work_dir`, so Act 3 never moves a ref, writes config or
//! prunes an object in the shared store that later acts read.
//!
//! The order is the one the real API allows. Statements require the target commit
//! to be present on the router already, and `push_coglet_state_without_data`
//! always CAS-moves the ref it is given. So the device first pushes the chain
//! (data omitted) to a public node-scoped upload ref, signed v1 statements move the
//! protected `refs/users/<c>/state`, and the public refs are deleted before the
//! read-isolation checks. Compaction runs last.
use crate::report::{ActReport, CounterState};
use coglet_types_hub0::Value as HubValue;
use gecaos::auth::{NamespaceTier, allowed_signers_path, generate_signing_identity_with_tier};
use gecaos::coglet::{CogletRefLayout, parse_coglet_ref};
use gecaos::coglet_protected_refs::{
    PROTECTED_REF_STATEMENT_VERSION, ProtectedRefIntent, ProtectedRefPurpose,
    ProtectedRefStatement, gate_unsigned_ref_write, is_protected_ref, read_generation_watermark,
    set_repository_id, sign_protected_ref_statement, verify_and_apply_protected_ref_statement,
};
use gecaos::coglet_router::{
    CborValue, CogletGrantRecord, RefStatement, STATEMENTS_NOTES_REF, SignedRefStatement,
    compact_coglet_history, encode_deterministic_cbor, iroh_pubkey_to_openssh,
    iroh_pubkey_to_openssh_str, push_coglet_state_without_data,
    read_protected_ref_statements_for_commit, read_ref_statements_for_commit,
    register_iroh_allowed_signer, sign_ref_statement, verify_and_apply_ref_statement,
    verify_ref_statement_signature, write_ref_permissions,
};
use gecaos::coglet_user_isolation::{
    ReadPrincipal, authorize_upload_pack_wants_for, fetch_promisor_private_object, hidden_refs_for,
    issue_signed_user_grant, read_user_ref_path_gated, user_isolation_active,
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
const NODE2_SEED: [u8; 32] = [0x32; 32];
const KERNEL_SEED: [u8; 32] = [0x4b; 32];
const FACTORY_SEED: [u8; 32] = [0x46; 32];
/// `unforged.repoId` of the hub: every v1 statement is bound to it.
const REPO_ID: &str = "coglet-unforged-proof-hub";
/// The node key may move refs only under its own runtime namespace.
const NODE_REFS: &str = "refs/namespaces/node-1/**";
const KEEP_RECENT: usize = 2;
/// coglet's own golden `hub-ref-statement` signatures (same tree as this spike).
const HUB0_FIXTURES: &str =
    include_str!("../../../crates/coglet-types-hub0/fixtures/signatures.json");

pub fn run(cs: &CounterState) -> Result<ActReport, String> {
    let mut r = ActReport::new(
        "3",
        "Stage 3: refs/users v1 statements (#9/#10), per-user read isolation (#11), .promisor push, compaction",
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
    // ADR-0040: the coglet is a user of the hub, inside node-1's runtime namespace.
    let users_state = format!(
        "refs/namespaces/node-1/{}",
        CogletRefLayout::Users.state_ref(id)
    );
    // Public (unprotected) refs: the data-omitted upload and the legacy hub0 ref.
    let upload_ref = format!("refs/namespaces/node-1/refs/uploads/{id}");
    let legacy_ref = format!("refs/namespaces/node-1/refs/hub/{id}/state");
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
    set_repository_id(&router, REPO_ID)?;

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
    for (name, group, seed, refs) in [
        (
            "node-2",
            "runtime",
            NODE2_SEED,
            vec!["refs/namespaces/node-2/**"],
        ),
        (
            "kernel",
            "kernel",
            KERNEL_SEED,
            vec!["refs/namespaces/*/refs/users/*/published"],
        ),
        // The factory's ACL deliberately also covers `published`: role separation,
        // not the ACL, is what keeps a policy producer from promoting.
        (
            "factory",
            "factory",
            FACTORY_SEED,
            vec![
                "refs/namespaces/*/refs/users/*/policy",
                "refs/namespaces/*/refs/users/*/published",
            ],
        ),
    ] {
        register_iroh_allowed_signer(
            &router,
            name,
            &[group],
            NamespaceTier::GitOnly,
            &iroh::SecretKey::from_bytes(&seed).public(),
            &refs,
            &[],
        )?;
    }
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
        &format!(
            "mode router\nnode-1      allow {NODE_REFS}\nnode-2      allow refs/namespaces/node-2/**\n\
             kernel      allow refs/namespaces/*/refs/users/*/published\n\
             factory     allow refs/namespaces/*/refs/users/*/policy\n\
             factory     allow refs/namespaces/*/refs/users/*/published\n\
             grant-admin allow refs/grants/*\ngrant-admin allow refs/namespaces/*/refs/grants/*\n"
        ),
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
    let s1 = sign_ref_statement(&node_sk, live(1, None, prev, &legacy_ref, "state-ref"))?;
    let s2 = sign_ref_statement(&node_sk, live(2, Some(prev), tip, &legacy_ref, "state-ref"))?;
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

    // ---- 1b. Legacy hub-ref-statements on a NON-protected hub ref ----
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
                "1. legacy verify_and_apply_ref_statement gen=1 (old=zero -> prev step)",
                !a.idempotent_retry && a.full_ref == legacy_ref && a.signer_principal == "node-1",
                format!("{} by {}", a.full_ref, a.signer_principal),
            );
        }
        Err(e) => {
            r.check(
                "1. legacy verify_and_apply_ref_statement gen=1 (old=zero -> prev step)",
                false,
                e,
            );
        }
    }
    match verify_and_apply_ref_statement(&router, &s2, None) {
        Ok(a) => {
            r.check(
                "1. legacy verify_and_apply_ref_statement gen=2 (prev step -> tip)",
                !a.idempotent_retry && a.old_oid.as_deref() == Some(prev),
                format!("{} gen {}", a.full_ref, a.generation),
            );
            r.check_eq(
                "1. refs/notes/statements moved in the same reftable tx",
                rev(&router, STATEMENTS_NOTES_REF),
                Some(a.notes_commit_oid.clone()),
            );
            r.check(
                "1. #9: legacy apply also lands its generation watermark + operation receipt refs",
                rev(&router, &a.watermark_ref).is_some()
                    && rev(&router, &a.operation_receipt_ref).is_some(),
                format!("{} / {}", a.watermark_ref, a.operation_receipt_ref),
            );
        }
        Err(e) => {
            r.check(
                "1. legacy verify_and_apply_ref_statement gen=2 (prev step -> tip)",
                false,
                e,
            );
        }
    }
    r.check_eq(
        "1. legacy hub ref == tip",
        rev(&router, &legacy_ref),
        Some(tip.to_string()),
    );
    r.check_eq(
        "1. refs/notes/statements entry on prev step == signed gen=1 statement",
        read_ref_statements_for_commit(&router, prev),
        Ok(vec![s1.clone()]),
    );
    let e = verify_and_apply_ref_statement(&router, &s1, None);
    r.check(
        "1. replayed gen=1 statement after gen=2: original outcome or stale, never a ref move",
        e.as_ref()
            .is_ok_and(|a| a.idempotent_retry && a.new_oid == prev)
            || e.as_ref().is_err_and(|m| m.contains("stale")),
        match &e {
            Ok(a) => format!("idempotent_retry={} new={}", a.idempotent_retry, a.new_oid),
            Err(m) => m.clone(),
        },
    );
    let fork = sign_ref_statement(
        &node_sk,
        live(2, Some(tip), &cs.step_commits[0], &legacy_ref, "state-ref"),
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
        retry.as_ref().is_ok_and(|a| a.idempotent_retry),
        retry.err().unwrap_or_default(),
    );
    r.check_eq(
        "1. legacy hub ref still tip after refused statements",
        rev(&router, &legacy_ref),
        Some(tip.to_string()),
    );
    for (label, refname, purpose) in [
        (
            "node-2's namespace",
            format!("refs/namespaces/node-2/refs/hub/{id}/state"),
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
            format!("1. node-1 statement for {label} REFUSED"),
            e.as_ref().is_err_and(|m| m.contains("permission_denied"))
                && rev(&router, &refname).is_none(),
            e.err().unwrap_or_default(),
        );
    }
    let st = sign_ref_statement(&node_sk, live(1, None, tip, &users_state, "state-ref"))?;
    let e = verify_and_apply_ref_statement(&router, &st, None);
    r.check(
        "1. #10: legacy hub0-domain statement on protected refs/users/<c>/state REFUSED (domain separation)",
        e.as_ref().is_err_and(|m| m.contains("protected")) && rev(&router, &users_state).is_none(),
        e.err().unwrap_or_default(),
    );

    // ---- 5. coglet-ref-statement/v1 on ADR-0040 refs/users (unforged#9, #10) ----
    stage3_v1(&mut r, cs, &router, &node_sk, &users_state)?;

    // Only the protected users/<c>/state ref may reach the tip from here on: the upload ref
    // and the legacy hub ref are public and would hand the chain to every reader.
    for gone in [&upload_ref, &legacy_ref] {
        router
            .git
            .delete_ref(gone, &env)
            .map_err(|e| e.to_string())?;
    }
    r.note(format!(
        "flow forced by the API: objects arrive via the data-omitted push to `{upload_ref}`; signed v1 statements move `{users_state}`; the public upload and legacy hub refs are deleted before the read-isolation checks"
    ));

    // ---- 3. Per-user read isolation over refs/users (unforged#11) ----
    let node2_ssh = iroh_pubkey_to_openssh(&iroh::SecretKey::from_bytes(&NODE2_SEED).public())?;
    let owner = ReadPrincipal::authenticated("node-1", Some(&node_ssh), None);
    let other = ReadPrincipal::authenticated("node-2", Some(&node2_ssh), None);
    let reader = ReadPrincipal::authenticated("auditor", Some(&auditor_pk), None);
    let anon = ReadPrincipal::unauthenticated(None);
    let node2_state = format!("refs/namespaces/node-2/refs/users/{id}/state");
    r.check_eq(
        "3. strict mode: an ADR-0040 refs/users ref exists",
        user_isolation_active(&router),
        Ok(true),
    );
    let hidden = |p: &ReadPrincipal| {
        hidden_refs_for(&router, p)
            .ok()
            .flatten()
            .unwrap_or_default()
    };
    let (h_owner, h_other, h_reader) = (hidden(&owner), hidden(&other), hidden(&reader));
    r.check(
        "3. advertisement: node-1:<c>/state hidden from node-2 and auditor, visible to node-1",
        h_other.contains(&users_state)
            && h_reader.contains(&users_state)
            && !h_owner.contains(&users_state),
        format!(
            "hidden: owner={} other={} reader={}",
            h_owner.len(),
            h_other.len(),
            h_reader.len()
        ),
    );
    r.check(
        "3. advertisement: same coglet name on node-2 is a different owner (hidden from node-1)",
        h_owner.contains(&node2_state) && !h_other.contains(&node2_state),
        String::new(),
    );
    r.check(
        "3. advertisement: refs/coglet-router/* (#9 receipts) hidden from non-admin readers",
        rev_list(&router, "refs/coglet-router/")
            .iter()
            .all(|x| h_reader.contains(x) && h_other.contains(x)),
        format!(
            "{} router refs",
            rev_list(&router, "refs/coglet-router/").len()
        ),
    );
    let read =
        |p: &ReadPrincipal, path: &str| read_user_ref_path_gated(&router, p, &users_state, path);
    r.check_eq(
        "3. owner node-1 reads its logs/events.jsonl",
        read(&owner, "logs/events.jsonl").ok().map(text),
        Some(text(device_events.clone())),
    );
    for (who, p) in [
        ("node-2", &other),
        ("auditor (no grant)", &reader),
        ("anonymous", &anon),
    ] {
        let e = read(p, "logs/events.jsonl");
        r.check(
            format!("3. {who} reading node-1:<c> logs/events.jsonl REFUSED"),
            e.is_err(),
            e.err().unwrap_or_default(),
        );
    }
    let want = |p: &ReadPrincipal, oid: &str| {
        authorize_upload_pack_wants_for(&router, p, &[oid.to_string()], Some("blob:none"))
    };
    r.check(
        "3. want of node-1's tip OID with --filter=blob:none: owner allowed",
        want(&owner, tip).is_ok(),
        want(&owner, tip).err().unwrap_or_default(),
    );
    for (who, p) in [
        ("node-2", &other),
        ("auditor", &reader),
        ("anonymous", &anon),
    ] {
        let e = want(p, tip);
        r.check(
            format!("3. knowing the tip OID is not a grant: {who}'s want REFUSED"),
            e.is_err(),
            e.err().unwrap_or_default(),
        );
    }
    r.check(
        "3. shared history: node-2 may want step 1 (its own state) but not node-1's tip",
        want(&other, &cs.step_commits[0]).is_ok() && want(&other, tip).is_err(),
        want(&other, &cs.step_commits[0]).err().unwrap_or_default(),
    );
    let grant = |runtime: Option<&str>, grants: &[&str], revoked: bool| {
        issue_signed_user_grant(
            &router,
            &grant_admin,
            runtime,
            &CogletGrantRecord {
                grantee: "auditor".into(),
                coglet: id.to_string(),
                grants: grants.iter().map(|g| g.to_string()).collect(),
                expires_at: None,
                revoked,
            },
        )
    };
    let g = grant(None, &["data.read"], false);
    r.check(
        "3. a TOP-LEVEL refs/grants/<c> grant never reaches the runtime-scoped owner node-1:<c>",
        g.is_ok() && read(&reader, "logs/events.jsonl").is_err(),
        g.err().unwrap_or_default(),
    );
    let g = grant(Some("node-1"), &["logs.read"], false);
    r.check(
        "3. logs.read grant on refs/namespaces/node-1/refs/grants/<c>: logs readable, data/ still REFUSED",
        g.is_ok()
            && read(&reader, "logs/events.jsonl").ok().map(text) == Some(text(device_events.clone()))
            && read(&reader, "data/count").is_err(),
        g.err().unwrap_or_default(),
    );
    let g = grant(Some("node-1"), &["data.read"], false);
    if let Ok(oid) = &g {
        r.hash(
            format!("refs/namespaces/node-1/refs/grants/{id}"),
            oid.as_str(),
        );
    }
    r.check(
        "3. data.read granted, but the bytes are still only on the device (.promisor push omitted them)",
        g.is_ok() && read(&reader, "data/count").is_err(),
        g.err().unwrap_or_default(),
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
        "gap: unforged has no API for a device to fill one `.promisor` blob on the hub after a grant; the spike copies exactly the granted blob (oid checked)",
    );
    r.check_eq(
        "3. granted gated read of data/count == \"6\"",
        read(&reader, "data/count").ok().map(text),
        Some("6".to_string()),
    );
    r.check_eq(
        "3. lazy promisor fetch of the data/count OID under the current grant",
        fetch_promisor_private_object(&router, &reader, &data_count_oid)
            .ok()
            .map(text),
        Some("6".to_string()),
    );
    r.check(
        "3. node-2 still cannot fetch the private data/count OID",
        fetch_promisor_private_object(&router, &other, &data_count_oid).is_err(),
        String::new(),
    );

    // ---- 4. Compaction of refs/users/<c>/state ----
    match compact_coglet_history(&router, &users_state, KEEP_RECENT) {
        Ok(rep) => {
            let want_kept: Vec<String> = cs
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
                want_kept.clone(),
            );
            r.check_eq(
                "4. refs/users/<c>/state still tip",
                rev(&router, &users_state),
                Some(tip.to_string()),
            );
            let fp = router
                .git
                .run(&["log", "--first-parent", "--format=%H", &users_state])
                .map(|s| s.lines().map(str::to_string).collect::<Vec<_>>());
            r.check_eq(
                "4. first-parent history == kept steps",
                fp.ok(),
                Some(want_kept.clone()),
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
            r.check(
                "4. older step commits pruned (step 1 survives: node-2's own state ref), kept ones present",
                cs.step_commits[1..n - KEEP_RECENT]
                    .iter()
                    .all(|c| !has_object(&router, c))
                    && has_object(&router, &cs.step_commits[0])
                    && want_kept.iter().all(|c| has_object(&router, c)),
                format!("pruned {}", rep.pruned_step_oids.len()),
            );
            r.check(
                "4. tip v1 statement note survives compaction",
                read_protected_ref_statements_for_commit(&router, tip)
                    .is_ok_and(|v| v.iter().any(|s| s.statement.physical_ref == users_state)),
                String::new(),
            );
            r.check_eq(
                "4. grant still valid and data/count still readable after compaction",
                read(&reader, "data/count").ok().map(text),
                Some("6".to_string()),
            );
        }
        Err(e) => {
            r.check(format!("4. compact K={KEEP_RECENT}"), false, e);
        }
    }

    // Revocation is re-read on every request (no cached grant survives it).
    let g = grant(Some("node-1"), &["data.read"], true);
    r.check(
        "3. revoked grant: path read AND lazy promisor fetch of data/count REFUSED immediately",
        g.is_ok()
            && read(&reader, "data/count").is_err()
            && fetch_promisor_private_object(&router, &reader, &data_count_oid).is_err(),
        g.err().unwrap_or_default(),
    );
    let (ok, detail) = fsck_full(&gitdir);
    r.check(
        "4. git --git-dir <router> fsck --full passes after compaction",
        ok,
        detail,
    );
    Ok(r.finish())
}

/// unforged#9/#10: `coglet-ref-statement/v1` statements over ADR-0040
/// `refs/namespaces/<runtime>/refs/users/<coglet>/...`, bound to the repository id,
/// runtime namespace, exact physical ref, generation, purpose and operation intent.
fn stage3_v1(
    r: &mut ActReport,
    cs: &CounterState,
    router: &GecaosOs,
    node_sk: &iroh::SecretKey,
    users_state: &str,
) -> Result<(), String> {
    let id = cs.coglet_id;
    let n = cs.step_commits.len();
    let (tip, prev) = (cs.tip, cs.step_commits[n - 2].as_str());
    let kernel_sk = iroh::SecretKey::from_bytes(&KERNEL_SEED);
    let factory_sk = iroh::SecretKey::from_bytes(&FACTORY_SEED);
    let node2_sk = iroh::SecretKey::from_bytes(&NODE2_SEED);
    let inner_state = CogletRefLayout::Users.state_ref(id);
    let parsed = parse_coglet_ref(&inner_state);
    r.check(
        "5. ADR-0040 layout: CogletRefLayout::Users.state_ref(c) == refs/users/<c>/state, parse_coglet_ref round-trips",
        inner_state == format!("refs/users/{id}/state")
            && parsed.as_ref().is_some_and(|p| p.to_ref() == inner_state),
        format!("{parsed:?}"),
    );
    r.check(
        "5. shared gate: an unsigned write to refs/users/<c>/state is refused, refs/heads/* is not",
        is_protected_ref(users_state)
            && gate_unsigned_ref_write(&router.git, users_state, "act3")
                .is_err_and(|e| e.contains("protected_ref"))
            && !is_protected_ref("refs/heads/main")
            && gate_unsigned_ref_write(&router.git, "refs/heads/main", "act3").is_ok(),
        String::new(),
    );
    #[allow(clippy::too_many_arguments)]
    fn stmt(
        ns: &str,
        inner: &str,
        old: Option<&str>,
        new: &str,
        generation: u64,
        purpose: ProtectedRefPurpose,
        op: &str,
        ticket: &str,
        repo: &str,
    ) -> ProtectedRefStatement {
        let physical_ref = format!("refs/namespaces/{ns}/{inner}");
        ProtectedRefStatement {
            version: PROTECTED_REF_STATEMENT_VERSION.into(),
            repository_id: repo.into(),
            namespace: Some(ns.into()),
            old_oid: old.map(str::to_string),
            new_oid: new.into(),
            generation,
            purpose,
            intent: ProtectedRefIntent {
                actor: ns.into(),
                target: physical_ref.clone(),
                ticket: ticket.into(),
                operation_id: op.into(),
                intent_digest: gecaos::coglet::sha256_hex(
                    format!("{op}/{ticket}/{physical_ref}").as_bytes(),
                ),
            },
            physical_ref,
        }
    }
    let apply = |sk: &iroh::SecretKey, st: ProtectedRefStatement, route: Option<&str>| {
        sign_protected_ref_statement(sk, st)
            .and_then(|s| verify_and_apply_protected_ref_statement(router, &s, route))
    };
    let state = |old, new, generation, op: &str, ticket: &str| {
        stmt(
            "node-1",
            &inner_state,
            old,
            new,
            generation,
            ProtectedRefPurpose::StateExport,
            op,
            ticket,
            REPO_ID,
        )
    };

    let a1 = apply(
        node_sk,
        state(None, prev, 1, "export-1", "TCK-1"),
        Some("node-1"),
    );
    r.check(
        "5. v1 StateExport gen=1 (zero -> prev step) on refs/namespaces/node-1/refs/users/<c>/state",
        a1.as_ref().is_ok_and(|a| a.full_ref == users_state && !a.idempotent_retry),
        a1.as_ref().map(|a| a.full_ref.clone()).unwrap_or_else(|e| e.clone()),
    );
    let a2 = apply(
        node_sk,
        state(Some(prev), tip, 2, "export-2", "TCK-2"),
        Some("node-1"),
    );
    match &a2 {
        Ok(a) => {
            r.check_eq(
                "5. #9: watermark + receipt + target + refs/notes/statements land in ONE ref transaction",
                (
                    rev(router, users_state),
                    rev(router, STATEMENTS_NOTES_REF),
                    rev(router, &a.watermark_ref).is_some(),
                    rev(router, &a.operation_receipt_ref).is_some(),
                ),
                (Some(tip.to_string()), Some(a.notes_commit_oid.clone()), true, true),
            );
            r.hash(
                "v1 operation receipt ref (gen=2)",
                a.operation_receipt_ref.as_str(),
            );
        }
        Err(e) => {
            r.check(
                "5. v1 StateExport gen=2 (prev step -> tip)",
                false,
                e.clone(),
            );
        }
    }
    r.check_eq(
        "5. #9: authoritative generation watermark == 2",
        read_generation_watermark(router, users_state),
        Ok(Some(2)),
    );
    let retry = apply(
        node_sk,
        state(None, prev, 1, "export-1", "TCK-1"),
        Some("node-1"),
    );
    r.check(
        "5. #9: retry of gen=1 AFTER gen=2 returns the ORIGINAL outcome (no ref move)",
        retry
            .as_ref()
            .is_ok_and(|a| a.idempotent_retry && a.new_oid == prev && a.generation == 1)
            && rev(router, users_state).as_deref() == Some(tip),
        retry
            .as_ref()
            .map(|a| format!("retry={} new={}", a.idempotent_retry, a.new_oid))
            .unwrap_or_else(|e| e.clone()),
    );
    for (label, st, route, needle) in [
        (
            "same operation id, changed ticket (different payload)",
            state(Some(prev), tip, 2, "export-2", "TCK-2b"),
            Some("node-1"),
            "conflict",
        ),
        (
            "fresh operation at gen=2 <= watermark (ABA / stale)",
            state(Some(tip), prev, 2, "export-2-aba", "TCK-3"),
            Some("node-1"),
            "stale",
        ),
        (
            "wrong repository id",
            stmt(
                "node-1",
                &inner_state,
                Some(tip),
                prev,
                3,
                ProtectedRefPurpose::StateExport,
                "export-3",
                "TCK-3",
                "some-other-repo",
            ),
            Some("node-1"),
            "repository",
        ),
        (
            "reroute (signed for node-1, arrived via node-2)",
            state(Some(tip), prev, 3, "export-3", "TCK-3"),
            Some("node-2"),
            "reroute",
        ),
        (
            "reroute (signed for node-1, arrived at top level)",
            state(Some(tip), prev, 3, "export-3", "TCK-3"),
            None,
            "reroute",
        ),
        (
            "node-1 key moving node-2's refs/users/<c>/state",
            stmt(
                "node-2",
                &inner_state,
                None,
                tip,
                1,
                ProtectedRefPurpose::StateExport,
                "x-1",
                "TCK-X",
                REPO_ID,
            ),
            Some("node-2"),
            "permission_denied",
        ),
        (
            "purpose StateExport aimed at refs/users/<c>/published",
            stmt(
                "node-1",
                &CogletRefLayout::Users.published_ref(id),
                None,
                tip,
                1,
                ProtectedRefPurpose::StateExport,
                "x-2",
                "TCK-X",
                REPO_ID,
            ),
            Some("node-1"),
            "purpose",
        ),
    ] {
        let e = apply(node_sk, st, route);
        r.check(
            format!("5. #10: {label} REFUSED"),
            e.as_ref().is_err_and(|m| m.contains(needle)),
            e.err().unwrap_or_else(|| "accepted".into()),
        );
    }
    let sha1 = apply(
        node_sk,
        state(Some(tip), &"a".repeat(40), 3, "export-sha1", "TCK-4"),
        Some("node-1"),
    );
    r.check(
        "5. #10: SHA-1 (40-hex) target REFUSED: the v1 profile is SHA-256 only",
        sha1.is_err() && rev(router, users_state).as_deref() == Some(tip),
        sha1.err().unwrap_or_else(|| "accepted".into()),
    );
    // Role separation: kernel alone promotes `published`; the factory only proposes policy.
    let policy = CogletRefLayout::Users.policy_ref(id);
    let published = CogletRefLayout::Users.published_ref(id);
    let role = |sk: &iroh::SecretKey, inner: &str, purpose, op: &str| {
        apply(
            sk,
            stmt("node-1", inner, None, tip, 1, purpose, op, "TCK-R", REPO_ID),
            Some("node-1"),
        )
    };
    let f_pub = role(
        &factory_sk,
        &published,
        ProtectedRefPurpose::PublishPromote,
        "f-pub",
    );
    r.check(
        "5. #10: @factory PublishPromote of refs/users/<c>/published REFUSED (ACL allows it; role does not)",
        f_pub.as_ref().is_err_and(|m| m.contains("permission_denied")),
        f_pub.err().unwrap_or_else(|| "accepted".into()),
    );
    let u_pub = role(
        node_sk,
        &published,
        ProtectedRefPurpose::PublishPromote,
        "u-pub",
    );
    r.check(
        "5. #10: runtime/user key PublishPromote REFUSED (user cannot publish)",
        u_pub.as_ref().is_err_and(|m| m.contains("kernel")),
        u_pub.err().unwrap_or_else(|| "accepted".into()),
    );
    let f_pol = role(
        &factory_sk,
        &policy,
        ProtectedRefPurpose::PolicyPropose,
        "f-pol",
    );
    r.check(
        "5. #10: @factory PolicyPropose of refs/users/<c>/policy accepted",
        f_pol.is_ok(),
        f_pol.err().unwrap_or_default(),
    );
    let k_pub = role(
        &kernel_sk,
        &published,
        ProtectedRefPurpose::PublishPromote,
        "k-pub",
    );
    r.check(
        "5. #10: @kernel PublishPromote of refs/users/<c>/published accepted",
        k_pub.is_ok()
            && rev(router, &format!("refs/namespaces/node-1/{published}")).as_deref() == Some(tip),
        k_pub.err().unwrap_or_default(),
    );
    // Same coglet name on a second runtime: node-2 exports its own (shorter) chain.
    let n2 = apply(
        &node2_sk,
        stmt(
            "node-2",
            &inner_state,
            None,
            &cs.step_commits[0],
            1,
            ProtectedRefPurpose::StateExport,
            "n2-1",
            "TCK-N2",
            REPO_ID,
        ),
        Some("node-2"),
    );
    r.check(
        "5. node-2 exports refs/namespaces/node-2/refs/users/<c>/state (same coglet name, other runtime)",
        n2.is_ok(),
        n2.err().unwrap_or_default(),
    );
    r.check(
        "5. v1 statement for the tip is readable from refs/notes/statements",
        read_protected_ref_statements_for_commit(router, tip).is_ok_and(|v| {
            v.iter()
                .any(|s| s.statement.physical_ref == users_state && s.statement.generation == 2)
        }),
        String::new(),
    );
    Ok(())
}

fn rev_list(os: &GecaosOs, prefix: &str) -> Vec<String> {
    os.git
        .run(&["for-each-ref", "--format=%(refname)", prefix])
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default()
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
