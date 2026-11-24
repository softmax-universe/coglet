use coglet_runtime::cbor;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn exchange(request: &Value, projection: Option<&str>) -> Value {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cogletd"));
    command.arg("--mock-stdio");
    if let Some(projection) = projection {
        command.args(["--trace-view", projection]);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let bytes = cbor(request).unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .unwrap();
    input.write_all(&bytes).unwrap();
    drop(input);
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert!(child.wait().unwrap().success());
    let length = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
    if request["operation"] == "abi.admit" {
        let start = 4 + length;
        let second = u32::from_be_bytes(output[start..start + 4].try_into().unwrap()) as usize;
        assert_eq!(output.len(), start + 4 + second);
    } else {
        assert_eq!(output.len(), 4 + length, "one response per operation");
    }
    coglet_types::decode_profile(&output[4..4 + length])
        .unwrap()
        .deserialized()
        .unwrap()
}

#[test]
fn empty_manifest_still_requires_identity_binding() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../conformance/abi/manifest-admission.json"
    ))
    .unwrap();
    let mut request = cases[0]["steps"][0]["send"].clone();
    request["manifest"]["required_capabilities"] = json!([]);
    request["manifest"]["exclusive_outputs"] = json!([]);
    request["grant"]["coglet"] = json!(format!("coglet:{}", "2".repeat(64)));
    assert_eq!(exchange(&request, None)["admitted"], false);
}

#[test]
fn unsupported_compute_mode_is_refused_before_instantiation() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../conformance/abi/admission.json")).unwrap();
    let mut request = cases[0]["steps"][0]["send"].clone();
    request["manifest"]["budgets"]["compute_mode"] = json!("unsupported-v999");
    request["budgets"]["compute_mode"] = json!("unsupported-v999");
    let raw = cbor(&request).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cogletd"))
        .arg("--mock-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
    input.write_all(&raw).unwrap();
    drop(input);
    assert!(!child.wait().unwrap().success());
}

#[cfg(feature = "native-engine")]
#[test]
fn admission_instantiates_without_consuming_input_or_calling_handle() {
    let cases: Vec<Value> =
        serde_json::from_str(include_str!("../../../conformance/abi/admission.json")).unwrap();
    for case in cases {
        assert_eq!(
            exchange(&case["steps"][0]["send"], None),
            case["steps"][1]["expect"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn manifest_catalog_and_grants_are_checked_over_the_wire() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../conformance/abi/manifest-admission.json"
    ))
    .unwrap();
    for case in cases {
        assert_eq!(
            exchange(&case["steps"][0]["send"], None),
            case["steps"][1]["expect"],
            "{}",
            case["name"]
        );
    }
}

#[test]
fn archive_views_are_explicit_not_selected_from_case_names() {
    for (text, view) in [
        (
            include_str!("../../../conformance/abi/provenance.json"),
            "validity",
        ),
        (
            include_str!("../../../conformance/abi/boundary-receipts.json"),
            "state",
        ),
        (
            include_str!("../../../conformance/abi/heard-archive.json"),
            "state",
        ),
    ] {
        let cases: Vec<Value> = serde_json::from_str(text).unwrap();
        for case in cases {
            let actual = exchange(&case["steps"][0]["send"], Some(view));
            let expected = &case["steps"][1];
            if let Some(exact) = expected.get("expect") {
                assert_eq!(&actual, exact, "{}", case["name"]);
            } else {
                for (key, value) in expected["match"].as_object().unwrap() {
                    assert_eq!(&actual[key], value, "{} {key}", case["name"]);
                }
            }
        }
    }
    // Selecting the view is independent of payload validity and harness env.
    assert_eq!(
        exchange(
            &json!({"operation":"abi.validate_trace","trace":{}}),
            Some("validity")
        ),
        json!({"valid":false,"dispatched":false})
    );
}
