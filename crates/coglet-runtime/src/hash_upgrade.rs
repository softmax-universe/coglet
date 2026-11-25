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
