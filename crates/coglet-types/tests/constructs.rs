use ciborium::Value;
use coglet_types::{decode_profile, encode_profile, validate_with_schema};
use serde_json::json;

fn check(node: serde_json::Value, good: Vec<Value>, bad: Vec<Value>) {
    let schema = json!({"spec::value": node}).to_string();
    for value in good {
        validate_with_schema(&schema, "spec::value", &value).unwrap();
        let raw = encode_profile(&value).unwrap();
        let decoded = decode_profile(&raw).unwrap();
        validate_with_schema(&schema, "spec::value", &decoded).unwrap();
        assert_eq!(encode_profile(&decoded).unwrap(), raw);
    }
    for value in bad {
        assert!(validate_with_schema(&schema, "spec::value", &value).is_err());
        let decoded = decode_profile(&encode_profile(&value).unwrap()).unwrap();
        assert!(validate_with_schema(&schema, "spec::value", &decoded).is_err());
    }
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}

#[test]
fn large_portable_regexp_counts_unicode_not_bytes() {
    check(
        json!({"op":"regexp","pattern":"^[\\s\\S]{0,16384}$"}),
        vec![text(&"é\n".repeat(8192))],
        vec![text(&"é".repeat(16385))],
    );
}

#[test]
fn inclusive_array_and_empty_map_bounds() {
    check(
        json!({"op":"array","min":0,"max":2,"item":{"op":"primitive","name":"tstr"}}),
        vec![
            Value::Array(vec![]),
            Value::Array(vec![text("a"), text("b")]),
        ],
        vec![Value::Array(vec![text("a"); 3])],
    );
    check(
        json!({"op":"map","min":0,"max":0,"item":{"op":"primitive","name":"tstr"}}),
        vec![Value::Map(vec![])],
        vec![Value::Map(vec![(text("k"), text("v"))])],
    );
}

#[test]
fn keyed_maps_and_record_rest() {
    let key = json!({"op":"regexp","pattern":"[a-z]+"});
    let item = json!({"op":"primitive","name":"bool"});
    check(
        json!({"op":"map","min":1,"max":2,"key":key,"item":item}),
        vec![Value::Map(vec![(text("a"), Value::Bool(true))])],
        vec![
            Value::Map(vec![]),
            Value::Map(vec![(text("A"), Value::Bool(true))]),
            Value::Map(vec![(text("a"), Value::Null)]),
        ],
    );
    check(
        json!({"op":"record","fields":[{"name":"id","optional":false,"type":{"op":"primitive","name":"tstr"}}],
        "rest":{"min":0,"max":0,"item":{"op":"intersection","items":[{"op":"literal","value":null},{"op":"literal","value":false}]}}}),
        vec![Value::Map(vec![(text("id"), text("a"))])],
        vec![Value::Map(vec![
            (text("id"), text("a")),
            (text("extra"), Value::Null),
        ])],
    );
}

#[test]
fn intersection_default_size_and_tuple_backtracking() {
    check(
        json!({"op":"intersection","items":[{"op":"literal","value":null},{"op":"literal","value":false}]}),
        vec![],
        vec![Value::Null, Value::Bool(false)],
    );
    check(
        json!({"op":"default","item":{"op":"intersection","items":[{"op":"primitive","name":"tstr"},{"op":"size","min":1,"max":2}]},"value":"a"}),
        vec![text("é"), text("b")],
        vec![Value::Null, text("éé")],
    );
    check(
        json!({"op":"tuple","items":[{"type":{"op":"primitive","name":"tstr"},"min":0,"max":2},
        {"type":{"op":"literal","value":"end"},"min":1,"max":1}]}),
        vec![
            Value::Array(vec![text("end")]),
            Value::Array(vec![text("a"), text("end")]),
            Value::Array(vec![text("a"), text("b"), text("end")]),
        ],
        vec![
            Value::Array(vec![]),
            Value::Array(vec![text("a")]),
            Value::Array(vec![text("a"), text("b"), text("c"), text("end")]),
        ],
    );
}
