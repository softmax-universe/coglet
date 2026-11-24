//! Durable private observation admission and lease-scoped queue retention.
use super::*;
use crate::SurfaceSession;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct EventLease {
    surface: String,
    epoch: u64,
    expires: u64,
    pub(super) event: Value,
}
impl RuntimeService {
    pub(super) fn validate_private_events(state: &ServiceState) -> Result<()> {
        for (ordinal, lease) in &state.private_events {
            let source = lease.event["source"]
                .as_str()
                .ok_or("private source missing")?;
            let seq = lease.event["seq"].as_u64().ok_or("private seq missing")?;
            let time = lease.event["logical_time"]
                .as_u64()
                .ok_or("private clock missing")?;
            if source != lease.surface
                || lease.epoch == 0
                || lease.expires == 0
                || state.surface_epochs.get(source) != Some(&lease.epoch)
                || !state
                    .router
                    .entries()
                    .any(|(n, e)| n == ordinal && e == &lease.event)
                || state
                    .surface_sequences
                    .get(source)
                    .is_none_or(|(s, t)| *s < seq || *t < time)
            {
                return Err("invalid recovered private lease/ordinal/event/epoch/highwater".into());
            }
        }
        Ok(())
    }
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
    ) -> Result<Option<String>> {
        if self.state.runtime.unresolved_attempt.is_some() {
            return Err("observations deferred until attempt reconciliation".into());
        }
        validate("event", event)?;
        let kind = event["kind"].as_str().ok_or("missing event kind")?;
        if event["source"] != session.surface()
            || self.surface_epoch(session.surface()) != session.grant_epoch()
        {
            return Err("unauthenticated or stale event source".into());
        }
        self.catalog
            .check(kind, "event", false, &event["payload"])?;
        if self.catalog.single_owner(kind) && crate::cbor(&event["payload"])?.len() > 262_144 {
            return Err("event payload exceeds encoded byte limit".into());
        }
        if event["payload"]["text"]
            .as_str()
            .is_some_and(|s| s.len() > 16_384)
        {
            return Err("speech transcript exceeds UTF-8 byte limit".into());
        }
        let coglet = self.state.runtime.manifest["id"].as_str().unwrap();
        let expires = session.event_lease(coglet, kind, now)?;
        if let Some(read) = self.catalog.selected_read(kind, &event["payload"])? {
            if kind == "game.observation"
                && event["payload"]["grant_epoch"] != session.grant_epoch()
            {
                return Err("selected observation grant epoch mismatch".into());
            }
            session.authorize(coglet, read, "read", now)?;
        }
        if kind == "speech.tool_call" {
            self.check_provider_session(session, coglet, &event["payload"]["session"])?;
        }
        let playback = matches!(kind, "speech.position" | "speech.barge_in");
        let provider = if playback {
            self.playback_owner(session, coglet, &event["payload"])?
        } else {
            None
        };
        let mut next = self.state.clone();
        Self::advance_surface_sequence(&mut next, event)?;
        let mut deliver =
            next.speech_temporal
                .admit(session.surface(), coglet, session.grant_epoch(), event)?;
        if playback {
            deliver &= next.speech_temporal.playback.admit(
                session.surface(),
                coglet,
                session.grant_epoch(),
                provider,
                event,
            )?;
        }
        let heard = if deliver {
            Self::stage_heard_observation(&mut next, session, event, expires)?
        } else {
            None
        };
        if deliver
            && self.state.runtime.manifest["subscriptions"]
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
        self.commit_surface_state(next)?;
        Ok(heard)
    }
    pub(crate) fn purge_surface_events(&mut self, surface: &str, epoch: u64) -> Result<()> {
        let mut next = self.state.clone();
        let old_heard = next.heard_pending.len();
        next.heard_pending
            .retain(|_, p| p.surface != surface || p.epoch != epoch);
        let revoked: Vec<_> = next
            .private_events
            .iter()
            .filter_map(|(id, lease)| {
                (lease.surface == surface && lease.epoch == epoch).then_some(*id)
            })
            .collect();
        if revoked.is_empty() && old_heard == next.heard_pending.len() {
            return Ok(());
        }
        for id in &revoked {
            next.private_events.remove(id);
        }
        let attempted = Self::attempted_ordinals(&next)?;
        next.router
            .retain_ordinals(|ordinal| !revoked.contains(&ordinal) || attempted.contains(&ordinal));
        self.commit_surface_state(next)
    }
    pub(super) fn expire_private_events(&mut self, now: u64) -> Result<()> {
        let mut next = self.state.clone();
        let old_heard = next.heard_pending.len();
        next.heard_pending
            .retain(|_, p| now < p.expires && self.surface_epoch(&p.surface) == p.epoch);
        let revoked: Vec<_> = next
            .private_events
            .iter()
            .filter_map(|(id, lease)| {
                (now >= lease.expires || self.surface_epoch(&lease.surface) != lease.epoch)
                    .then_some(*id)
            })
            .collect();
        if revoked.is_empty() && old_heard == next.heard_pending.len() {
            return Ok(());
        }
        for id in &revoked {
            next.private_events.remove(id);
        }
        let attempted = Self::attempted_ordinals(&next)?;
        next.router
            .retain_ordinals(|ordinal| !revoked.contains(&ordinal) || attempted.contains(&ordinal));
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
