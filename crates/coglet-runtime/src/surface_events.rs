//! Durable private observation admission and lease-scoped queue retention.
use super::*;
use crate::{SurfaceSession, catalog};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EventLease {
    surface: String,
    epoch: u64,
    expires: u64,
    pub(super) event: Value,
}
impl RuntimeService {
    pub(super) fn advance_surface_sequence(next: &mut ServiceState, event: &Value) -> Result<()> {
        let source = event["source"].as_str().ok_or("missing source")?;
        let seq = event["seq"].as_u64().ok_or("missing seq")?;
        let clock = event["logical_time"]
            .as_u64()
            .ok_or("missing event clock")?;
        if next
            .surface_sequences
            .get(source)
            .is_some_and(|(old_seq, old_clock)| seq <= *old_seq || clock < *old_clock)
        {
            return Err("surface source sequence/clock rollback".into());
        }
        next.surface_sequences
            .insert(source.to_owned(), (seq, clock));
        Ok(())
    }
    pub(crate) fn record_surface_event(
        &mut self,
        session: &SurfaceSession,
        event: &Value,
        now: u64,
    ) -> Result<()> {
        validate("event", event)?;
        let kind = event["kind"].as_str().ok_or("missing event kind")?;
        if event["source"] != session.surface()
            || self.surface_epoch(session.surface()) != session.grant_epoch()
        {
            return Err("unauthenticated or stale event source".into());
        }
        let op = catalog::operation(kind, "event")?;
        catalog::check(&op["input"], &event["payload"])?;
        if event["payload"]["text"]
            .as_str()
            .is_some_and(|s| s.len() > 16_384)
        {
            return Err("speech transcript exceeds UTF-8 byte limit".into());
        }
        let coglet = self.state.runtime.manifest["id"].as_str().unwrap();
        let expires = session.event_lease(coglet, kind, now)?;
        if kind == "speech.tool_call" {
            self.check_provider_session(session, coglet, &event["payload"]["session"])?;
        }
        let mut next = self.state.clone();
        Self::advance_surface_sequence(&mut next, event)?;
        if self.state.runtime.manifest["subscriptions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == kind)
        {
            let ordinal = next.router.enqueue(event.clone())?;
            next.private_events.insert(
                ordinal,
                EventLease {
                    surface: session.surface().to_owned(),
                    epoch: session.grant_epoch(),
                    expires,
                    event: event.clone(),
                },
            );
        }
        self.commit_surface_state(next)
    }
    pub(crate) fn purge_surface_events(&mut self, surface: &str, epoch: u64) -> Result<()> {
        let mut next = self.state.clone();
        let revoked: Vec<_> = next
            .private_events
            .iter()
            .filter_map(|(id, lease)| {
                (lease.surface == surface && lease.epoch == epoch).then_some(*id)
            })
            .collect();
        if revoked.is_empty() {
            return Ok(());
        }
        for id in &revoked {
            next.private_events.remove(id);
        }
        next.router
            .retain_ordinals(|ordinal| !revoked.contains(&ordinal));
        self.commit_surface_state(next)
    }
    pub(super) fn expire_private_events(&mut self, now: u64) -> Result<()> {
        let mut next = self.state.clone();
        let revoked: Vec<_> = next
            .private_events
            .iter()
            .filter_map(|(id, lease)| {
                (now >= lease.expires || self.surface_epoch(&lease.surface) != lease.epoch)
                    .then_some(*id)
            })
            .collect();
        if revoked.is_empty() {
            return Ok(());
        }
        for id in &revoked {
            next.private_events.remove(id);
        }
        next.router
            .retain_ordinals(|ordinal| !revoked.contains(&ordinal));
        self.commit_surface_state(next)
    }
    fn commit_surface_state(&mut self, next: ServiceState) -> Result<()> {
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(())
    }
}
