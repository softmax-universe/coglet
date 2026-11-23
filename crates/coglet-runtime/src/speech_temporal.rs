//! Runtime defense in depth for private speech input. Tombstones persist across
//! links/epochs: changing owners cannot reopen an utterance or provider call.
use super::*;

const MAX_IDENTITIES: usize = 4096;
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SpeechTemporal {
    utterances: BTreeMap<String, Utterance>,
    calls: BTreeMap<String, Value>,
    #[serde(default)]
    discarded_revisions: u64,
    #[serde(default)]
    repeated_calls: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Utterance {
    owner: String,
    epoch: u64,
    revision: u64,
    start: u64,
    ended: bool,
}
impl SpeechTemporal {
    pub(super) fn validate(&self, epochs: &BTreeMap<String, u64>, owner: &Value) -> Result<()> {
        if self.utterances.len() > MAX_IDENTITIES || self.calls.len() > MAX_IDENTITIES {
            return Err("recovered speech identity bound exceeded".into());
        }
        for (key, utterance) in &self.utterances {
            let (surface, id) = key
                .split_once('/')
                .ok_or("invalid recovered utterance key")?;
            validate("surface-id", &json!(surface))?;
            validate("correlation-id", &json!(id))?;
            if owner != &utterance.owner
                || utterance.epoch == 0
                || epochs.get(surface).is_none_or(|e| utterance.epoch > *e)
                || (utterance.ended && utterance.revision == 0)
            {
                return Err("invalid recovered capture owner/epoch/revision".into());
            }
        }
        for (key, payload) in &self.calls {
            let (surface, _) = key
                .split_once('/')
                .ok_or("invalid recovered tool-call key")?;
            validate("surface-id", &json!(surface))?;
            crate::catalog::check(
                &crate::catalog::operation("speech.tool_call", "event")?["input"],
                payload,
            )?;
            if !epochs.contains_key(surface)
                || *key
                    != format!(
                        "{surface}/{}/{}",
                        payload["session"]
                            .as_str()
                            .ok_or("missing recovered session")?,
                        payload["call"].as_str().ok_or("missing recovered call")?
                    )
            {
                return Err("recovered provider call key mismatch".into());
            }
        }
        Ok(())
    }
    /// false consumes only the source highwater, never queues stale private text.
    pub(super) fn admit(
        &mut self,
        surface: &str,
        owner: &str,
        epoch: u64,
        event: &Value,
    ) -> Result<bool> {
        let payload = &event["payload"];
        let kind = event["kind"].as_str().ok_or("missing speech kind")?;
        let key = |id: &str| format!("{surface}/{id}");
        match kind {
            "speech.start" => {
                let id = key(payload["utterance"].as_str().ok_or("missing utterance")?);
                if self.utterances.contains_key(&id) {
                    return Err("conflict: utterance identity reused".into());
                }
                if self.utterances.len() >= MAX_IDENTITIES {
                    return Err("resource_exhausted: utterance tombstones full".into());
                }
                self.utterances.insert(
                    id,
                    Utterance {
                        owner: owner.into(),
                        epoch,
                        revision: 0,
                        start: payload["position_ms"]
                            .as_u64()
                            .ok_or("missing capture position")?,
                        ended: false,
                    },
                );
            }
            "speech.partial" | "speech.end" => {
                let id = key(payload["utterance"].as_str().ok_or("missing utterance")?);
                let utterance = self
                    .utterances
                    .get_mut(&id)
                    .ok_or("speech update without start")?;
                if utterance.owner != owner || utterance.epoch != epoch {
                    return Err("speech update belongs to revoked capture".into());
                }
                let revision = payload["revision"].as_u64().ok_or("missing revision")?;
                if utterance.ended || revision <= utterance.revision {
                    self.discarded_revisions = self
                        .discarded_revisions
                        .checked_add(1)
                        .ok_or("speech discard counter exhausted")?;
                    return Ok(false);
                }
                if kind == "speech.end" {
                    if payload["position_ms"]
                        .as_u64()
                        .ok_or("missing end position")?
                        < utterance.start
                    {
                        return Err("capture position regressed".into());
                    }
                    utterance.ended = true;
                }
                utterance.revision = revision;
            }
            "speech.tool_call" => {
                let id = format!(
                    "{surface}/{}/{}",
                    payload["session"].as_str().ok_or("missing session")?,
                    payload["call"].as_str().ok_or("missing call")?
                );
                if let Some(old) = self.calls.get(&id) {
                    if old != payload {
                        return Err("conflict: provider call identity reused".into());
                    }
                    self.repeated_calls = self
                        .repeated_calls
                        .checked_add(1)
                        .ok_or("provider repeat counter exhausted")?;
                    return Ok(false);
                }
                if self.calls.len() >= MAX_IDENTITIES {
                    return Err("resource_exhausted: provider call tombstones full".into());
                }
                self.calls.insert(id, payload.clone());
            }
            _ => {}
        }
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn event(kind: &str, revision: u64) -> Value {
        json!({"kind":kind,"payload":{"utterance":"a".repeat(32),"revision":revision,"position_ms":10}})
    }
    #[test]
    fn revisions_and_end_fence_stale_text_without_reopening() {
        let mut state = SpeechTemporal::default();
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.partial", 1))
                .is_err()
        );
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.start", 0))
                .unwrap()
        );
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.partial", 1))
                .unwrap()
        );
        assert!(
            !state
                .admit("surface", "owner", 1, &event("speech.partial", 1))
                .unwrap()
        );
        assert!(
            !state
                .admit("surface", "owner", 1, &event("speech.end", 1))
                .unwrap()
        );
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.end", 2))
                .unwrap()
        );
        for kind in ["speech.partial", "speech.end"] {
            assert!(!state.admit("surface", "owner", 1, &event(kind, 3)).unwrap());
        }
        let raw = serde_json::to_vec(&state).unwrap();
        let mut recovered: SpeechTemporal = serde_json::from_slice(&raw).unwrap();
        assert!(
            !recovered
                .admit("surface", "owner", 1, &event("speech.partial", 9))
                .unwrap()
        );
        assert!(
            recovered
                .admit("surface", "owner", 2, &event("speech.partial", 9))
                .is_err()
        );
        assert!(
            recovered
                .admit("surface", "new-owner", 2, &event("speech.start", 0))
                .is_err()
        );
    }
    #[test]
    fn capture_position_regression_is_not_an_end() {
        let mut state = SpeechTemporal::default();
        state
            .admit("surface", "owner", 1, &event("speech.start", 0))
            .unwrap();
        let mut end = event("speech.end", 1);
        end["payload"]["position_ms"] = json!(9);
        assert!(state.admit("surface", "owner", 1, &end).is_err());
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.end", 1))
                .unwrap()
        );
    }
    #[test]
    fn provider_calls_are_deduplicated_not_reexecuted_or_revised() {
        let mut state = SpeechTemporal::default();
        let mut call = json!({"kind":"speech.tool_call","payload":{"session":"s","call":"c","name":"tool","arguments":{}}});
        assert!(state.admit("surface", "owner", 1, &call).unwrap());
        assert!(!state.admit("surface", "owner", 1, &call).unwrap());
        call["payload"]["name"] = json!("other");
        assert!(state.admit("surface", "owner", 1, &call).is_err());
        assert!(state.admit("other-surface", "owner", 1, &call).unwrap());
    }
    #[test]
    fn identity_bounds_do_not_evict_tombstones() {
        let mut state = SpeechTemporal::default();
        for n in 0..MAX_IDENTITIES {
            let mut start = event("speech.start", 0);
            start["payload"]["utterance"] = json!(format!("{n:032x}"));
            state.admit("surface", "owner", 1, &start).unwrap();
        }
        assert!(
            state
                .admit("surface", "owner", 1, &event("speech.start", 0))
                .is_err()
        );
        let mut first = event("speech.end", 1);
        first["payload"]["utterance"] = json!(format!("{:032x}", 0));
        assert!(state.admit("surface", "owner", 1, &first).unwrap());
    }
}
