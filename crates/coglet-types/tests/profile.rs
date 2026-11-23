//! Profile-only (shape independent) tests: strict decode, deterministic encode.
use ciborium::Value;
use coglet_types::{decode_profile, encode_profile};

fn hex(s: &str) -> Vec<u8> {
    assert!(s.len().is_multiple_of(2));
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn accepts_scalars_and_roundtrips() {
    for h in [
        "00",
        "17",
        "1818",
        "1b ffffffffffffffff",
        "20",
        "3b 7fffffffffffffff",
        "f4",
        "f5",
        "f6",
        "60",
        "80",
        "a0",
        "6161",
    ] {
        let raw = hex(&h.replace(' ', ""));
        let v = decode_profile(&raw).unwrap_or_else(|e| panic!("{h}: {e}"));
        assert_eq!(encode_profile(&v).unwrap(), raw, "{h}");
    }
}

#[test]
fn rejects_non_profile_wire() {
    for (n, h) in [
        ("non-shortest", "1800"),
        ("non-shortest16", "190001"),
        ("indef-array", "9f01ff"),
        ("indef-map", "bf616101ff"),
        ("indef-text", "7f6161ff"),
        ("dup", "a2616101616102"),
        ("trailing", "0001"),
        ("bytes", "4100"),
        ("tag", "c001"),
        ("half", "f93c00"),
        ("float", "fa3f800000"),
        ("double", "fb3ff0000000000000"),
        ("undefined", "f7"),
        ("simple", "f0"),
        ("simple-ext", "f820"),
        ("break", "ff"),
        ("neg-below-i64", "3b8000000000000000"),
        ("nontext-key", "a10101"),
        ("order", "a2616201616101"),
        ("bad-utf8", "62c328"),
        ("truncated", "8201"),
        ("empty", ""),
        ("huge-len", "9bffffffffffffffff"),
        ("huge-text", "7bffffffffffffffff"),
        ("reserved-ai", "1c"),
    ] {
        assert!(decode_profile(&hex(h)).is_err(), "{n} accepted");
    }
}

#[test]
fn bounded_nesting() {
    let mut deep = vec![0x81u8; 10_000];
    deep.push(0x00);
    assert!(decode_profile(&deep).is_err());
    let mut v = Value::Null;
    for _ in 0..10_000 {
        v = Value::Array(vec![v]);
    }
    assert!(encode_profile(&v).is_err());
    std::mem::forget(v); // avoid recursive drop overflow
    let mut ok = vec![0x81u8; 32];
    ok.push(0x00);
    assert!(decode_profile(&ok).is_ok());
}

#[test]
fn encode_rejects_non_profile_values() {
    assert!(encode_profile(&Value::Bytes(vec![1])).is_err());
    assert!(encode_profile(&Value::Float(1.0)).is_err());
    assert!(encode_profile(&Value::Tag(1, Box::new(Value::Null))).is_err());
    assert!(encode_profile(&Value::Map(vec![(Value::Integer(1.into()), Value::Null)])).is_err());
    assert!(
        encode_profile(&Value::Map(vec![
            (text("a"), Value::Null),
            (text("a"), Value::Null)
        ]))
        .is_err()
    );

    assert!(encode_profile(&Value::Integer((i64::MIN as i128 - 1).try_into().unwrap())).is_err());
    assert!(encode_profile(&Value::Integer(i64::MIN.into())).is_ok());
    assert!(encode_profile(&Value::Integer(u64::MAX.into())).is_ok());
}

#[test]
fn encode_sorts_keys_by_encoded_bytes() {
    let v = Value::Map(vec![
        (text("b"), Value::from(1)),
        (text("aa"), Value::from(2)),
        (text("z"), Value::from(3)),
    ]);
    assert_eq!(
        hex_of(&encode_profile(&v).unwrap()),
        "a3616201617a0362616102"
    );
    // bytewise (not length-first): "a"=6161 < "b"=6162 < "ab"=626162
    let v = Value::Map(vec![
        (text("b"), Value::from(0)),
        (text("ab"), Value::from(0)),
        (text("a"), Value::from(0)),
    ]);
    assert_eq!(
        hex_of(&encode_profile(&v).unwrap()),
        "a3616100616200626162".to_string() + "00"
    );
}

fn hex_of(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn nested_maps_canonicalized_on_encode() {
    let inner = Value::Map(vec![(text("z"), Value::Null), (text("a"), Value::Null)]);
    let v = Value::Array(vec![inner]);
    let raw = encode_profile(&v).unwrap();
    assert_eq!(hex_of(&raw), "81a26161f6617af6");
    assert!(decode_profile(&raw).is_ok());
}
