//! IR validator semantics against an inline schema (generated schema tested in fixtures.rs).
use ciborium::Value;
use coglet_types::validate_with_schema;

const S: &str = r#"{
 "id": {"op":"regexp","pattern":"^[0-9a-f]{4}$"},
 "u8": {"op":"range","min":0,"max":255},
 "i64": {"op":"range","min":-9223372036854775808,"max":18446744073709551615},
 "lit": {"op":"literal","value":"x"},
 "litmap": {"op":"literal","value":{"b":1,"a":[true,null]}},
 "u": {"op":"union","items":[{"op":"ref","name":"u8"},{"op":"primitive","name":"null"}]},
 "t": {"op":"primitive","name":"tstr"},
 "b": {"op":"primitive","name":"bool"},
 "arr": {"op":"array","item":{"op":"ref","name":"u8"},"min":1},
 "m": {"op":"map","item":{"op":"ref","name":"u8"}},
 "rec": {"op":"record","fields":[{"name":"a","optional":false,"type":{"op":"ref","name":"u8"}},{"name":"o","optional":true,"type":{"op":"ref","name":"t"}}]},
 "sz": {"op":"size","min":1,"max":3},
 "loop": {"op":"ref","name":"loop"},
 "bad": {"op":"frobnicate"},
 "dangling": {"op":"ref","name":"nope"}
}"#;

fn t(s: &str) -> Value {
    Value::Text(s.into())
}
fn i(n: i128) -> Value {
    Value::Integer(n.try_into().unwrap())
}
fn ok(rule: &str, v: Value) {
    validate_with_schema(S, rule, &v).unwrap_or_else(|e| panic!("{rule}: {e}"))
}
fn bad(rule: &str, v: Value) {
    assert!(
        validate_with_schema(S, rule, &v).is_err(),
        "{rule} accepted {v:?}"
    )
}

#[test]
fn regexp_full_match_no_trailing_newline() {
    ok("id", t("ab12"));
    bad("id", t("ab12\n"));
    bad("id", t("xab12"));
    bad("id", t("AB12"));
    bad("id", i(1));
}

#[test]
fn ranges_full_64_bit() {
    ok("u8", i(255));
    bad("u8", i(256));
    bad("u8", i(-1));
    ok("i64", i(u64::MAX as i128));
    ok("i64", i(i64::MIN as i128));
    bad("i64", i(i64::MIN as i128 - 1));
    bad("u8", Value::Float(1.0));
}

#[test]
fn literal_union_primitive() {
    ok("lit", t("x"));
    bad("lit", t("y"));
    ok(
        "litmap",
        Value::Map(vec![
            (t("a"), Value::Array(vec![Value::Bool(true), Value::Null])),
            (t("b"), i(1)),
        ]),
    );
    bad(
        "litmap",
        Value::Map(vec![(t("a"), Value::Array(vec![])), (t("b"), i(1))]),
    );
    ok("u", Value::Null);
    ok("u", i(3));
    bad("u", t("s"));
    ok("b", Value::Bool(false));
    bad("b", Value::Null);
}

#[test]
fn containers() {
    ok("arr", Value::Array(vec![i(1)]));
    bad("arr", Value::Array(vec![]));
    bad("arr", Value::Array(vec![i(300)]));
    ok("m", Value::Map(vec![(t("k"), i(1))]));
    bad("m", Value::Map(vec![(t("k"), t("v"))]));
    ok("rec", Value::Map(vec![(t("a"), i(1))]));
    ok("rec", Value::Map(vec![(t("o"), t("s")), (t("a"), i(1))]));
    bad("rec", Value::Map(vec![]));
    bad("rec", Value::Map(vec![(t("a"), i(1)), (t("zz"), i(1))]));
    bad("rec", Value::Map(vec![(t("a"), i(1)), (t("o"), i(1))]));
    ok("sz", t("abc"));
    bad("sz", t(""));
    bad("sz", t("é é")); // 4 bytes
    ok("sz", t("é"));
    bad("sz", t("éé"));
}

#[test]
fn fail_closed() {
    bad("bad", Value::Null);
    bad("dangling", Value::Null);
    bad("loop", Value::Null); // bounded ref recursion
    bad("missing-rule", Value::Null);
    bad("u8", Value::Bytes(vec![1])); // profile checked first
    assert!(validate_with_schema("not json", "u8", &i(1)).is_err());
    assert!(validate_with_schema("[]", "u8", &i(1)).is_err());
}
