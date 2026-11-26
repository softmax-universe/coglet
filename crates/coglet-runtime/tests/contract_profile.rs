//! Local profile shape proofs. These do not qualify surface wire 0.2 dispatch.
use coglet_runtime::ContractProfile;
use serde_json::{Value, json};

fn artifact(operation: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({"version":1,"namespace":"softmax",
        "entries":{"softmax.app":["softmax.app_control"]},
        "schemas":{"https://example.invalid/softmax/1#empty":"empty"},
        "rules":{"empty":{"op":"record","fields":[]}},
        "operations":{operation:{"entry":"softmax.app","direction":"action",
            "input":"https://example.invalid/softmax/1#empty",
            "output":"https://example.invalid/softmax/1#empty",
            "resources":["softmax.app_control"],"single_owner":true}}}))
    .unwrap()
}

#[test]
fn local_contract_profile_catalog_v1_closed() {
    use coglet_runtime::{TrustedExtensionCatalog, digest};
    let raw = artifact("softmax.app.navigate");
    assert!(TrustedExtensionCatalog::from_pinned_bytes(&raw, &digest(&raw)).is_err());
    let catalog = TrustedExtensionCatalog::from_pinned_bytes_with_profile(
        &raw,
        &digest(&raw),
        ContractProfile::Surface02,
    )
    .unwrap();
    assert_eq!(catalog.contract_profile(), ContractProfile::Surface02);
    for profile in [ContractProfile::Surface01, ContractProfile::Surface02] {
        let raw = artifact("softmax.navigate");
        TrustedExtensionCatalog::from_pinned_bytes_with_profile(&raw, &digest(&raw), profile)
            .unwrap();
    }
    for (key, value) in [
        ("contract_profile", json!("surface-0.2")),
        ("version", json!(2)),
    ] {
        let mut changed: Value = serde_json::from_slice(&raw).unwrap();
        changed[key] = value;
        let bytes = serde_json::to_vec(&changed).unwrap();
        assert!(
            TrustedExtensionCatalog::from_pinned_bytes_with_profile(
                &bytes,
                &digest(&bytes),
                ContractProfile::Surface02
            )
            .is_err()
        );
    }
    for name in [
        "other.app.navigate",
        "softmax.app.navigate.more",
        "softmax.apple.navigate",
    ] {
        let bytes = artifact(name);
        assert!(
            TrustedExtensionCatalog::from_pinned_bytes_with_profile(
                &bytes,
                &digest(&bytes),
                ContractProfile::Surface02
            )
            .is_err()
        );
    }
    assert!(
        TrustedExtensionCatalog::from_pinned_bytes_with_profile(
            &raw,
            &"0".repeat(64),
            ContractProfile::Surface02
        )
        .is_err()
    );
}

fn setup() -> Value {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap()["steps"][0]["send"]
        .clone()
}

#[test]
fn local_contract_profile_replay_pair_fenced() {
    use coglet_runtime::{DurableStore, RuntimeService, TrustedExtensionCatalog, digest};
    for with_catalog in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wal");
        let raw = artifact("softmax.navigate");
        let open = |setup: &Value, profile: ContractProfile| {
            if with_catalog {
                RuntimeService::open_with_catalog(
                    &path,
                    setup,
                    8,
                    TrustedExtensionCatalog::from_pinned_bytes_with_profile(
                        &raw,
                        &digest(&raw),
                        profile,
                    )
                    .unwrap(),
                )
            } else {
                RuntimeService::open(&path, setup, 8)
            }
        };
        let original = setup();
        drop(open(&original, ContractProfile::Surface01).unwrap());
        {
            let mut store = DurableStore::open(&path).unwrap();
            let mut state = store.state().unwrap().clone();
            assert_eq!(state["contract_profile"], "surface-0.1");
            state.as_object_mut().unwrap().remove("contract_profile");
            state["runtime"]
                .as_object_mut()
                .unwrap()
                .remove("contract_profile");
            state["router"]
                .as_object_mut()
                .unwrap()
                .remove("contract_profile");
            store.commit(&state, None).unwrap();
        }
        drop(open(&original, ContractProfile::Surface01).unwrap());
        let mut explicit = original.clone();
        explicit["contract_profile"] = json!("surface-0.1");
        drop(open(&explicit, ContractProfile::Surface01).unwrap());
        let mut opted = original.clone();
        opted["contract_profile"] = json!("surface-0.2");
        assert!(open(&opted, ContractProfile::Surface02).is_err());
        for value in [json!(""), json!("surface-0.3"), Value::Null] {
            let mut invalid = original.clone();
            invalid["contract_profile"] = value;
            assert!(open(&invalid, ContractProfile::Surface01).is_err());
        }
        {
            let mut store = DurableStore::open(&path).unwrap();
            let mut state = store.state().unwrap().clone();
            state["contract_profile"] = json!("surface-0.3");
            store.commit(&state, None).unwrap();
        }
        assert!(open(&original, ContractProfile::Surface01).is_err());

        // A new explicitly selected WAL may retain the same digest at 0.2;
        // neither removing nor changing its profile may reinterpret that WAL.
        let fresh = tempfile::tempdir().unwrap();
        let new_path = fresh.path().join("wal");
        let open_new = |s: &Value, p: ContractProfile| {
            if with_catalog {
                RuntimeService::open_with_catalog(
                    &new_path,
                    s,
                    8,
                    TrustedExtensionCatalog::from_pinned_bytes_with_profile(&raw, &digest(&raw), p)
                        .unwrap(),
                )
            } else {
                RuntimeService::open(&new_path, s, 8)
            }
        };
        drop(open_new(&opted, ContractProfile::Surface02).unwrap());
        drop(open_new(&opted, ContractProfile::Surface02).unwrap());
        assert!(open_new(&original, ContractProfile::Surface01).is_err());
        if with_catalog {
            assert!(open_new(&original, ContractProfile::Surface02).is_err());
            assert!(open_new(&opted, ContractProfile::Surface01).is_err());
        }
    }
}

