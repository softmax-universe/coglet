//! Deterministic CBOR (RFC 8949 §4.2.1 core deterministic, spec/conventions.md profile)
//! runtime plus a fail-closed validator for the generated schema IR.

mod generated;
pub use generated::*;

pub use ciborium::Value;
use regex::Regex;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value as Json;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};

/// Maximum nesting depth of arrays/maps in a profile value.
pub const MAX_DEPTH: usize = 64;
/// Maximum depth of rule expression / `ref` recursion while validating.
const MAX_EXPR_DEPTH: usize = 128;

// ---------------------------------------------------------------- profile

fn check_profile(v: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_DEPTH {
        return Err("nesting too deep".into());
    }
    match v {
        Value::Integer(i) => {
            let i = i128::from(*i);
            if i < i64::MIN as i128 || i > u64::MAX as i128 {
                Err(format!("integer {i} outside profile range"))
            } else {
                Ok(())
            }
        }
        Value::Text(_) | Value::Bool(_) | Value::Null => Ok(()),
        Value::Array(a) => a.iter().try_for_each(|x| check_profile(x, depth + 1)),
        Value::Map(m) => {
            let mut seen = BTreeSet::new();
            for (k, x) in m {
                let Value::Text(k) = k else {
                    return Err("map key is not text".into());
                };
                if !seen.insert(k.as_str()) {
                    return Err(format!("duplicate map key {k:?}"));
                }
                check_profile(x, depth + 1)?;
            }
            Ok(())
        }
        Value::Bytes(_) => Err("bytes (major 2) forbidden".into()),
        Value::Tag(..) => Err("tags (major 6) forbidden".into()),
        Value::Float(_) => Err("floats forbidden".into()),
        _ => Err("unsupported CBOR value".into()),
    }
}

