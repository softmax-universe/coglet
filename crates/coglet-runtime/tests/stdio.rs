//! Exercise the actual executable's data-only mock seam, not an in-process helper.
use coglet_runtime::cbor;
use serde_json::Value;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

#[test]
fn cogletd_mock_stdio_computes_journal_over_native_frames() {
    let cases: Value =
        serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
    let case = cases
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "no-effects")
        .unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_cogletd"))
        .arg("--mock-stdio")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let raw = cbor(&case["steps"][0]["send"]).unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&(raw.len() as u32).to_be_bytes()).unwrap();
    stdin.write_all(&raw).unwrap();
    drop(stdin);
    let mut output = Vec::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_end(&mut output)
        .unwrap();
    assert!(child.wait().unwrap().success());
    assert!(output.len() >= 4, "missing framed ABI response");
    let n = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
    assert!(
        output.len() >= n + 8,
        "missing separate journal-hash assertion frame"
    );
    let decoded: Value = coglet_types::decode_profile(&output[4..n + 4])
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(decoded, case["steps"][1]["expect"]);
    let hash_len = u32::from_be_bytes(output[n + 4..n + 8].try_into().unwrap()) as usize;
    assert_eq!(output.len(), n + 8 + hash_len);
    let hashes: Value = coglet_types::decode_profile(&output[n + 8..])
        .unwrap()
        .deserialized()
        .unwrap();
    assert_eq!(hashes["record_hash"], decoded["record_hash"]);
}
