//! Independent executable replay; run with --features native-engine.
// The qualification checker always enables native-engine and requires all five
// named results; default-feature workspace tests are not qualification evidence.

#[cfg(feature = "native-engine")]
#[allow(dead_code)] // builder is used by the separate regeneration example
#[path = "support/archive_corpus.rs"]
mod corpus;
#[cfg(feature = "native-engine")]
mod native {
    use super::corpus;
    use coglet_runtime::{Runtime, Snapshot, cbor, digest};
    use serde_json::{Value, json};
    use std::collections::BTreeSet;
    fn fixture(name: &str) -> Value {
        serde_json::from_slice(
            &std::fs::read(corpus::root().join(format!("conformance/abi/{name}"))).unwrap(),
        )
        .unwrap()
    }
    fn trace(case: &Value) -> &Value {
        &case["steps"][0]["send"]["trace"]
    }
    fn inventory() -> Vec<Value> {
        fixture("executable-archive.fixture")
            .as_array()
            .unwrap()
            .clone()
    }
    fn cases() -> Vec<Value> {
        fixture("executable-archive.json")
            .as_array()
            .unwrap()
            .clone()
    }

    #[test]
    fn executable_archive_inventory_covers_every_source_record() {
        let sources = corpus::sources();
        let rows = inventory();
        let companions = cases();
        assert_eq!(sources.len(), 80);
        assert_eq!(rows.len(), sources.len());
        let mut mapped = BTreeSet::new();
        let mut total = 0;
        for ((profile, source), row) in sources.iter().zip(&rows) {
            assert_eq!(row["profile"], *profile);
            assert_eq!(row["source"], source["name"]);
            assert_eq!(row["source_sha256"], digest(&cbor(source).unwrap()));
            assert_eq!(row["rejection"].is_null(), corpus::valid(source));
            assert_eq!(row["boundary"], "runtime validation only");
            let records = trace(source)["records"].as_array().unwrap();
            assert!(records.iter().all(|r| r.get("step").is_some()));
            assert_eq!(
                row["runtime_boundaries"],
                json!({"pre_step": records.iter().enumerate().filter(|(_, r)| !r["boundary"].as_array().unwrap().is_empty()).map(|(i, _)| i).collect::<Vec<_>>(), "trailing": !trace(source)["trailing_boundary"].is_null()})
            );
            if !corpus::valid(source) {
                assert_eq!(
                    row["rejection"],
                    corpus::rejection_reason(source["name"].as_str().unwrap())
                );
            }
            let mappings = row["guest_records"].as_array().unwrap();
            assert_eq!(mappings.len(), records.len());
            if !mappings.is_empty() || corpus::valid(source) {
                assert_eq!(
                    row["companion"],
                    format!("executable-{}", source["name"].as_str().unwrap())
                );
                let companion = companions
                    .iter()
                    .find(|c| c["name"] == row["companion"])
                    .unwrap();
                if corpus::valid(source) {
                    let original_trailing = &trace(source)["trailing_boundary"];
                    let new_trailing = &trace(companion)["trailing_boundary"];
                    assert_eq!(original_trailing.is_null(), new_trailing.is_null());
                    if !original_trailing.is_null() {
                        for k in ["boundary", "logical_time"] {
                            assert_eq!(new_trailing[k], original_trailing[k]);
                        }
                    }
                }
                let wat = corpus::program(source["name"].as_str().unwrap());
                assert_eq!(row["wat"], wat);
                assert_eq!(row["wasm_hex"], hex::encode(wat::parse_str(wat).unwrap()));
            }
            for (i, mapping) in mappings.iter().enumerate() {
                total += 1;
                assert_eq!(mapping["source_record"], i);
                assert_eq!(mapping["companion_record"], i);
                let companion = companions
                    .iter()
                    .find(|c| c["name"] == mapping["companion"])
                    .unwrap();
                let record = &trace(companion)["records"][i];
                assert_eq!(
                    trace(companion)["code_hash"],
                    digest(&hex::decode(row["wasm_hex"].as_str().unwrap()).unwrap())
                );
                assert!(mapped.insert((source["name"].as_str().unwrap().to_owned(), i)));
                if corpus::valid(source) {
                    for k in ["input_files", "output_files"] {
                        let without_code = |v: &Value| {
                            v.as_object()
                                .unwrap()
                                .iter()
                                .filter(|(p, _)| !p.starts_with("/code/"))
                                .map(|(p, b)| (p.clone(), b.clone()))
                                .collect::<serde_json::Map<String, Value>>()
                        };
                        assert_eq!(without_code(&record[k]), without_code(&records[i][k]));
                    }
                }
                // Verify scenario fidelity separately from pinned native expectations.
                // Native failure details/usage/code-containing hashes necessarily differ.
                for key in [
                    "events",
                    "boundary",
                    "grants",
                    "logical_time",
                    "seed",
                    "calls",
                    "actions",
                    "timers",
                    "llm_requests",
                    "logs",
                    "status",
                ] {
                    assert_eq!(
                        record[key], records[i][key],
                        "{} step {i}: {key}",
                        source["name"]
                    );
                }
            }
        }
        assert_eq!(total, 32);
        assert_eq!(
            companions.len(),
            rows.iter()
                .filter(|r| !r["guest_records"].as_array().unwrap().is_empty()
                    || r["rejection"].is_null())
                .count()
        );
    }