fn put_head(out: &mut Vec<u8>, major: u8, n: u64) {
    let m = major << 5;
    match n {
        0..=23 => out.push(m | n as u8),
        24..=0xff => out.extend([m | 24, n as u8]),
        0x100..=0xffff => {
            out.push(m | 25);
            out.extend((n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(m | 26);
            out.extend((n as u32).to_be_bytes());
        }
        _ => {
            out.push(m | 27);
            out.extend(n.to_be_bytes());
        }
    }
}

/// Encode an already-checked value, sorting map keys by encoded bytes.
fn put_value(out: &mut Vec<u8>, v: &Value) {
    match v {
        Value::Integer(i) => {
            let i = i128::from(*i);
            if i >= 0 {
                put_head(out, 0, i as u64);
            } else {
                put_head(out, 1, (-1 - i) as u64);
            }
        }
        Value::Text(s) => {
            put_head(out, 3, s.len() as u64);
            out.extend(s.as_bytes());
        }
        Value::Bool(b) => out.push(if *b { 0xf5 } else { 0xf4 }),
        Value::Null => out.push(0xf6),
        Value::Array(a) => {
            put_head(out, 4, a.len() as u64);
            a.iter().for_each(|x| put_value(out, x));
        }
        Value::Map(m) => {
            let mut ents: Vec<(Vec<u8>, &Value)> = m
                .iter()
                .map(|(k, x)| {
                    let mut kb = Vec::new();
                    put_value(&mut kb, k);
                    (kb, x)
                })
                .collect();
            ents.sort_by(|a, b| a.0.cmp(&b.0));
            put_head(out, 5, ents.len() as u64);
            for (kb, x) in ents {
                out.extend(kb);
                put_value(out, x);
            }
        }
        _ => unreachable!("checked by check_profile"),
    }
}

/// Encode a profile value as deterministic CBOR (rejects non-profile values).
pub fn encode_profile(v: &Value) -> Result<Vec<u8>, String> {
    check_profile(v, 0)?;
    let mut out = Vec::new();
    put_value(&mut out, v);
    Ok(out)
}

struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8], String> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|e| *e <= self.b.len())
            .ok_or("truncated input")?;
        let s = &self.b[self.pos..end];
        self.pos = end;
        Ok(s)
    }

    /// Returns (major, argument) enforcing shortest form and definite length.
    fn head(&mut self) -> Result<(u8, u64), String> {
        let ib = self.take(1)?[0];
        let (major, ai) = (ib >> 5, ib & 31);
        let n = match ai {
            0..=23 => return Ok((major, ai as u64)),
            24 => self.take(1)?[0] as u64,
            25 => u16::from_be_bytes(self.take(2)?.try_into().unwrap()) as u64,
            26 => u32::from_be_bytes(self.take(4)?.try_into().unwrap()) as u64,
            27 => u64::from_be_bytes(self.take(8)?.try_into().unwrap()),
            31 => return Err("indefinite length / break forbidden".into()),
            _ => return Err("reserved additional info".into()),
        };
        let min = match ai {
            24 => 24,
            25 => 0x100,
            26 => 0x1_0000,
            _ => 0x1_0000_0000,
        };
        if n < min {
            return Err("non-shortest integer/length encoding".into());
        }
        Ok((major, n))
    }

    fn item(&mut self, depth: usize) -> Result<Value, String> {
        if depth > MAX_DEPTH {
            return Err("nesting too deep".into());
        }
        let (major, n) = self.head()?;
        Ok(match major {
            0 => Value::Integer(n.into()),
            1 => {
                if n > i64::MAX as u64 {
                    return Err("negative integer below i64::MIN".into());
                }
                Value::Integer((-1 - n as i64).into())
            }
            2 => return Err("bytes (major 2) forbidden".into()),
            3 => {
                let len = usize::try_from(n).map_err(|_| "length overflow")?;
                let s = std::str::from_utf8(self.take(len)?).map_err(|_| "invalid UTF-8")?;
                Value::Text(s.to_owned())
            }
            4 => {
                // each element needs >= 1 byte: bound declared length by remaining input
                if n > (self.b.len() - self.pos) as u64 {
                    return Err("truncated input".into());
                }
                let mut a = Vec::with_capacity(n as usize);
                for _ in 0..n {
                    a.push(self.item(depth + 1)?);
                }
                Value::Array(a)
            }
            5 => {
                if n > ((self.b.len() - self.pos) / 2) as u64 {
                    return Err("truncated input".into());
                }
                let mut m: Vec<(Value, Value)> = Vec::with_capacity(n as usize);
                let mut prev: Option<(usize, usize)> = None;
                for _ in 0..n {
                    let start = self.pos;
                    let (km, _) = self.head().map_err(|e| format!("map key: {e}"))?;
                    if km != 3 {
                        return Err("map key is not text".into());
                    }
                    self.pos = start;
                    let k = self.item(depth + 1)?;
                    let kr = (start, self.pos);
                    if let Some(p) = prev {
                        match self.b[p.0..p.1].cmp(&self.b[kr.0..kr.1]) {
                            std::cmp::Ordering::Less => {}
                            std::cmp::Ordering::Equal => return Err("duplicate map key".into()),
                            std::cmp::Ordering::Greater => {
                                return Err("map keys not in deterministic order".into());
                            }
                        }
                    }
                    prev = Some(kr);
                    m.push((k, self.item(depth + 1)?));
                }
                Value::Map(m)
            }
            6 => return Err("tags (major 6) forbidden".into()),
            _ => match n {
                20 => Value::Bool(false),
                21 => Value::Bool(true),
                22 => Value::Null,
                _ => return Err("only false/true/null allowed in major 7".into()),
            },
        })
    }
}

/// Decode exactly one deterministic profile item; anything else is an error.
pub fn decode_profile(raw: &[u8]) -> Result<Value, String> {
    let mut r = Reader { b: raw, pos: 0 };
    let v = r.item(0)?;
    if r.pos != raw.len() {
        return Err("trailing data".into());
    }
    // Belt and braces: canonical re-encode must be byte-identical.
    if encode_profile(&v)? != raw {
        return Err("not deterministically encoded".into());
    }
    Ok(v)
}

// ---------------------------------------------------------------- IR

enum Schema {
    Eager(BTreeMap<String, Json>),
    Embedded(Vec<OnceLock<Result<Json, String>>>),
}

