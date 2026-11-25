//! ADR-0033 local producer REPLY shapes. Validates the closed JSON Schema
//! `tests/vectors/policy_local_replies.schema.json` and the shared positive/negative
//! vectors in `tests/vectors/policy_local_replies.json` with a deliberately small,
//! strict validator (an unsupported keyword panics, so the schema cannot drift into
//! constructs this test silently ignores). The same files are consumed by the
//! black-box harness and the production consumer.
use super::*;

const SCHEMA: &str = include_str!("../tests/vectors/policy_local_replies.schema.json");
const REPLIES: &str = include_str!("../tests/vectors/policy_local_replies.json");

// ---- minimal JSON Schema (draft 2020-12 subset used by the file) --------------

/// `^` atoms `$` where an atom is a literal or `[class]`, optionally `{n}`/`{n,m}`.
fn pattern_matches(pattern: &str, text: &str) -> bool {
    struct Atom {
        set: Vec<(char, char)>,
        min: usize,
        max: usize,
    }
    let chars: Vec<char> = pattern.chars().collect();
    assert!(
        chars.first() == Some(&'^') && chars.last() == Some(&'$'),
        "unsupported pattern {pattern}"
    );
    let (mut atoms, mut i) = (Vec::new(), 1);
    while i < chars.len() - 1 {
        let mut set = Vec::new();
        if chars[i] == '[' {
            i += 1;
            while chars[i] != ']' {
                assert!(chars[i] != '^' && chars[i] != '\\', "unsupported pattern");
                if chars[i + 1] == '-' && chars[i + 2] != ']' {
                    set.push((chars[i], chars[i + 2]));
                    i += 3;
                } else {
                    set.push((chars[i], chars[i]));
                    i += 1;
                }
            }
            i += 1;
        } else {
            assert!(!"\\.*+?()|{}".contains(chars[i]), "unsupported pattern");
            set.push((chars[i], chars[i]));
            i += 1;
        }
        let (mut min, mut max) = (1, 1);
        if chars[i] == '{' {
            let end = chars[i..].iter().position(|c| *c == '}').unwrap() + i;
            let body: String = chars[i + 1..end].iter().collect();
            let mut parts = body.split(',');
            min = parts.next().unwrap().parse().unwrap();
            max = parts.next().map_or(min, |p| p.parse().unwrap());
            i = end + 1;
        }
        atoms.push(Atom { set, min, max });
    }
    fn go(atoms: &[Atom], text: &[char]) -> bool {
        let Some((atom, rest)) = atoms.split_first() else {
            return text.is_empty();
        };
        let mut n = 0;
        while n < text.len()
            && n < atom.max
            && atom
                .set
                .iter()
                .any(|(lo, hi)| (*lo..=*hi).contains(&text[n]))
        {
            n += 1;
        }
        (atom.min..=n).rev().any(|k| go(rest, &text[k..]))
    }
    go(&atoms, &text.chars().collect::<Vec<_>>())
}

fn resolve<'a>(root: &'a Value, reference: &str) -> &'a Value {
    let name = reference.strip_prefix("#/$defs/").expect("local $defs ref");
    &root["$defs"][name]
}

