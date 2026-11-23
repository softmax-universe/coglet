//! Recursively walks spec/fixtures/**/*.json against the generated schema.
use ciborium::Value;
use coglet_types::{decode, decode_profile, encode, validate};
use std::path::{Path, PathBuf};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut es: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    es.sort();
    for p in es {
        if p.is_dir() {
            walk(&p, out);
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p);
        }
    }
}

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2), "odd hex");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// JSON debugging view -> CBOR; no tagged conversions. Err for floats / ints outside i128.
fn json_to_cbor(v: &serde_json::Value) -> Result<Value, String> {
    use serde_json::Value as J;
    Ok(match v {
        J::Null => Value::Null,
        J::Bool(b) => Value::Bool(*b),
        J::String(s) => Value::Text(s.clone()),
        J::Number(n) => {
            let s = n.to_string();
            let i: i128 = s.parse().map_err(|_| format!("non-integer number {s}"))?;
            Value::Integer(i.try_into().map_err(|_| format!("integer overflow {s}"))?)
        }
        J::Array(a) => Value::Array(a.iter().map(json_to_cbor).collect::<Result<_, _>>()?),
        J::Object(m) => Value::Map(
            m.iter()
                .map(|(k, v)| Ok((Value::Text(k.clone()), json_to_cbor(v)?)))
                .collect::<Result<_, String>>()?,
        ),
    })
}

#[test]
fn json_conversion_detects_overflow() {
    let big: serde_json::Value = serde_json::from_str("18446744073709551616").unwrap();
    // ciborium::Integer cannot even represent 2^64: conversion itself must fail closed.
    assert!(json_to_cbor(&big).is_err());
    let fl: serde_json::Value = serde_json::from_str("1.5").unwrap();
    assert!(json_to_cbor(&fl).is_err());
    let max: serde_json::Value = serde_json::from_str("18446744073709551615").unwrap();
    assert!(validate_any(&json_to_cbor(&max).unwrap()).is_ok());
}

fn validate_any(v: &Value) -> Result<(), String> {
    coglet_types::encode_profile(v).map(|_| ())
}

#[test]
fn spec_fixtures() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures");
    let mut files = vec![];
    walk(&root, &mut files);
    assert!(!files.is_empty(), "no fixtures found");
    let (mut sem, mut valid, mut wire) = (0, 0, 0);
    for f in &files {
        let j: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        let cases = j
            .as_array()
            .unwrap_or_else(|| panic!("{f:?}: not an array"));
        for c in cases {
            let name = format!("{}::{}", f.display(), c["name"].as_str().expect("name"));
            if let Some(ty) = c.get("type").and_then(|t| t.as_str()) {
                sem += 1;
                let is_valid = c["valid"].as_bool().expect("valid");
                let conv = json_to_cbor(&c["value"]);
                if is_valid {
                    valid += 1;
                    let v = conv.unwrap_or_else(|e| panic!("{name}: {e}"));
                    validate(ty, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
                    let raw = encode(ty, &v).unwrap_or_else(|e| panic!("{name}: {e}"));
                    let h = c["cbor_hex"]
                        .as_str()
                        .unwrap_or_else(|| panic!("{name}: valid case lacks cbor_hex"));
                    assert_eq!(raw, hex(h), "{name}: encode != golden");
                    let d = decode(ty, &hex(h)).unwrap_or_else(|e| panic!("{name}: {e}"));
                    assert_eq!(encode(ty, &d).unwrap(), raw, "{name}: reencode");
                    validate(ty, &d).unwrap();
                } else if let Ok(v) = conv {
                    assert!(validate(ty, &v).is_err(), "{name}: invalid accepted");
                    if let Some(h) = c.get("cbor_hex").and_then(|h| h.as_str()) {
                        assert!(
                            decode(ty, &hex(h)).is_err(),
                            "{name}: invalid golden decoded"
                        );
                    }
                }
            } else {
                wire += 1;
                let h = c["cbor_hex"]
                    .as_str()
                    .unwrap_or_else(|| panic!("{name}: no cbor_hex"));
                assert!(
                    decode_profile(&hex(h)).is_err(),
                    "{name}: wire-invalid accepted"
                );
            }
        }
    }
    eprintln!(
        "files={} semantic={sem} valid={valid} wire_invalid={wire}",
        files.len()
    );
    assert!(sem > 0 && wire > 0);
}

/// Schema-independent: every valid golden is deterministic profile CBOR matching its JSON view.
#[test]
fn goldens_are_profile_deterministic() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/fixtures");
    let mut files = vec![];
    walk(&root, &mut files);
    let mut n = 0;
    for f in &files {
        let j: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(f).unwrap()).unwrap();
        for c in j.as_array().unwrap() {
            if c.get("valid").and_then(|v| v.as_bool()) != Some(true) {
                continue;
            }
            let name = c["name"].as_str().unwrap();
            let v = json_to_cbor(&c["value"]).unwrap_or_else(|e| panic!("{name}: {e}"));
            let golden = hex(c["cbor_hex"].as_str().unwrap());
            assert_eq!(coglet_types::encode_profile(&v).unwrap(), golden, "{name}");
            let d = decode_profile(&golden).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(coglet_types::encode_profile(&d).unwrap(), golden, "{name}");
            n += 1;
        }
    }
    eprintln!("goldens checked: {n}");
    assert!(n > 0);
}