impl Schema {
    fn embedded() -> Self {
        Self::Embedded(
            (0..generated::SCHEMA_RULES.len())
                .map(|_| OnceLock::new())
                .collect(),
        )
    }

    fn rule(&self, name: &str) -> Result<&Json, String> {
        match self {
            Self::Eager(rules) => rules
                .get(name)
                .ok_or_else(|| format!("unknown rule `{name}`")),
            Self::Embedded(cache) => {
                let index = generated::SCHEMA_RULES
                    .binary_search_by_key(&name, |(key, _)| *key)
                    .map_err(|_| format!("unknown rule `{name}`"))?;
                cache[index]
                    .get_or_init(|| {
                        serde_json::from_str(generated::SCHEMA_RULES[index].1)
                            .map_err(|e| format!("bad schema IR: {e}"))
                    })
                    .as_ref()
                    .map_err(Clone::clone)
            }
        }
    }
}

fn parse_schema(src: &str) -> Result<Schema, String> {
    let j: Json = serde_json::from_str(src).map_err(|e| format!("bad schema IR: {e}"))?;
    let Json::Object(m) = j else {
        return Err("schema IR must be an object of rule -> expr".into());
    };
    Ok(Schema::Eager(m.into_iter().collect()))
}

fn embedded() -> Result<&'static Schema, String> {
    static S: OnceLock<Schema> = OnceLock::new();
    Ok(S.get_or_init(Schema::embedded))
}

fn regex_for(pattern: &str) -> Result<Regex, String> {
    static C: OnceLock<Mutex<BTreeMap<String, Regex>>> = OnceLock::new();
    let mut c = C
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "poisoned")?;
    if let Some(r) = c.get(pattern) {
        return Ok(r.clone());
    }
    // CDDL regexps match the whole string; `\z` (not `$`) so a trailing newline fails.
    // The portable any-character class must not expand Unicode whitespace
    // tables thousands of times for bounded text fields. This is equivalent
    // for every Unicode scalar, including newline, not an ASCII approximation.
    let portable = pattern.replace(r"[\s\S]", "(?s:.)");
    let r = Regex::new(&format!(r"\A(?:{portable})\z"))
        .map_err(|e| format!("bad regexp {pattern:?}: {e}"))?;
    c.insert(pattern.to_owned(), r.clone());
    Ok(r)
}

fn field<'a>(e: &'a Json, k: &str) -> Result<&'a Json, String> {
    e.get(k).ok_or_else(|| format!("IR missing `{k}`"))
}

fn int_of(j: &Json) -> Result<i128, String> {
    if let Some(i) = j.as_i64() {
        Ok(i as i128)
    } else if let Some(u) = j.as_u64() {
        Ok(u as i128)
    } else {
        Err(format!("IR integer expected, got {j}"))
    }
}

fn only_keys(e: &Json, allowed: &[&str]) -> Result<(), String> {
    for k in e.as_object().ok_or("IR expr must be an object")?.keys() {
        if !allowed.contains(&k.as_str()) {
            return Err(format!("unknown IR key `{k}`"));
        }
    }
    Ok(())
}