fn valid(root: &Value, schema: &Value, value: &Value) -> bool {
    let Some(map) = schema.as_object() else {
        panic!("schema must be an object: {schema}");
    };
    for (key, rule) in map {
        let pass = match key.as_str() {
            "$schema" | "$id" | "title" | "description" | "$defs" => true,
            "$ref" => valid(root, resolve(root, rule.as_str().unwrap()), value),
            "type" => match rule.as_str().unwrap() {
                "object" => value.is_object(),
                "array" => value.is_array(),
                "string" => value.is_string(),
                "boolean" => value.is_boolean(),
                "null" => value.is_null(),
                "integer" => value.is_u64() || value.is_i64(),
                other => panic!("unsupported type {other}"),
            },
            "const" => value == rule,
            "enum" => rule.as_array().unwrap().contains(value),
            "pattern" => value
                .as_str()
                .is_none_or(|s| pattern_matches(rule.as_str().unwrap(), s)),
            "minLength" => value
                .as_str()
                .is_none_or(|s| s.chars().count() as u64 >= rule.as_u64().unwrap()),
            "maxLength" => value
                .as_str()
                .is_none_or(|s| s.chars().count() as u64 <= rule.as_u64().unwrap()),
            "minimum" | "maximum" => match (value.is_u64() || value.is_i64(), value.as_i64()) {
                (false, _) => true,
                (true, signed) => {
                    let n: i128 = signed
                        .map(i128::from)
                        .unwrap_or_else(|| value.as_u64().unwrap().into());
                    let bound: i128 = rule.as_u64().unwrap().into();
                    if key == "minimum" {
                        n >= bound
                    } else {
                        n <= bound
                    }
                }
            },
            "properties" => value.as_object().is_none_or(|object| {
                rule.as_object()
                    .unwrap()
                    .iter()
                    .all(|(name, sub)| object.get(name).is_none_or(|v| valid(root, sub, v)))
            }),
            "required" => value.as_object().is_none_or(|object| {
                rule.as_array()
                    .unwrap()
                    .iter()
                    .all(|n| object.contains_key(n.as_str().unwrap()))
            }),
            "additionalProperties" => {
                assert_eq!(rule, &json!(false), "only closed objects are supported");
                value.as_object().is_none_or(|object| {
                    let known = map["properties"].as_object().unwrap();
                    object.keys().all(|k| known.contains_key(k))
                })
            }
            "items" => value
                .as_array()
                .is_none_or(|items| items.iter().all(|v| valid(root, rule, v))),
            "maxItems" => value
                .as_array()
                .is_none_or(|items| items.len() as u64 <= rule.as_u64().unwrap()),
            "uniqueItems" => value.as_array().is_none_or(|items| {
                !rule.as_bool().unwrap()
                    || items
                        .iter()
                        .enumerate()
                        .all(|(i, a)| !items[..i].contains(a))
            }),
            "oneOf" => {
                rule.as_array()
                    .unwrap()
                    .iter()
                    .filter(|s| valid(root, s, value))
                    .count()
                    == 1
            }
            "allOf" => rule
                .as_array()
                .unwrap()
                .iter()
                .all(|s| valid(root, s, value)),
            "if" => {
                if valid(root, rule, value) {
                    map.get("then").is_none_or(|then| valid(root, then, value))
                } else {
                    map.get("else")
                        .is_none_or(|other| valid(root, other, value))
                }
            }
            "then" | "else" => true, // evaluated with `if`
            other => panic!("unsupported schema keyword {other}"),
        };
        if !pass {
            return false;
        }
    }
    true
}

// ---- cross-field rules JSON Schema cannot express (docs: Semantic reply checks) --

fn semantic(request: &Request, reply: &Value) -> std::result::Result<(), String> {
    macro_rules! need {
        ($cond:expr, $why:expr) => {
            if !$cond {
                return Err($why.to_owned());
            }
        };
    }
    if reply["ok"] == json!(false) {
        need!(
            reply["nonce"].is_null() || reply["nonce"] == json!(request.nonce),
            "failure nonce is neither null nor the request nonce"
        );
        return Ok(());
    }
    let instance = reply["daemon_instance"].as_str().ok_or("daemon_instance")?;
    need!(
        &reply_ok(request, instance, reply["result"].clone()) == reply,
        "envelope does not echo method/nonce/principal/actor/target/ticket"
    );
    let r = &reply["result"];
    let params = &request.params;
    let pins = |r: &Value| -> std::result::Result<(), String> {
        need!(
            r["generation"].as_u64() == params.generation
                && r["commit"].as_str() == params.commit.as_deref()
                && r["executable_sha256"].as_str() == params.executable_sha256.as_deref(),
            "result pins differ from the request"
        );
        Ok(())
    };
    match request.method {
        Method::AttestApplied => {
            pins(r)?;
            need!(r["target"] == json!(request.target), "target differs");
            let statement = &r["code"]["signed_ref"]["statement"];
            need!(statement["new"] == r["commit"], "statement.new != commit");
            need!(
                statement["generation"] == r["generation"],
                "statement.generation"
            );
            need!(
                statement["coglet"] == r["target"],
                "statement.coglet != target"
            );
            need!(
                statement["hub"] == r["router"]
                    && r["code"]["signed_ref"]["signer"] == r["router"]
                    && r["initial"]["router"] == r["router"],
                "router identity differs"
            );
            let (initial, install) = (&r["initial"]["code"], &r["initial"]["install"]);
            need!(
                install["operation_id"] != initial["operation_id"],
                "install reuses code op"
            );
            let boundary = &r["boundary"];
            match boundary["kind"].as_str() {
                Some("install") => {
                    need!(
                        r["code"] == *initial,
                        "install attests the original admission"
                    );
                    need!(
                        r["receipt"]["operation_id"] == install["operation_id"]
                            && boundary["operation_id"] == install["operation_id"],
                        "install receipt/boundary op differs from the install operation"
                    );
                    need!(
                        r["receipt"]["step"] == json!(0) && r["record_offset"] == json!(0),
                        "install step"
                    );
                }
                Some("code") => {
                    need!(
                        r["receipt"]["operation_id"] == r["code"]["operation_id"]
                            && boundary["operation_id"] == r["code"]["operation_id"],
                        "code receipt/boundary op differs from the code admission"
                    );
                    need!(
                        r["code"]["operation_id"] != initial["operation_id"],
                        "code op repeats install admission"
                    );
                    need!(
                        r["generation"].as_u64()
                            > initial["signed_ref"]["statement"]["generation"].as_u64(),
                        "code generation did not advance past the original admission"
                    );
                }
                _ => return Err("boundary kind".into()),
            }
        }
        Method::ObserveHealth => {
            pins(r)?;
            need!(r["target"] == json!(request.target), "target differs");
            let c = &r["counters"];
            let n = |k: &str| c[k].as_u64().unwrap();
            let unresolved = u64::from(r["attempt_fence"] == "uncertain");
            need!(
                n("committed")
                    .checked_add(n("failed"))
                    .and_then(|x| x.checked_add(unresolved))
                    == Some(n("attempted")),
                "counters do not reconcile with the attempt fence"
            );
            need!(
                n("budget_exceeded") <= n("failed"),
                "budget_exceeded > failed"
            );
            let k = &r["clock"];
            let m = |f: &str| k[f].as_u64().unwrap();
            need!(
                m("logical_millis") >= m("runtime_boundary_millis"),
                "clock behind boundary"
            );
            need!(
                m("continuous_millis") >= m("anchor_continuous_millis"),
                "continuous before anchor"
            );
            let expected = m("anchor_logical_millis")
                + (m("continuous_millis") - m("anchor_continuous_millis"));
            need!(
                m("logical_millis").abs_diff(expected) <= 1,
                "logical clock is not the anchored mapping"
            );
            let same = r["applied_operation_id"] == r["activation"]["operation_id"];
            need!(
                same == (r["activation"]["kind"] == "code"),
                "applied operation vs activation boundary"
            );
        }
        Method::EnableOutputLease => {
            pins(r)?;
            need!(
                r["daemon_instance"] == json!(request.params.daemon_instance)
                    && r["daemon_instance"] == reply["daemon_instance"],
                "daemon_instance pin"
            );
        }
        Method::RevokeOutputLease => {
            need!(
                r["daemon_instance"] == reply["daemon_instance"],
                "daemon_instance"
            );
        }
        Method::ReconcileEmergencyStop => {}
    }
    Ok(())
}

