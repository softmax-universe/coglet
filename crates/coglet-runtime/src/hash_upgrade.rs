//! Private historical WAL reader. Never used for public admission or replay.
use super::*;
use serde_json::{Value, json};
use std::cell::Cell;
thread_local! { static HISTORICAL: Cell<bool> = const { Cell::new(false) }; }
pub(crate) fn historical() -> bool {
    HISTORICAL.with(Cell::get)
}
/// Scoped and panic-safe; only trusted recovery/archive paths may enter this.
pub(crate) fn historical_scope<T>(enabled: bool, f: impl FnOnce() -> T) -> T {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) {
            HISTORICAL.with(|v| v.set(self.0));
        }
    }
    let _reset = Reset(HISTORICAL.with(|v| v.replace(enabled)));
    f()
}
pub(crate) fn legacy_version(v: &Value) -> bool {
    v == "0.1" || v == "0.2"
}
pub(crate) fn database_version(v: &Value) -> bool {
    v == "0.2" || v == "1.1"
}
pub(crate) fn current_version(v: &Value) -> Result<&'static str> {
    match v.as_str() {
        Some("0.1" | "1.0") => Ok("1.0"),
        Some("0.2" | "1.1") => Ok("1.1"),
        _ => Err("unknown ABI version".into()),
    }
}

/// Validation-only projection for selected surface-0.2 roots written under the
/// historical ABI 0.1 / DB 0.2 literals. Only the known structural `version`
/// fields (root, trace records, trailing boundary) are mapped to the current
/// literal; payloads, names and every other byte are untouched, and the result
/// is never persisted or hashed. Outside the private historical scope, or for a
/// current/unknown/mixed version, returns `None` and the strict current rule
/// applies unchanged.
pub(crate) fn current_projection(
    rule: &str,
    value: &coglet_types::Value,
) -> Result<Option<coglet_types::Value>> {
    use coglet_types::Value as V;
    if !historical() {
        return Ok(None);
    }
    let (old, new, trace) = match rule {
        "surface02::s02-manifest" | "surface02::s02-step-record" => ("0.1", "1.0", false),
        "surface02::s02-trace" => ("0.1", "1.0", true),
        "surface02::s02-db-manifest" | "surface02::s02-db-step-record" => ("0.2", "1.1", false),
        "surface02::s02-db-trace" => ("0.2", "1.1", true),
        _ => return Ok(None),
    };
    fn map_version(v: &V, old: &str, new: &str) -> Result<V> {
        let V::Map(entries) = v else {
            return Err("historical root is not a map".into());
        };
        let mut seen = 0;
        let out = entries
            .iter()
            .map(|(k, x)| {
                if matches!(k, V::Text(t) if t == "version") {
                    seen += 1;
                    if x == &V::Text(old.into()) {
                        return Ok((k.clone(), V::Text(new.into())));
                    }
                    return Err("mixed or non-historical ABI version".to_string());
                }
                Ok((k.clone(), x.clone()))
            })
            .collect::<Result<Vec<_>>>()?;
        if seen != 1 {
            return Err("historical root needs exactly one version".into());
        }
        Ok(V::Map(out))
    }
    let V::Map(entries) = value else {
        return Ok(None);
    };
    let is_old = entries
        .iter()
        .any(|(k, x)| matches!(k, V::Text(t) if t == "version") && x == &V::Text(old.into()));
    if !is_old {
        return Ok(None);
    }
    let mut root = map_version(value, old, new)?;
    if trace && let V::Map(out) = &mut root {
        for (k, x) in out.iter_mut() {
            let V::Text(name) = k else { continue };
            match (name.as_str(), &*x) {
                ("records", V::Array(items)) => {
                    *x = V::Array(
                        items
                            .iter()
                            .map(|i| map_version(i, old, new))
                            .collect::<Result<_>>()?,
                    )
                }
                ("trailing_boundary", V::Null) => {}
                ("trailing_boundary", b) => *x = map_version(b, old, new)?,
                _ => {}
            }
        }
    }
    Ok(Some(root))
}
impl Runtime {
    /// Internal cut, like a code revision: no empty ABI boundary is fabricated.
    pub(super) fn hash_successor(&self, trace_id: &Value) -> Result<Self> {
        if !legacy_version(&self.manifest["version"]) || self.unresolved_attempt.is_some() {
            return Err("hash upgrade requires resolved historical predecessor".into());
        }
        let mut old = self.clone();
        old.sealed = true;
        let mut next = historical_scope(true, || old.start_segment(trace_id))?;
        next.initial_setup["manifest"]["version"] =
            json!(current_version(&self.manifest["version"])?);
        let checked = historical_scope(false, || Self::from_fixture(&next.initial_setup))?;
        next.files = checked.files;
        next.manifest = checked.manifest;
        Ok(next)
    }
}
/// Decode current ABI records without changing the frozen hub/0 profile type.
pub(crate) fn decode_current(rule: &str, raw: &[u8]) -> Result<coglet_types::Value> {
    coglet_types_current::decode(rule, raw)?;
    coglet_types::decode_profile(raw)
}
pub(crate) fn encode_database(value: &coglet_types::Value) -> Result<Vec<u8>> {
    let raw = coglet_types::encode_profile(value)?;
    decode_current("database::db-step-record", &raw)?;
    Ok(raw)
}

impl RuntimeService {
    /// Keep an uncertain guest/database decision fenced until its authenticated
    /// original outcome is reconciled. Never cut away the reservation.
    pub(super) fn upgrade_hash_if_ready(&mut self) -> Result<()> {
        if !legacy_version(&self.state.runtime.manifest["version"])
            || self.state.runtime.unresolved_attempt.is_some()
        {
            return Ok(());
        }
        let mut next = self.state.clone();
        let old_id = match next.runtime.trace_id.clone() {
            Some(id) => id,
            None => json!(&digest(&cbor(&next.runtime.initial_setup)?)[..32]),
        };
        let next_id = json!(
            &digest(&cbor(&json!([
                "git-tree-upgrade",
                old_id,
                self.store.records().len()
            ]))?)[..32]
        );
        if next
            .completed_segments
            .iter()
            .any(|segment| segment.trace_id == next_id)
        {
            return Err("hash upgrade successor trace identity already used".into());
        }
        let successor = next.runtime.hash_successor(&next_id)?;
        next.completed_segments.push(trace::CompletedSegment {
            initial_setup: next.runtime.initial_setup.clone(),
            trace_id: old_id,
            end: self.store.records().len(),
            successor_trace: Some(next_id),
            code_revision: None,
            hash_upgrade: true,
        });
        next.runtime = successor;
        // Already acknowledged host mutations stay pending, with identical
        // change bytes and queue reservations. They belong to the successor's
        // first actual boundary, not to a fabricated empty predecessor record.
        if let Some(staged) = &mut next.staged_boundary {
            let mut runtime = next.runtime.clone();
            runtime.apply_boundary_changes(&staged.changes)?;
            runtime.next_source_seq = staged.runtime.next_source_seq;
            staged.runtime = runtime;
        }
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        #[cfg(feature = "native-engine")]
        {
            self.native = None;
        }
        Ok(())
    }
}
