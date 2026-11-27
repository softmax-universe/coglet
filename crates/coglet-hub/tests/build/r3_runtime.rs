//! Required native replay of the ONE real signed publication fixture. No rewrite
//! or synthetic CI construction occurs here; both hosts consume identical bytes.
use coglet_hub::build::Artifact;
use coglet_runtime::{NativeEngine, Runtime, RuntimeService, Snapshot};
#[path = "../support/r3_diagnostics.rs"]
mod r3_diagnostics;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
fn hash(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("/usr/bin/git")
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8(o.stdout).unwrap().trim().into()
}
fn admitted_fixture(dir: &Path) -> (Value, Vec<u8>) {
    let setup: Value =
        serde_json::from_slice(&fs::read(dir.join("runtime-setup.json")).unwrap()).unwrap();
    let mut a: Artifact =
        serde_json::from_slice(&fs::read(dir.join("publication.json")).unwrap()).unwrap();
    // Artifact metadata intentionally omits bundle bytes; envelope admission
    // below authenticates the separately retained transfer against signed pins.
    a.bundle = fs::read(dir.join("publication.bundle")).unwrap();
    let wasm = fs::read(dir.join("step.wasm")).unwrap();
    assert_eq!(a.provenance["rewritten_sha256"], hash(&wasm));
    let metadata = coglet_proto::git_metadata::VerifiedRef::verify(
        &a.signed,
        coglet_proto::git_metadata::RefKind::Hub,
        a.signed["signer"].as_str().unwrap(),
        setup["manifest"]["id"].as_str().unwrap(),
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async {
        use coglet_proto::transfer::DurableSink;
        let mut sink =
            coglet_proto::quarantine::BundleSink::create(root.path(), a.bundle.len() as u64)
                .await
                .unwrap();
        for chunk in a.bundle.chunks(16384) {
            sink.append(chunk).await.unwrap();
        }
        let env = sink
            .check_envelope(&metadata, &hash(&a.bundle), 100000)
            .await
            .unwrap();
        env.validate(
            &coglet_proto::validator::Validator::new(env!(
                "CARGO_BIN_EXE_coglet-hub-build-validator"
            )),
            coglet_proto::validator::Limits::default(),
        )
        .await
        .unwrap();
    });
    // Verify the entire runtime /code projection against the signed archive, not
    // only step.wasm. Native replay cannot invent or launder provenance.
    let tree = tempfile::tempdir().unwrap();
    fs::write(tree.path().join("transfer.bundle"), &a.bundle).unwrap();
    git(tree.path(), &["init", "--initial-branch=proof"]);
    git(tree.path(), &["fetch", "transfer.bundle", "refs/heads/hub"]);
    git(tree.path(), &["checkout", "--detach", "FETCH_HEAD"]);
    let mut files = serde_json::Map::new();
    for name in git(tree.path(), &["ls-tree", "-r", "--name-only", "HEAD"]).lines() {
        files.insert(
            format!("/code/{name}"),
            json!(hex::encode(fs::read(tree.path().join(name)).unwrap())),
        );
    }
    let configured: serde_json::Map<_, _> = setup["initial_files"]
        .as_object()
        .unwrap()
        .iter()
        .filter(|(k, _)| k.starts_with("/code/"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(files, configured);
    assert_eq!(
        fs::read(tree.path().join("manifest.json")).unwrap(),
        serde_json::to_vec(&setup["manifest"]).unwrap()
    );
    assert_eq!(
        coglet_proto::wire::decode(
            &hex::decode(configured["/code/build/provenance.cbor"].as_str().unwrap()).unwrap()
        )
        .unwrap(),
        a.provenance
    );
    assert_eq!(
        hex::decode(configured["/code/bin/step.wasm"].as_str().unwrap()).unwrap(),
        wasm
    );
    assert_eq!(
        a.reproduction["evidence"]["gates"]["output_sha256"],
        hash(&wasm)
    );
    (setup, wasm)
}
#[test]
fn published_r3_runtime_journal_fs_trap_payload_chain() {
    let dir = PathBuf::from(
        std::env::var_os("COGLET_R3_PUBLICATION_DIR")
            .expect("required proof needs the real publication artifact"),
    );
    let out = PathBuf::from(
        std::env::var_os("COGLET_R3_RUNTIME_DIR")
            .expect("required proof retains raw runtime evidence"),
    );
    // Immutable pins are hashed from the retained inputs before any execution.
    let pins = r3_diagnostics::Pins::from_publication_dir(&dir).unwrap();
    let diagnostics =
        r3_diagnostics::Diagnostics::create(&out, std::env::consts::ARCH, pins).unwrap();
    fs::create_dir_all(&out).unwrap();
    let (setup, wasm) = admitted_fixture(&dir);
    let engine = || {
        NativeEngine::new(
            wasm.clone(),
            &setup["manifest"]["budgets"],
            16 * 1024 * 1024,
            65536,
        )
        .unwrap()
    };
    let root = tempfile::tempdir().unwrap();
    let wal = root.path().join("runtime.wal");
    let mut context = setup["inputs"].clone();
    let mut seed = 0xfff8_0000_0000_0042u64;
    let mut hashes = Vec::new();
    let mut records = Vec::new();
    let mut initial_hash = None;
    for step in 0..3 {
        // An actual durable close/open between attempts, not a cloned runtime.
        let mut service = RuntimeService::open(&wal, &setup, 128).unwrap();
        assert_eq!(service.records().len(), step);
        let mut event = setup["inputs"]["events"][0].clone();
        event["seq"] = json!(step + 1);
        event["logical_time"] = json!(10 + step);
        service.enqueue(event).unwrap();
        context["logical_time"] = json!(10 + step);
        context["seed"] = json!(seed);
        let outcome = service.run_next(&context, &mut engine());
        // Retain the complete original outcome before any unwrap/assertion.
        diagnostics.persist_or_panic(step, &context, &outcome);
        let result = outcome.unwrap().unwrap();
        let r = &result["journal"];
        assert_eq!(r["step"], step);
        let approved: Value = serde_json::from_str(include_str!("qualified-ci.json")).unwrap();
        assert_eq!(r["engine"], approved["runtime_engine"]);
        assert_ne!(r["engine"], setup["inputs"]["engine"]);
        assert!(r["usage"]["compute"].as_u64().unwrap() > 0);
        assert_eq!(r["calls"].as_array().unwrap().len(), 1);
        assert_eq!(r["calls"][0]["result"], 0);
        let message = r["calls"][0]["argument"]["message"].as_str().unwrap();
        let values: Vec<_> = (0..6)
            .map(|n| u64::from_str_radix(&message[n * 16..n * 16 + 16], 16).unwrap())
            .collect();
        let canonical64 = coglet_transform::r3_bits(
            coglet_transform::Op::Add,
            coglet_transform::Width::F64,
            seed,
            1f64.to_bits(),
        );
        let canonical32 = coglet_transform::r3_bits(
            coglet_transform::Op::Add,
            coglet_transform::Width::F32,
            0xffc1_2345,
            1f32.to_bits().into(),
        );
        assert_eq!(
            values,
            vec![
                seed,
                seed | 0x8000_0000_0000_0000,
                canonical64,
                0xffc1_2345,
                0xffc1_2345,
                canonical32
            ]
        );
        assert_eq!(
            r["output_files"]["/data/payload.bin"], setup["initial_files"]["/data/payload.bin"],
            "raw payload-bearing state must not normalize"
        );
        let snapshot =
            Snapshot::new(serde_json::from_value(r["output_files"].clone()).unwrap()).unwrap();
        assert_eq!(result["state_hash"], snapshot.tree_hash().unwrap());
        if step == 0 {
            initial_hash = Some(r["input_hash"].clone());
        }
        if step < 2 {
            assert_eq!(r["status"], "committed");
            assert_ne!(r["input_hash"], r["output_hash"]);
            let logs =
                hex::decode(r["output_files"]["/logs/events.jsonl"].as_str().unwrap()).unwrap();
            let lines: Vec<_> = String::from_utf8(logs)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .collect();
            assert_eq!(lines.len(), step + 1);
            assert_eq!(lines.last().unwrap()["message"], message);
            // Feed the previous raw payload back through the recorded next input;
            // after two copies, consume the canonical arithmetic result on trap.
            seed = if step == 0 { values[0] } else { values[2] };
        } else {
            assert_eq!(r["status"], "aborted");
            assert_eq!(r["failure"]["kind"], "trap");
            assert_eq!(r["input_hash"], r["output_hash"]);
            assert_eq!(r["input_files"], r["output_files"]);
            assert_eq!(r["logs"], json!([]), "staged log must roll back on trap");
        }
        assert_eq!(service.queued(), 0, "trapped input consumed exactly once");
        hashes.push(result["state_hash"].as_str().unwrap().to_owned());
        records = service.records().to_vec();
    }
    assert_ne!(hashes[0], hashes[1]);
    assert_eq!(hashes[1], hashes[2]);
    let reopened = RuntimeService::open(&wal, &setup, 128).unwrap();
    assert_eq!(reopened.records(), records);
    assert_eq!(reopened.queued(), 0);
    let replay = Runtime::from_fixture(&setup)
        .unwrap()
        .replay(&records, &mut engine())
        .unwrap();
    assert_eq!(
        replay, hashes,
        "real guest reexecution of committed and trapped attempts"
    );
    fs::write(
        out.join("records.json"),
        serde_json::to_vec(&records).unwrap(),
    )
    .unwrap();
    fs::write(
        out.join("runtime.json"),
        serde_json::to_vec(&json!({
            "arch":std::env::consts::ARCH,"mode":"native","wasm_sha256":hash(&wasm),
            "setup_sha256":hash(&fs::read(dir.join("runtime-setup.json")).unwrap()),
            "publication_sha256":hash(&fs::read(dir.join("publication.json")).unwrap()),
            "initial_hash":initial_hash,"state_hashes":hashes,"replay_hashes":replay,
            "statuses":["committed","committed","aborted"],"cases":3
        }))
        .unwrap(),
    )
    .unwrap();
}