// ---- tests ------------------------------------------------------------------

/// For integration tests: assert that a `result` produced by the REAL producer
/// seam (`policy_attest_applied`, `policy_observe_health`, `local_*`) is accepted
/// by the published closed reply schema once wrapped in the success envelope.
#[allow(dead_code)]
pub(crate) fn assert_result_conforms(method: &str, target: &str, result: &Value) {
    let schema: Value = serde_json::from_str(SCHEMA).unwrap();
    let reply = json!({"version":1,"ok":true,"method":method,"nonce":"conform-1",
        "principal":"operator","actor":"actor-1","target":target,"ticket":null,
        "daemon_instance":"1".repeat(64),"result":result});
    assert!(
        valid(&schema, &schema, &reply),
        "{method} result violates the reply schema: {result}"
    );
}

fn files() -> (Value, Value) {
    (
        serde_json::from_str(SCHEMA).unwrap(),
        serde_json::from_str(REPLIES).unwrap(),
    )
}

#[test]
fn shared_reply_vectors_hold_for_schema_and_semantics() {
    let (schema, vectors) = files();
    assert_eq!(vectors["schema"], "coglet-runtime/policy-local-replies/1");
    let (mut positive, mut schema_neg, mut semantic_neg) = (0, 0, 0);
    let mut by_method: BTreeMap<String, (u32, u32)> = BTreeMap::new();
    for v in vectors["vectors"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let request = parse_request(v["request"].as_str().unwrap().as_bytes())
            .unwrap_or_else(|e| panic!("{name}: request frame must parse: {e}"));
        let reply = &v["reply"];
        let by_schema = valid(&schema, &schema, reply);
        let by_semantics = if by_schema {
            semantic(&request, reply)
        } else {
            Err("schema".to_owned())
        };
        let entry = by_method
            .entry(v["method"].as_str().unwrap().to_owned())
            .or_default();
        if v["valid"].as_bool().unwrap() {
            positive += 1;
            entry.0 += 1;
            assert!(by_schema, "{name}: schema must accept");
            assert_eq!(by_semantics, Ok(()), "{name}");
        } else if v["layer"] == "schema" {
            schema_neg += 1;
            entry.1 += 1;
            assert!(!by_schema, "{name}: schema must reject");
        } else {
            assert_eq!(v["layer"], "semantic", "{name}");
            semantic_neg += 1;
            entry.1 += 1;
            assert!(by_schema, "{name}: semantic negatives must be schema-valid");
            assert!(by_semantics.is_err(), "{name}: semantic rule must reject");
        }
        if v["valid"].as_bool().unwrap() {
            if v["method"] == "failure" {
                assert_eq!(reply["ok"], json!(false), "{name}");
            } else {
                assert_eq!(reply["method"], v["method"], "{name}");
            }
        }
    }
    assert!(
        positive >= 20 && schema_neg >= 60 && semantic_neg >= 30,
        "{positive}/{schema_neg}/{semantic_neg}"
    );
    // Every operation (and the failure shape) has positives and negatives.
    for method in [
        "attest_applied",
        "observe_health",
        "enable_output_lease",
        "revoke_output_lease",
        "reconcile_emergency_stop",
        "failure",
    ] {
        let (p, n) = by_method.get(method).copied().unwrap_or_default();
        assert!(p >= 1 && n >= 1, "{method}: {p} positive / {n} negative");
    }
}