#[test]
fn local_contract_profile_defaults_and_unknowns() {
    assert_eq!(ContractProfile::default(), ContractProfile::Surface01);
    for (raw, expected) in [
        ("surface-0.1", ContractProfile::Surface01),
        ("surface-0.2", ContractProfile::Surface02),
    ] {
        assert_eq!(raw.parse::<ContractProfile>().unwrap(), expected);
        assert_eq!(
            serde_json::from_value::<ContractProfile>(json!(raw)).unwrap(),
            expected
        );
        assert_eq!(serde_json::to_value(expected).unwrap(), json!(raw));
    }
    for raw in ["", "0.2", "surface-0.3", "surface-0.2\n", "Surface-0.2"] {
        assert!(raw.parse::<ContractProfile>().is_err());
        assert!(serde_json::from_value::<ContractProfile>(json!(raw)).is_err());
    }
    for name in [
        "speech.start",
        "expression.set",
        "softmax.app",
        "softmax.app_sensitive",
    ] {
        ContractProfile::Surface01.validate_name(name).unwrap();
        ContractProfile::Surface02.validate_name(name).unwrap();
    }
    for name in ["softmax.app.state", "softmax.app.view_changed"] {
        assert!(ContractProfile::Surface01.validate_name(name).is_err());
        ContractProfile::Surface02.validate_name(name).unwrap();
    }
    for name in [
        "other.app.state",
        "softmax.apple.state",
        "softmax.app.state.x",
        "softmax.app.state\n",
    ] {
        assert!(ContractProfile::Surface01.validate_name(name).is_err());
        assert!(ContractProfile::Surface02.validate_name(name).is_err());
    }
}

#[test]
fn local_contract_profiles_reject_old_public_records() {
    for (source, rules) in [
        (
            include_str!("../../../spec/fixtures/abi/cases.json"),
            &["step-record", "boundary-record", "trace"][..],
        ),
        (
            include_str!("../../../spec/fixtures/database/cases.json"),
            &["db-step-record", "db-trace"][..],
        ),
    ] {
        let cases: Vec<Value> = serde_json::from_str(source).unwrap();
        for rule in rules {
            let value = &cases
                .iter()
                .find(|case| case["type"] == *rule && case["valid"] == true)
                .unwrap()["value"];
            let qualified = if rule.starts_with("db-") {
                format!("database::{rule}")
            } else {
                format!("abi::{rule}")
            };
            for profile in [ContractProfile::Surface01, ContractProfile::Surface02] {
                profile.validate(&qualified, value).unwrap();
                for version in ["0.1", "0.2"] {
                    let mut old = value.clone();
                    old["version"] = json!(version);
                    assert!(profile.validate(&qualified, &old).is_err());
                }
            }
        }
    }
}

fn manifest(version: &str) -> Value {
    let mut m = json!({"version":version,"id":format!("coglet:{}", "a".repeat(64)),
        "subscriptions":["softmax.app.view_changed"],"required_capabilities":["softmax.app"],
        "exclusive_outputs":["softmax.app_control"],
        "budgets":{"compute":1,"compute_mode":"fuel","memory_pages":1,"wall_time_us":1,"max_events":1,"host_bytes":1}});
    if version == "1.1" || version == "0.2" {
        m["database"] = json!({"schema_min":0,"schema_max":1,"catalog_hash":"0".repeat(64)});
    }
    m
}

#[test]
fn local_contract_profile_manifest_format_independent() {
    for version in ["1.0", "1.1"] {
        let mut m = manifest(version);
        assert!(ContractProfile::Surface01.validate_manifest(&m).is_err());
        ContractProfile::Surface02.validate_manifest(&m).unwrap();
        m["subscriptions"] = json!(["speech.start"]);
        ContractProfile::Surface01.validate_manifest(&m).unwrap();
        ContractProfile::Surface02.validate_manifest(&m).unwrap();
        if version == "1.1" {
            m.as_object_mut().unwrap().remove("database");
        } else {
            m["database"] = json!({"schema_min":0,"schema_max":1,"catalog_hash":"0".repeat(64)});
        }
        assert!(ContractProfile::Surface01.validate_manifest(&m).is_err());
        assert!(ContractProfile::Surface02.validate_manifest(&m).is_err());
    }
    for version in ["0.1", "0.2", "surface-0.2", "0.3", ""] {
        let mut old = manifest(version);
        old["subscriptions"] = json!(["speech.start"]);
        for profile in [ContractProfile::Surface01, ContractProfile::Surface02] {
            assert!(profile.validate_manifest(&old).is_err());
        }
    }
}
