use coglet_runtime::{ContractProfile, TrustedExtensionCatalog};
use sha2::{Digest, Sha256};

fn pin(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn main() {
    let raw = include_bytes!("../../catalog.v1.json");
    let digest = include_str!("../../catalog.v1.sha256").trim();
    let catalog = TrustedExtensionCatalog::from_pinned_bytes_with_profile(
        raw,
        digest,
        ContractProfile::Surface02,
    )
    .expect("real runtime loader");
    assert_eq!(catalog.contract_profile(), ContractProfile::Surface02);
    assert!(TrustedExtensionCatalog::from_pinned_bytes(raw, digest).is_err());
    assert!(
        TrustedExtensionCatalog::from_pinned_bytes_with_profile(
            raw,
            &"0".repeat(64),
            ContractProfile::Surface02
        )
        .is_err()
    );
    let mut artifact: serde_json::Value = serde_json::from_slice(raw).unwrap();
    for invalid in [
        serde_json::json!({"op":"ref","name":"missing"}),
        serde_json::json!({"op":"map","item":{"op":"primitive","name":"tstr"}}),
    ] {
        artifact["rules"]["unused-malformed"] = invalid;
        let bytes = serde_json::to_vec(&artifact).unwrap();
        assert!(
            TrustedExtensionCatalog::from_pinned_bytes_with_profile(
                &bytes,
                &pin(&bytes),
                ContractProfile::Surface02
            )
            .is_err()
        );
    }
    let fixtures: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures.json")).unwrap();
    let ir = include_str!("../../softmax-app.ir.json");
    let mut count = 0;
    for case in fixtures.as_array().unwrap() {
        let hex = case["cbor_hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        let result = coglet_types::decode_profile(&bytes).and_then(|v| {
            coglet_types::validate_with_schema(ir, case["rule"].as_str().unwrap(), &v)
        });
        assert_eq!(
            result.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {:?}",
            case["name"],
            result
        );
        count += 1;
    }
    println!(
        "real Rust catalog loader: surface-0.2 accepts; default/wrong-pin/malformed-IR reject; {count} selected-rule fixture checks pass"
    );
}