fn json_matches(lit: &Json, v: &Value) -> Result<bool, String> {
    Ok(match (lit, v) {
        (Json::Null, Value::Null) => true,
        (Json::Bool(a), Value::Bool(b)) => a == b,
        (Json::String(a), Value::Text(b)) => a == b,
        (Json::Number(_), Value::Integer(b)) => int_of(lit)? == i128::from(*b),
        (Json::Array(a), Value::Array(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (x, y) in a.iter().zip(b) {
                if !json_matches(x, y)? {
                    return Ok(false);
                }
            }
            true
        }
        (Json::Object(a), Value::Map(b)) => {
            if a.len() != b.len() {
                return Ok(false);
            }
            for (k, x) in a {
                let Some((_, y)) = b
                    .iter()
                    .find(|(bk, _)| matches!(bk, Value::Text(t) if t == k))
                else {
                    return Ok(false);
                };
                if !json_matches(x, y)? {
                    return Ok(false);
                }
            }
            true
        }
        _ => false,
    })
}

fn eval(s: &Schema, e: &Json, v: &Value, depth: usize) -> Result<(), String> {
    if depth > MAX_EXPR_DEPTH {
        return Err("rule recursion too deep".into());
    }
    let op = field(e, "op")?.as_str().ok_or("IR `op` must be a string")?;
    match op {
        "ref" => {
            only_keys(e, &["op", "name"])?;
            let n = field(e, "name")?.as_str().ok_or("ref name")?;
            let t = s.rule(n)?;
            eval(s, t, v, depth + 1).map_err(|m| format!("{n}: {m}"))
        }
        "range" => {
            only_keys(e, &["op", "min", "max"])?;
            let (lo, hi) = (int_of(field(e, "min")?)?, int_of(field(e, "max")?)?);
            match v {
                Value::Integer(i) if (lo..=hi).contains(&i128::from(*i)) => Ok(()),
                Value::Integer(i) => Err(format!("{} outside {lo}..{hi}", i128::from(*i))),
                _ => Err("expected integer".into()),
            }
        }
        "literal" => {
            only_keys(e, &["op", "value"])?;
            if json_matches(field(e, "value")?, v)? {
                Ok(())
            } else {
                Err("literal mismatch".into())
            }
        }
        "union" => {
            only_keys(e, &["op", "items"])?;
            let items = field(e, "items")?.as_array().ok_or("union items")?;
            if items.is_empty() {
                return Err("empty union".into());
            }
            let mut errs = Vec::new();
            for it in items {
                match eval(s, it, v, depth + 1) {
                    Ok(()) => return Ok(()),
                    Err(m) => errs.push(m),
                }
            }
            Err(format!(
                "no union alternative matched ({})",
                errs.join("; ")
            ))
        }
        "intersection" => {
            only_keys(e, &["op", "items"])?;
            let items = field(e, "items")?.as_array().ok_or("intersection items")?;
            if items.is_empty() {
                return Err("empty intersection".into());
            }
            items.iter().try_for_each(|x| eval(s, x, v, depth + 1))
        }
        "default" => {
            only_keys(e, &["op", "item", "value"])?;
            field(e, "value")?;
            eval(s, field(e, "item")?, v, depth + 1)
        }
        "tuple" => {
            only_keys(e, &["op", "items"])?;
            let Value::Array(a) = v else {
                return Err("expected tuple array".into());
            };
            let mut positions = BTreeSet::from([0usize]);
            for item in field(e, "items")?.as_array().ok_or("tuple items")? {
                only_keys(item, &["type", "min", "max"])?;
                let min = int_of(field(item, "min")?)?;
                let max = item
                    .get("max")
                    .map(int_of)
                    .transpose()?
                    .unwrap_or(a.len() as i128);
                if min < 0 || max < min {
                    return Err("invalid tuple bounds".into());
                }
                let mut next = BTreeSet::new();
                for start in positions {
                    let limit = max.min((a.len() - start) as i128) as usize;
                    for count in 0..=limit {
                        if count as i128 >= min {
                            next.insert(start + count);
                        }
                        if count == limit
                            || eval(s, field(item, "type")?, &a[start + count], depth + 1).is_err()
                        {
                            break;
                        }
                    }
                }
                positions = next;
            }
            if positions.contains(&a.len()) {
                Ok(())
            } else {
                Err("tuple mismatch".into())
            }
        }
        "primitive" => {
            only_keys(e, &["op", "name"])?;
            match (field(e, "name")?.as_str().ok_or("primitive name")?, v) {
                ("tstr", Value::Text(_)) | ("bool", Value::Bool(_)) | ("null", Value::Null) => {
                    Ok(())
                }
                ("tstr" | "bool" | "null", _) => Err("primitive type mismatch".into()),
                (o, _) => Err(format!("unknown primitive `{o}`")),
            }
        }
        "array" => {
            only_keys(e, &["op", "item", "min", "max"])?;
            let min = int_of(field(e, "min")?)?;
            let Value::Array(a) = v else {
                return Err("expected array".into());
            };
            if (a.len() as i128) < min {
                return Err(format!("array shorter than {min}"));
            }
            if let Some(mx) = e.get("max")
                && a.len() as i128 > int_of(mx)?
            {
                return Err("array too long".into());
            }
            let item = field(e, "item")?;
            a.iter().try_for_each(|x| eval(s, item, x, depth + 1))
        }
        "map" => {
            only_keys(e, &["op", "item", "key", "min", "max"])?;
            let Value::Map(m) = v else {
                return Err("expected map".into());
            };
            let min = e.get("min").map(int_of).transpose()?.unwrap_or(0);
            let max = e
                .get("max")
                .map(int_of)
                .transpose()?
                .unwrap_or(m.len() as i128);
            if (m.len() as i128) < min || (m.len() as i128) > max {
                return Err("map occurrence bounds".into());
            }
            let item = field(e, "item")?;
            m.iter().try_for_each(|(k, x)| {
                if let Some(key) = e.get("key") {
                    eval(s, key, k, depth + 1)?;
                }
                eval(s, item, x, depth + 1)
            })
        }
        "record" => {
            only_keys(e, &["op", "fields", "rest"])?;
            let fields = field(e, "fields")?.as_array().ok_or("record fields")?;
            let Value::Map(m) = v else {
                return Err("expected map".into());
            };
            let mut known = BTreeSet::new();
            for f in fields {
                only_keys(f, &["name", "optional", "type"])?;
                let name = field(f, "name")?.as_str().ok_or("field name")?;
                let optional = field(f, "optional")?.as_bool().ok_or("field optional")?;
                if !known.insert(name) {
                    return Err(format!("IR duplicate field `{name}`"));
                }
                match m
                    .iter()
                    .find(|(k, _)| matches!(k, Value::Text(t) if t == name))
                {
                    Some((_, x)) => eval(s, field(f, "type")?, x, depth + 1)
                        .map_err(|e| format!("field `{name}`: {e}"))?,
                    None if optional => {}
                    None => return Err(format!("missing field `{name}`")),
                }
            }
            let mut extras = 0;
            for (k, x) in m {
                if !matches!(k, Value::Text(t) if known.contains(t.as_str())) {
                    let rest = e.get("rest").ok_or("unknown field")?;
                    only_keys(rest, &["item", "key", "min", "max"])?;
                    if let Some(key) = rest.get("key") {
                        eval(s, key, k, depth + 1)?;
                    }
                    eval(s, field(rest, "item")?, x, depth + 1)?;
                    extras += 1;
                }
            }
            if let Some(rest) = e.get("rest") {
                let min = int_of(field(rest, "min")?)?;
                let max = rest.get("max").map(int_of).transpose()?.unwrap_or(extras);
                if extras < min || extras > max {
                    return Err("record rest occurrence bounds".into());
                }
            }
            Ok(())
        }
        "regexp" => {
            only_keys(e, &["op", "pattern"])?;
            let pattern = field(e, "pattern")?.as_str().ok_or("pattern")?;
            // Common portable bounded text pattern: count Unicode scalars
            // directly, avoiding a multi-megabyte expanded regex automaton.
            if let Some(bounds) = pattern
                .strip_prefix(r"^[\s\S]{")
                .and_then(|p| p.strip_suffix("}$"))
                && let Some((lo, hi)) = bounds.split_once(',')
                && let (Ok(lo), Ok(hi)) = (lo.parse::<usize>(), hi.parse::<usize>())
            {
                return match v {
                    Value::Text(t) if (lo..=hi).contains(&t.chars().count()) => Ok(()),
                    _ => Err("regexp bounded text mismatch".into()),
                };
            }
            if pattern == "^([0-9a-f]{2})*$" {
                // Exactly the regex's language: even-length lowercase ASCII
                // hex, empty allowed; `$` is end-of-text (no trailing \n).
                return match v {
                    Value::Text(t) if is_lower_hex_pairs(t) => Ok(()),
                    Value::Text(_) => Err("regexp mismatch".into()),
                    _ => Err("expected text".into()),
                };
            }
            let re = regex_for(pattern)?;
            match v {
                Value::Text(t) if re.is_match(t) => Ok(()),
                Value::Text(_) => Err("regexp mismatch".into()),
                _ => Err("expected text".into()),
            }
        }
        "size" => {
            only_keys(e, &["op", "min", "max"])?;
            let (lo, hi) = (int_of(field(e, "min")?)?, int_of(field(e, "max")?)?);
            match v {
                Value::Text(t) if (lo..=hi).contains(&(t.len() as i128)) => Ok(()),
                Value::Text(t) => Err(format!("text length {} bytes outside {lo}..{hi}", t.len())),
                _ => Err("expected text".into()),
            }
        }
        o => Err(format!("unknown IR op `{o}`")),
    }
}

fn validate_in(s: &Schema, rule: &str, value: &Value) -> Result<(), String> {
    check_profile(value, 0)?;
    let e = s.rule(rule)?;
    eval(s, e, value, 0)
}

// ---------------------------------------------------------------- public API

/// Validate against an explicit schema IR JSON string (profile checked first).
pub fn validate_with_schema(schema_json: &str, rule: &str, value: &Value) -> Result<(), String> {
    validate_in(&parse_schema(schema_json)?, rule, value)
}

/// Validate `value` against generated rule `rule` (profile and shape).
pub fn validate(rule: &str, value: &Value) -> Result<(), String> {
    validate_in(embedded()?, rule, value)
}

/// Validate then encode as deterministic CBOR.
pub fn encode(rule: &str, value: &Value) -> Result<Vec<u8>, String> {
    validate(rule, value)?;
    encode_profile(value)
}

/// Strictly decode, then validate against `rule`.
pub fn decode(rule: &str, raw: &[u8]) -> Result<Value, String> {
    let v = decode_profile(raw)?;
    validate(rule, &v)?;
    Ok(v)
}

pub fn encode_typed<T: Serialize>(rule: &str, value: &T) -> Result<Vec<u8>, String> {
    let v = Value::serialized(value).map_err(|e| e.to_string())?;
    encode(rule, &v)
}

pub fn decode_typed<T: DeserializeOwned>(rule: &str, raw: &[u8]) -> Result<T, String> {
    decode(rule, raw)?.deserialized().map_err(|e| e.to_string())
}

/// Fast path for the portable `^([0-9a-f]{2})*$` regexp: even-length, lowercase
/// ASCII hex only. Non-ASCII bytes can never match `[0-9a-f]`.
fn is_lower_hex_pairs(t: &str) -> bool {
    t.len().is_multiple_of(2) && t.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

#[cfg(test)]
mod hex_pattern_tests {
    use super::*;

    const FAST: &str = "^([0-9a-f]{2})*$";
    // Same language, different spelling: forces the general regex engine.
    const REGEX: &str = "^(?:[0-9a-f]{2})*$";

    fn check(pattern: &str, v: &Value) -> Result<(), String> {
        let ir = serde_json::json!({"r": {"op": "regexp", "pattern": pattern}});
        validate_in(&parse_schema(&ir.to_string()).unwrap(), "r", v)
    }

    fn text(s: &str) -> Value {
        Value::Text(s.to_string())
    }

    #[test]
    fn fast_path_matches_regex_engine_on_exhaustive_short_and_adversarial_text() {
        // Every one- and two-byte ASCII string, plus all-ASCII 3-/4-char
        // strings drawn from the boundary characters of each class.
        let mut cases: Vec<String> = vec![String::new()];
        let alphabet: Vec<char> = (0u8..128).map(char::from).collect();
        for &a in &alphabet {
            cases.push(a.to_string());
            for &b in &alphabet {
                cases.push(format!("{a}{b}"));
            }
        }
        let edge = ['0', '9', 'a', 'f', 'g', 'A', 'F', '/', ':', '`', '\n', ' '];
        for &a in &edge {
            for &b in &edge {
                for &c in &edge {
                    cases.push(format!("{a}{b}{c}"));
                    for &d in &edge {
                        cases.push(format!("{a}{b}{c}{d}"));
                    }
                }
            }
        }
        for s in [
            "00ff",
            "00FF",
            "00fF",
            "0",
            "abc",
            "00\n",
            "0\n",
            "00ff\n",
            "\n00",
            " 00",
            "00 ",
            "0x00",
            "é",
            "éé",
            "0é",
            "é0",
            "ée",               // 3 bytes, odd length
            "\u{ff10}\u{ff10}", // fullwidth digits
            "\u{0660}\u{0661}", // Arabic-Indic digits
            "00\u{0}",
            "\u{0}\u{0}",
            "😀",
            "00😀",
        ] {
            cases.push(s.into());
        }
        cases.push("a".repeat(1 << 16));
        cases.push("a".repeat((1 << 16) + 1));
        cases.push(format!("{}g", "0f".repeat(4096)));
        cases.push("0f".repeat(4096));
        let mut accepted = 0;
        for s in &cases {
            let fast = check(FAST, &text(s));
            assert_eq!(fast, check(REGEX, &text(s)), "{s:?}");
            accepted += usize::from(fast.is_ok());
        }
        assert!(accepted > 256, "differential set must include accepts");
    }

    #[test]
    fn fast_path_accepts_and_rejects_exact_cases() {
        for ok in ["", "00", "ff", "0a9f", "deadbeef"] {
            assert_eq!(check(FAST, &text(ok)), Ok(()), "{ok:?}");
        }
        for bad in [
            "0", "abc", "FF", "0F", "g0", "0g", "00\n", "\n00", "é", "0é", "0x", " 00",
        ] {
            assert_eq!(
                check(FAST, &text(bad)),
                Err("regexp mismatch".into()),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn fast_path_rejects_non_text_like_regex_engine() {
        for v in [
            Value::Null,
            Value::Bool(true),
            Value::Integer(0u8.into()),
            Value::Array(vec![]),
        ] {
            assert_eq!(check(FAST, &v), Err("expected text".into()), "{v:?}");
            assert_eq!(check(FAST, &v), check(REGEX, &v));
        }
    }

    #[test]
    fn fast_path_is_only_for_the_exact_pattern_string() {
        // Look-alike patterns have different languages and must still use the
        // regex engine, not the fast path.
        let plus = "^([0-9a-f]{2})+$";
        assert_eq!(check(plus, &text("")), Err("regexp mismatch".into()));
        assert_eq!(check(plus, &text("00")), Ok(()));
        let upper = "^([0-9A-F]{2})*$";
        assert_eq!(check(upper, &text("00")), Ok(()));
        assert_eq!(check(upper, &text("ff")), Err("regexp mismatch".into()));
        let one_more = "^([0-9a-f]{2})*x$";
        assert_eq!(check(one_more, &text("00")), Err("regexp mismatch".into()));
    }
}

#[cfg(test)]
mod lazy_schema_tests {
    use super::*;

    #[test]
    fn fresh_schema_parses_only_reached_rules_and_rejects_unknown() {
        let schema = Schema::embedded();
        // The runtime is also compiled against generated, non-Coglet schemas.
        // Parsing one rule must not eagerly parse the rest of that schema.
        let rule = generated::SCHEMA_RULES.first().unwrap().0;
        schema.rule(rule).unwrap();
        let Schema::Embedded(cache) = &schema else {
            unreachable!()
        };
        let parsed = cache.iter().filter(|slot| slot.get().is_some()).count();
        assert!(parsed == 1, "parsed {parsed} of {} rules", cache.len());
        assert_eq!(
            schema.rule("not-a-rule"),
            Err("unknown rule `not-a-rule`".into())
        );
    }

    #[test]
    fn generated_rule_table_is_exact_sorted_and_unique() {
        let all: BTreeMap<String, Json> = serde_json::from_str(SCHEMA_JSON).unwrap();
        assert_eq!(generated::SCHEMA_RULES.len(), all.len());
        let mut previous = None;
        for &(name, raw) in generated::SCHEMA_RULES {
            assert!(previous.is_none_or(|p| p < name));
            assert_eq!(serde_json::from_str::<Json>(raw).unwrap(), all[name]);
            previous = Some(name);
        }
    }
}
