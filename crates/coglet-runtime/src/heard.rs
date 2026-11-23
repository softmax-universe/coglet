//! Archive-only heard history: deterministic journal bytes, never a live read.
use crate::{Result, cbor, validate};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const PATH: &str = "/data/runtime/heard.cbor";

pub fn decode(encoded: Option<&str>) -> Result<Value> {
    let Some(encoded) = encoded else {
        return Ok(json!({"items":[]}));
    };
    let raw = hex::decode(encoded).map_err(|e| e.to_string())?;
    let state: Value = coglet_types::decode("heard-state", &raw)?
        .deserialized()
        .map_err(|e| e.to_string())?;
    // decode checks the deterministic profile; encode equality also binds bytes.
    if cbor(&state)? != raw {
        return Err("noncanonical heard-state".into());
    }
    validate("heard-state", &state)?;
    let mut previous = None;
    for item in state["items"].as_array().unwrap() {
        let key = (
            item["surface"].as_str().unwrap(),
            item["item"].as_str().unwrap(),
        );
        if previous.is_some_and(|old| old >= key) {
            return Err("heard items must be sorted and unique".into());
        }
        previous = Some(key);
        let duration = item["duration_ms"].as_u64().unwrap();
        let played = item["played_ms"].as_u64().unwrap();
        if played > duration {
            return Err("heard position exceeds duration".into());
        }
        let mut end = 0;
        let mut projected = String::new();
        for span in item["spans"].as_array().unwrap() {
            let at = span["end_ms"].as_u64().unwrap();
            if at <= end || at > duration {
                return Err("heard span order/duration mismatch".into());
            }
            end = at;
            if at <= played && projected.len() < 16_384 {
                // Prefix of concatenated spans, capped without splitting UTF-8.
                let text = span["text"].as_str().unwrap();
                let mut n = (16_384 - projected.len()).min(text.len());
                while !text.is_char_boundary(n) {
                    n -= 1;
                }
                projected.push_str(&text[..n]);
                // Once a character cannot fit, later spans cannot fill the gap.
                if n < text.len() {
                    break;
                }
            }
        }
        // Do not stop validation at a truncated span: all alignment is evidence.
        let mut last = 0;
        for span in item["spans"].as_array().unwrap() {
            let at = span["end_ms"].as_u64().unwrap();
            if at <= last || at > duration {
                return Err("heard span order/duration mismatch".into());
            }
            last = at;
        }
        if item["heard"] != projected {
            return Err("heard projection mismatch".into());
        }
    }
    Ok(state)
}

pub fn transition(before: Option<&str>, after: &str) -> Result<()> {
    let before = decode(before)?;
    let after = decode(Some(after))?;
    let current: BTreeMap<_, _> = after["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                (
                    item["surface"].as_str().unwrap(),
                    item["item"].as_str().unwrap(),
                ),
                item,
            )
        })
        .collect();
    for old in before["items"].as_array().unwrap() {
        let key = (
            old["surface"].as_str().unwrap(),
            old["item"].as_str().unwrap(),
        );
        let new = current.get(&key).ok_or("heard items cannot be removed")?;
        let old_spans = old["spans"].as_array().unwrap();
        let new_spans = new["spans"].as_array().unwrap();
        if new["duration_ms"].as_u64() < old["duration_ms"].as_u64()
            || new["played_ms"].as_u64() < old["played_ms"].as_u64()
            || (old["stopped"] == true
                && (new["stopped"] != true || new["played_ms"] != old["played_ms"]))
            || !new_spans.starts_with(old_spans)
        {
            return Err("heard history regressed or rewrote alignment".into());
        }
    }
    Ok(())
}
