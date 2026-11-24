//! Heard history: authenticated live construction and archive-only validation.
use crate::{Result, cbor, validate};
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub const PATH: &str = "/data/runtime/heard.cbor";

/// Construct M-D alignment only from an already authenticated owner read.
/// The caller supplies the stop fence from trusted playback/control state.
pub(crate) fn collect(
    before: Option<&str>,
    surface: &str,
    read: &Value,
    stopped: bool,
) -> Result<String> {
    crate::catalog::check(
        &crate::catalog::operation("speech.heard", "read")?["output"],
        read,
    )?;
    let mut state = decode(before)?;
    let items = state["items"].as_array_mut().unwrap();
    let index = if let Some(i) = items
        .iter()
        .position(|v| v["surface"] == surface && v["item"] == read["item"])
    {
        i
    } else {
        if items.len() >= 4096 {
            return Err("resource_exhausted: heard item lifetime cap".into());
        }
        items.push(json!({"surface":surface,"item":read["item"],"duration_ms":0,"played_ms":0,"stopped":false,"spans":[],"heard":""}));
        items.len() - 1
    };
    let item = &mut items[index];
    let played = read["played_ms"].as_u64().unwrap();
    let text = read["text"].as_str().unwrap();
    if text.len() > 16_384 {
        return Err("heard text exceeds UTF-8 byte limit".into());
    }
    if played < item["played_ms"].as_u64().unwrap() {
        return Err("heard position regressed".into());
    }
    if item["stopped"] == true {
        if played != item["played_ms"].as_u64().unwrap() || !stopped {
            return Err("stopped heard item advanced or resumed".into());
        }
        return Ok(hex::encode(cbor(&state)?));
    }
    let spans = item["spans"].as_array_mut().unwrap();
    let prefix: String = spans.iter().map(|v| v["text"].as_str().unwrap()).collect();
    let end = spans.last().and_then(|v| v["end_ms"].as_u64()).unwrap_or(0);
    if played > end && text.len() > prefix.len() && text.starts_with(&prefix) {
        spans.push(json!({"end_ms":played,"text":&text[prefix.len()..]}));
    }
    let heard: String = spans
        .iter()
        .filter(|v| v["end_ms"].as_u64().unwrap() <= played)
        .map(|v| v["text"].as_str().unwrap())
        .collect();
    item["heard"] = json!(heard);
    item["played_ms"] = json!(played);
    item["duration_ms"] = json!(played.max(item["duration_ms"].as_u64().unwrap()));
    item["stopped"] = json!(stopped);
    items.sort_by(|a, b| {
        (a["surface"].as_str(), a["item"].as_str())
            .cmp(&(b["surface"].as_str(), b["item"].as_str()))
    });
    let encoded = hex::encode(cbor(&state)?);
    transition(before, &encoded)?;
    Ok(encoded)
}

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
            || (old["stopped"] == true && **new != *old)
            || !new_spans.starts_with(old_spans)
        {
            return Err("heard history regressed or rewrote alignment".into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn owner_reads_append_exact_prefix_and_freeze_entire_stopped_item() {
        let surface = format!("surface:{}", "b".repeat(64));
        let item = "4".repeat(32);
        let read = |played, text: &str| json!({"item":item,"played_ms":played,"text":text});
        let a = collect(None, &surface, &read(10, "hé"), false).unwrap();
        let b = collect(Some(&a), &surface, &read(20, "revised"), false).unwrap();
        let c = collect(Some(&b), &surface, &read(30, "héllo"), true).unwrap();
        let state = decode(Some(&c)).unwrap();
        assert_eq!(
            state["items"][0]["spans"],
            json!([{"end_ms":10,"text":"hé"},{"end_ms":30,"text":"llo"}])
        );
        assert_eq!(state["items"][0]["heard"], "héllo");
        assert_eq!(
            collect(Some(&c), &surface, &read(30, "héllo"), true).unwrap(),
            c
        );
        assert!(collect(Some(&c), &surface, &read(31, "héllo!"), true).is_err());
        assert!(collect(Some(&b), &surface, &read(19, "hé"), false).is_err());
        let mut extended = state.clone();
        extended["items"][0]["duration_ms"] = json!(31);
        assert!(transition(Some(&c), &hex::encode(cbor(&extended).unwrap())).is_err());
    }
    #[test]
    fn heard_truncation_hello_world_1200_of_3000() {
        let surface = format!("surface:{}", "a".repeat(64));
        let item = "1".repeat(32);
        let encoded = collect(
            None,
            &surface,
            &json!({"item":item,"played_ms":1200,"text":"Hello"}),
            true,
        )
        .unwrap();
        let state = decode(Some(&encoded)).unwrap();
        assert_eq!(state["items"][0]["heard"], "Hello");
        assert_eq!(state["items"][0]["played_ms"], 1200);
        assert_eq!(state["items"][0]["stopped"], true);
        let replay = collect(
            Some(&encoded),
            &surface,
            &json!({"item":item,"played_ms":1200,"text":"Hello"}),
            true,
        )
        .unwrap();
        assert_eq!(replay, encoded);
    }
}
