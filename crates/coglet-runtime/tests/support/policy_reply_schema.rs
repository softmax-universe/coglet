// Included by policy_producers.rs. Verbatim copy of the minimal strict validator
// used by src/policy_local_reply_tests.rs (integration tests cannot reach that
// private module). It validates the SAME shared closed schema file; an
// unsupported keyword panics instead of being ignored.
mod reply_schema {
    use serde_json::{Value, json};
    pub const SCHEMA: &str = include_str!("../vectors/policy_local_replies.schema.json");
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

pub fn valid(root: &Value, schema: &Value, value: &Value) -> bool {
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
}
