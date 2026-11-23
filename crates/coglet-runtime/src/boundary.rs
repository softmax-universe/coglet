use crate::{Result, Snapshot, validate};
use serde_json::Value;
use std::collections::BTreeSet;

/// Parse exact canonical JSONL, then validate the embedded event as well as its
/// container. Canonical reserialization also rejects duplicate JSON object keys.
pub fn inbox(files: &Snapshot) -> Result<Vec<Value>> {
    let raw = hex::decode(
        files
            .files()
            .get("/data/inbox.jsonl")
            .map(String::as_str)
            .unwrap_or(""),
    )
    .map_err(|e| e.to_string())?;
    if !raw.is_empty() && raw.last() != Some(&b'\n') {
        return Err("inbox missing LF".into());
    }
    let mut result = Vec::new();
    let mut ids = BTreeSet::new();
    for terminated in raw.split_inclusive(|b| *b == b'\n') {
        let line = &terminated[..terminated.len() - 1];
        let message: Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        validate("inbox-message", &message)?;
        if serde_json::to_vec(&message).map_err(|e| e.to_string())? != line {
            return Err("noncanonical inbox JSONL".into());
        }
        if message["index"].as_u64() != Some(result.len() as u64)
            || message["sender"] != message["event"]["source"]
        {
            return Err("inbox index/sender mismatch".into());
        }
        if !ids.insert(message["id"].as_str().unwrap().to_owned()) {
            return Err("duplicate inbox ID".into());
        }
        match message["event"]["kind"].as_str().unwrap() {
            "llm.reply" => validate("llm-reply", &message["event"]["payload"])?,
            "runtime.action_rejected" => {
                validate("action-rejection", &message["event"]["payload"])?
            }
            "action.outcome" => {
                validate("action-outcome", &message["event"]["payload"])?;
                if message["event"]["correlation_id"]
                    != message["event"]["payload"]["correlation_id"]
                {
                    return Err("outcome correlation mismatch".into());
                }
            }
            _ => (),
        }
        result.push(message);
    }
    Ok(result)
}

/// Input is an already authenticated applied mutation, not an authorization API.
/// The hub link must authenticate the operation before passing it to this seam.
pub fn apply(files: &Snapshot, change: &Value, coglet: &Value) -> Result<Snapshot> {
    validate("boundary-change", change)?;
    if &change["target"] != coglet {
        return Err("boundary target mismatch".into());
    }
    let path = change["path"].as_str().unwrap();
    let before = files
        .files()
        .get(path)
        .map(|s| Value::String(s.clone()))
        .unwrap_or(Value::Null);
    if before != change["before"] {
        return Err("boundary before bytes mismatch".into());
    }
    let method = change["method"].as_str().unwrap();
    match method {
        "runtime.heard" if path == crate::heard::PATH => {
            if &change["actor"] != coglet || !change["ticket"].is_null() {
                return Err("runtime heard actor/ticket mismatch".into());
            }
            crate::heard::transition(
                before.as_str(),
                change["after"]
                    .as_str()
                    .ok_or("heard history cannot be removed")?,
            )?;
        }
        "hub.inbox" | "runtime.receipt" if path == "/data/inbox.jsonl" => {
            if method == "hub.inbox" {
                if !change["actor"].as_str().unwrap().starts_with("hub:") {
                    return Err("inbox requires authenticated hub actor".into());
                }
            } else if &change["actor"] != coglet || !change["ticket"].is_null() {
                return Err("runtime receipt actor/ticket mismatch".into());
            }
            let old = before.as_str().unwrap_or("");
            let new = change["after"].as_str().ok_or("inbox cannot be removed")?;
            if !new.starts_with(old) || new.len() <= old.len() {
                return Err("inbox is append-only".into());
            }
        }
        "debug.write"
            if path.starts_with("/data/")
                && !matches!(path, "/data/inbox.jsonl" | "/data/runtime/heard.cbor") =>
        {
            if !change["actor"].as_str().unwrap().starts_with("hub:") {
                return Err("debug.write requires authenticated hub actor".into());
            }
        }
        _ => return Err("unsupported or forbidden boundary mutation".into()),
    }
    let mut next = files.files().clone();
    if let Some(after) = change["after"].as_str() {
        next.insert(path.into(), after.into());
    } else {
        next.remove(path);
    }
    let next = Snapshot::new(next)?;
    if matches!(method, "hub.inbox" | "runtime.receipt") {
        let old_count = inbox(files)?.len();
        let messages = inbox(&next)?;
        if messages.len() != old_count + 1 {
            return Err("one inbox append required per boundary".into());
        }
        let terminal = matches!(
            messages.last().unwrap()["event"]["kind"].as_str(),
            Some("action.outcome" | "runtime.action_rejected")
        );
        if terminal != (method == "runtime.receipt") {
            return Err("terminal messages require runtime.receipt exclusively".into());
        }
    }
    Ok(next)
}