    #[test]
    fn executable_archive_records_reexecute_on_wamr_with_identical_step_hashes() {
        let mut steps = 0;
        for case in cases() {
            let t = trace(&case);
            let imported = Runtime::import_trace(t).unwrap();
            assert_eq!(imported.export_trace(&t["trace_id"]).unwrap(), *t);
            let s = corpus::setup(t);
            let mut fresh = Runtime::from_fixture(&s).unwrap();
            let mut records = t["records"].as_array().unwrap().clone();
            steps += records.len();
            if !t["trailing_boundary"].is_null() {
                records.push(t["trailing_boundary"].clone());
            }
            let hashes = fresh
                .replay(&records, &mut corpus::engine(t))
                .unwrap_or_else(|e| panic!("{}: {e}", case["name"]));
            assert_eq!(hashes.len(), records.len());
            for (hash, record) in hashes.iter().zip(&records) {
                let state =
                    Snapshot::new(serde_json::from_value(record["output_files"].clone()).unwrap())
                        .unwrap();
                assert_eq!(hash, record["output_hash"].as_str().unwrap());
                assert_eq!(*hash, state.tree_hash().unwrap());
                if record.get("step").is_some() {
                    assert!(record["usage"]["compute"].as_u64().unwrap() > 0);
                }
            }
        }
        assert_eq!(steps, 32);
        println!("Native WAMR replay: {steps} guest records passed, 0 failures, 0 skips");
    }

    #[test]
    fn executable_archive_corruptions_reject_before_guest_entry() {
        // Admission API cannot receive an engine: invalid archives cannot enter WAMR.
        let mut rejected = 0;
        for (_, source) in corpus::sources() {
            if !corpus::valid(&source) {
                assert!(
                    Runtime::import_trace(trace(&source)).is_err(),
                    "{}",
                    source["name"]
                );
                rejected += 1;
            }
        }
        assert_eq!(rejected, 52);
        for case in cases() {
            let mut corrupt = trace(&case).clone();
            corrupt["final_hash"] = json!("0".repeat(64));
            assert!(Runtime::import_trace(&corrupt).is_err());
        }
    }

    #[test]
    fn executable_archive_replay_rejects_guest_program_mismatch() {
        // A committed no-op journal with a new, correctly pinned aborting module is
        // structurally consistent. Archive validation accepts it; real execution must not.
        let original = cases()
            .into_iter()
            .find(|c| c["name"] == "executable-provenance-control")
            .unwrap();
        let mut s = corpus::setup(trace(&original));
        let code = wat::parse_str(corpus::program("trailing-write-after-abort")).unwrap();
        s["initial_files"]["/code/bin/step.wasm"] = json!(hex::encode(&code));
        let mut p: Value = coglet_types::decode(
            "hub-build-record",
            &hex::decode(
                s["initial_files"]["/code/build/provenance.cbor"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
        .deserialized()
        .unwrap();
        p["rewritten_sha256"] = json!(digest(&code));
        s["initial_files"]["/code/build/provenance.cbor"] = json!(hex::encode(cbor(&p).unwrap()));
        let mut t = trace(&original).clone();
        t["initial_files"] = s["initial_files"].clone();
        t["code_hash"] = json!(digest(&code));
        t["blobs"][digest(&code)] = json!(hex::encode(&code));
        let hash = Snapshot::new(serde_json::from_value(s["initial_files"].clone()).unwrap())
            .unwrap()
            .tree_hash()
            .unwrap();
        t["initial_hash"] = json!(hash);
        t["final_hash"] = json!(hash);
        let r = &mut t["records"][0];
        r["code_hash"] = json!(digest(&code));
        for k in ["before_hash", "input_hash", "output_hash"] {
            r[k] = json!(hash);
        }
        for k in ["input_files", "output_files"] {
            r[k] = s["initial_files"].clone();
        }
        r.as_object_mut().unwrap().remove("record_hash");
        r["record_hash"] = json!(digest(&cbor(r).unwrap()));
        Runtime::import_trace(&t).expect("negative control must pass archive consistency first");
        let error = Runtime::from_fixture(&s)
            .unwrap()
            .replay(t["records"].as_array().unwrap(), &mut corpus::engine(&t))
            .unwrap_err();
        assert!(error.contains("replay mismatch"), "{error}");
    }

    #[test]
    fn frozen_archive_corpus_and_spec_v0_remain_unchanged() {
        // ADR-0035 regenerates state/record identities, not inventory or semantics.
        // Pins below bind that regenerated corpus; the original spec-v0 tag stays.
        for (name, expected) in [
            (
                "boundary-receipts",
                "f71273c8c95fe68e5d2f1f7aa0744eccf44257f2878ea0adb5c45b77c42ed78b",
            ),
            (
                "provenance",
                "2cfd07fd8ca9ba64b88996102c155a3a4eb065980d101116f6f649a5c45b52ba",
            ),
            (
                "heard-archive",
                "efa862a38b6b8012b194b61bf6866e9dbd017ebf39f33370fe191b2230a3721f",
            ),
        ] {
            assert_eq!(
                digest(
                    &std::fs::read(corpus::root().join(format!("conformance/abi/{name}.json")))
                        .unwrap()
                ),
                expected
            );
        }
        // Shallow non-qualification checkouts need not have the tag; the checker
        // requires the fetched ref and CI uses fetch-depth:0.
        let tag = std::process::Command::new("git")
            .args(["rev-parse", "--verify", "refs/tags/spec-v0"])
            .current_dir(corpus::root())
            .output()
            .unwrap();
        if tag.status.success() {
            assert_eq!(
                String::from_utf8(tag.stdout).unwrap().trim(),
                "c348fdeca3c162bdaeef0f65b4da74f5ecfb59f3"
            );
        }
    }
}