#[test]
fn raw_reply_lines_follow_the_frame_rules() {
    let (schema, vectors) = files();
    for v in vectors["raw"].as_array().unwrap() {
        let name = v["name"].as_str().unwrap();
        let line = v["reply"].as_str().unwrap();
        let accepted = reject_duplicate_keys(line).is_ok()
            && line.len() <= MAX_FRAME
            && serde_json::from_str::<Value>(line)
                .is_ok_and(|reply| valid(&schema, &schema, &reply));
        assert_eq!(accepted, v["valid"].as_bool().unwrap(), "{name}");
    }
    // Over-size replies are generated, not stored.
    let pad = "x".repeat(MAX_FRAME);
    let big = format!("{{\"version\":1,\"ok\":false,\"nonce\":\"{pad}\"}}");
    assert!(big.len() > MAX_FRAME);
    let reply: Value = serde_json::from_str(&big).unwrap();
    assert!(!valid(&schema, &schema, &reply));
}

#[test]
fn production_failure_replies_match_the_schema() {
    let (schema, _) = files();
    for code in [
        "invalid_argument",
        "permission_denied",
        "conflict",
        "resource_exhausted",
        "unavailable",
        "unauthenticated",
    ] {
        for nonce in [None, Some("n-1")] {
            let reply = reply_err(nonce, code);
            assert!(valid(&schema, &schema, &reply), "{code}: {reply}");
        }
    }
    // An unknown wire code is not part of the closed set.
    assert!(!valid(&schema, &schema, &reply_err(None, "internal")));
    // Success envelopes built by production code match for every method's positive result.
    let (_, vectors) = files();
    for v in vectors["vectors"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["valid"] == true && v["method"] != "failure")
    {
        let request = parse_request(v["request"].as_str().unwrap().as_bytes()).unwrap();
        let built = reply_ok(
            &request,
            v["reply"]["daemon_instance"].as_str().unwrap(),
            v["reply"]["result"].clone(),
        );
        assert_eq!(built, v["reply"], "{}", v["name"]);
        assert!(valid(&schema, &schema, &built));
    }
}

#[test]
fn schema_is_closed_everywhere() {
    // Every object schema must set additionalProperties:false and list every
    // property as required unless it is an explicitly optional one.
    fn walk(path: &str, node: &Value, optional_ok: bool) {
        match node {
            Value::Object(map) => {
                if map.get("type") == Some(&json!("object")) {
                    assert_eq!(
                        map.get("additionalProperties"),
                        Some(&json!(false)),
                        "{path} not closed"
                    );
                    let props: BTreeSet<_> = map["properties"]
                        .as_object()
                        .unwrap()
                        .keys()
                        .cloned()
                        .collect();
                    let required: BTreeSet<_> = map["required"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|v| v.as_str().unwrap().to_owned())
                        .collect();
                    assert!(required.is_subset(&props), "{path}");
                    let optional: Vec<_> = props.difference(&required).collect();
                    // Only a receipt scope may omit a field: an absent ticket is omission.
                    assert!(
                        optional.is_empty() || (optional_ok && optional == [&"ticket".to_owned()]),
                        "{path}: optional {optional:?}"
                    );
                }
                for (k, v) in map {
                    walk(
                        &format!("{path}/{k}"),
                        v,
                        optional_ok || k == "receipt_scope",
                    );
                }
            }
            Value::Array(items) => items
                .iter()
                .enumerate()
                .for_each(|(i, v)| walk(&format!("{path}/{i}"), v, optional_ok)),
            _ => {}
        }
    }
    let (schema, _) = files();
    walk("#", &schema, false);
}
