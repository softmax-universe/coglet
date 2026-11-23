//! Runtime-owned read barriers and M-D boundary construction, never guest events.
use super::*;
use crate::{SurfaceSession, heard};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct HeardObservation {
    pub(super) surface: String,
    pub(super) epoch: u64,
    pub(super) item: String,
    pub(super) played: u64,
    pub(super) stopped: bool,
    pub(super) expires: u64,
}
impl RuntimeService {
    pub(crate) fn heard_owner(&self) -> String {
        self.state.runtime.manifest["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
    pub(super) fn stage_heard_observation(
        next: &mut ServiceState,
        session: &SurfaceSession,
        event: &Value,
        expires: u64,
    ) -> Result<Option<String>> {
        if !session.collects_heard()
            || !matches!(
                event["kind"].as_str(),
                Some("speech.position" | "speech.barge_in")
            )
        {
            return Ok(None);
        }
        let payload = &event["payload"];
        let item = payload["item"].as_str().ok_or("missing playback item")?;

        if next.heard_pending.len() >= 128 {
            return Err("resource_exhausted: heard barriers".into());
        }
        let id = digest(&cbor(&json!([
            "runtime.heard",
            session.surface(),
            session.grant_epoch(),
            event["seq"]
        ]))?)[..32]
            .to_owned();
        next.heard_pending.insert(
            id.clone(),
            HeardObservation {
                surface: session.surface().to_owned(),
                epoch: session.grant_epoch(),
                item: item.into(),
                expires,
                played: payload["played_ms"]
                    .as_u64()
                    .ok_or("missing played position")?,
                stopped: payload["state"] == "stopped"
                    || payload["state"] == "finished"
                    || payload["reflex"] == "stopped",
            },
        );
        Ok(Some(id))
    }
    pub(crate) fn record_heard_read(
        &mut self,
        session: &SurfaceSession,
        id: &str,
        read: &Value,
        snapshot: u64,
        now: u64,
    ) -> Result<()> {
        let pending = self
            .state
            .heard_pending
            .get(id)
            .ok_or("missing heard barrier")?;
        let owner = self.heard_owner();
        if pending.surface != session.surface()
            || pending.epoch != session.grant_epoch()
            || self.surface_epoch(session.surface()) != pending.epoch
        {
            return Err("stale heard observation".into());
        }
        session.authorize_heard(&owner, now)?;
        self.check_heard_owner(session, &owner, &pending.item)?;
        let played = read["played_ms"].as_u64().ok_or("missing read position")?;
        if read["item"] != pending.item
            || played < pending.played
            || (pending.stopped && played != pending.played)
        {
            // Live playback may advance before a read. Stopped cutoffs cannot.
            return Err("heard reply does not match playback cutoff".into());
        }
        let mut next = self.state.clone();
        let current_snapshot = next.speech_temporal.playback.observe_snapshot(
            session.surface(),
            &pending.item,
            snapshot,
        )?;
        let mut staged = next
            .staged_boundary
            .take()
            .unwrap_or_else(|| StagedBoundary {
                changes: Vec::new(),
                runtime: next.runtime.clone(),
            });
        if staged.runtime.sealed || now < staged.runtime.time {
            return Err("cannot materialize heard in sealed/past segment".into());
        }
        let before = staged.runtime.files.files().get(heard::PATH).cloned();
        let archive = heard::decode(before.as_deref())?;
        let superseded = !current_snapshot
            || archive["items"].as_array().unwrap().iter().any(|v| {
                v["surface"] == session.surface()
                    && v["item"] == pending.item
                    && (v["played_ms"].as_u64().is_some_and(|p| played < p)
                        || (v["stopped"] == true && !pending.stopped))
            });
        // Concurrent reads may complete out of order. An authenticated older
        // snapshot releases its own barrier, never rolls back/fills frozen history.
        if superseded {
            next.staged_boundary = Some(staged);
            next.heard_pending.remove(id);
            self.store.commit(
                &serde_json::to_value(&next).map_err(|e| e.to_string())?,
                None,
            )?;
            self.state = next;
            return Ok(());
        }
        let after = heard::collect(before.as_deref(), session.surface(), read, pending.stopped)?;
        if before.as_ref() != Some(&after) {
            let change = json!({"actor":owner,"audit_id":id,"ticket":null,"method":"runtime.heard","target":owner,"path":heard::PATH,"before":before,"after":after});
            staged
                .runtime
                .apply_boundary_changes(std::slice::from_ref(&change))?;
            staged.changes.push(change);
        }
        next.staged_boundary = Some(staged);
        next.heard_pending.remove(id);
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
}
