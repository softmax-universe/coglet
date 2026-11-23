//! Durable defense for authenticated playback observations (not native audio).
use super::*;

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Playback {
    items: BTreeMap<String, Item>,
    streams: BTreeMap<String, Stream>,
    barges: BTreeMap<String, Value>,
    #[serde(default)]
    snapshots: BTreeMap<String, u64>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Stream {
    pair: Option<Value>,
    owner: String,
    epoch: u64,
    provider: Option<String>,
    position: u64,
    stopped: bool,
    chunks: BTreeMap<u64, String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    owner: String,
    epoch: u64,
    provider: Option<String>,
    stream: String,
    chunk: Option<u64>,
    played: u64,
    position: u64,
    state: String,
    kind: String,
}
impl Playback {
    pub(super) fn observe_snapshot(
        &mut self,
        surface: &str,
        item: &str,
        snapshot: u64,
    ) -> Result<bool> {
        let key = format!("{surface}/{item}");
        if !self.items.contains_key(&key) {
            return Err("snapshot without playback binding".into());
        }
        if self.snapshots.get(&key).is_some_and(|old| snapshot < *old) {
            return Ok(false);
        }
        self.snapshots.insert(key, snapshot);
        Ok(true)
    }
    pub(super) fn admit(
        &mut self,
        surface: &str,
        owner: &str,
        epoch: u64,
        provider: Option<String>,
        event: &Value,
    ) -> Result<bool> {
        let p = &event["payload"];
        let id = p["item"].as_str().ok_or("missing playback item")?;
        let stream = p["stream"].as_str().ok_or("missing playback stream")?;
        let played = p["played_ms"].as_u64().ok_or("missing played position")?;
        let position = p["position_ms"].as_u64().ok_or("missing stream position")?;
        let state = p["state"]
            .as_str()
            .or_else(|| p["reflex"].as_str())
            .ok_or("missing playback state")?;
        let key = format!("{surface}/{id}");
        let stream_key = format!("{surface}/{stream}");
        if self.duplicate_barge(surface, event)? {
            return Ok(false);
        }
        let chunk = p["chunk"].as_u64();
        if self.items.get(&key).is_some_and(|old| {
            old.owner == owner
                && old.epoch == epoch
                && old.provider == provider
                && old.stream == stream
                && chunk.zip(old.chunk).is_none_or(|(a, b)| a == b)
                && old.played == played
                && old.position == position
                && old.state == state
                && event["kind"] == old.kind
        }) {
            // A prior item's exact terminal report remains harmless after the
            // stream has progressed to a later chunk. Do not roll back highwater.
            return Ok(false);
        }
        if let Some(old) = self.streams.get(&stream_key) {
            if event["kind"] == "speech.position"
                && let Some(pair) = &old.pair
                && (pair["item"] != p["item"]
                    || pair["played_ms"] != p["played_ms"]
                    || pair["position_ms"] != p["position_ms"]
                    || pair["reflex"] != p["state"])
            {
                return Err("barge position does not match applied reflex".into());
            }
            if old.owner != owner
                || old.epoch != epoch
                || old.provider != provider
                || position < old.position
                || (old.stopped && !self.items.contains_key(&key))
                || chunk.is_some_and(|c| old.chunks.get(&c).is_some_and(|item| item != id))
            {
                return Err("playback stream owner/chunk/position conflict".into());
            }
        }
        if chunk.is_some_and(|c| c >= 64) {
            return Err("playback chunk bound".into());
        }
        let mut duplicate = false;
        if let Some(old) = self.items.get(&key) {
            if old.owner != owner
                || old.epoch != epoch
                || old.provider != provider
                || old.stream != stream
                || chunk.zip(old.chunk).is_some_and(|(a, b)| a != b)
            {
                return Err("playback item binding changed".into());
            }
            if played < old.played
                || (matches!(old.state.as_str(), "stopped" | "finished")
                    && (played != old.played || state != old.state))
            {
                return Err("playback item regressed or resumed after stop".into());
            }
            duplicate = played == old.played
                && position == old.position
                && state == old.state
                && event["kind"] == old.kind;
        } else if self.items.len() >= 4096
            || (!self.streams.contains_key(&stream_key) && self.streams.len() >= 4096)
        {
            return Err("resource_exhausted: playback identities".into());
        }
        if event["kind"] == "speech.barge_in" {
            let barge_key = format!(
                "{surface}/{}",
                p["utterance"].as_str().ok_or("missing barge utterance")?
            );
            if let Some(old) = self.barges.get(&barge_key) {
                if old != p {
                    return Err("conflicting barge pair".into());
                }
                return Ok(false);
            }
            if self.barges.len() >= 4096 {
                return Err("resource_exhausted: barge identities".into());
            }
            self.barges.insert(barge_key, p.clone());
            // A barge is a distinct observation even if position arrived first.
            duplicate = false;
        }
        let old_chunk = self.items.get(&key).and_then(|i| i.chunk);
        let s = self.streams.entry(stream_key).or_insert_with(|| Stream {
            pair: None,
            owner: owner.into(),
            epoch,
            provider: provider.clone(),
            position: 0,
            stopped: false,
            chunks: BTreeMap::new(),
        });
        if let Some(chunk) = chunk.or(old_chunk) {
            s.chunks.insert(chunk, id.into());
        }
        s.position = position;
        if event["kind"] == "speech.barge_in" {
            s.pair = Some(p.clone());
        } else {
            s.pair = None;
        }
        s.stopped |= state == "stopped";
        self.items.insert(
            key,
            Item {
                owner: owner.into(),
                epoch,
                provider,
                stream: stream.into(),
                chunk: chunk.or(old_chunk),
                played,
                position,
                state: state.into(),
                kind: event["kind"].as_str().unwrap().into(),
            },
        );
        Ok(!duplicate)
    }
    pub(super) fn duplicate_barge(&self, surface: &str, event: &Value) -> Result<bool> {
        if event["kind"] != "speech.barge_in" {
            return Ok(false);
        }
        let p = &event["payload"];
        let key = format!(
            "{surface}/{}",
            p["utterance"].as_str().ok_or("missing barge utterance")?
        );
        match self.barges.get(&key) {
            Some(old) if old == p => Ok(true),
            Some(_) => Err("conflicting barge pair".into()),
            None => Ok(false),
        }
    }
    pub(super) fn provider(&self, surface: &str, item: &str) -> Option<&str> {
        self.items
            .get(&format!("{surface}/{item}"))
            .and_then(|i| i.provider.as_deref())
    }
    pub(super) fn check_provider_owner(
        &self,
        surface: &str,
        item: &str,
        owner: &str,
        epoch: u64,
    ) -> Result<()> {
        let item = self
            .items
            .get(&format!("{surface}/{item}"))
            .ok_or("missing native item")?;
        if item.owner != owner || item.epoch != epoch {
            return Err("native item owner revoked".into());
        }
        Ok(())
    }
    pub(super) fn validate(&self, epochs: &BTreeMap<String, u64>, owner: &Value) -> Result<()> {
        if self.snapshots.keys().any(|k| !self.items.contains_key(k)) {
            return Err("recovered snapshot without playback binding".into());
        }
        if self.items.len() > 4096 || self.streams.len() > 4096 || self.barges.len() > 4096 {
            return Err("recovered playback limit".into());
        }
        for (key, item) in &self.items {
            let (surface, id) = key.split_once('/').ok_or("invalid playback key")?;
            validate("surface-id", &json!(surface))?;
            validate("correlation-id", &json!(id))?;
            validate("correlation-id", &json!(item.stream))?;
            if let Some(provider) = &item.provider {
                validate("correlation-id", &json!(provider))?;
            }
            if owner != &item.owner
                || item.epoch == 0
                || epochs.get(surface).is_none_or(|e| *e < item.epoch)
                || !matches!(
                    item.state.as_str(),
                    "playing" | "ducked" | "stopped" | "finished"
                )
                || self
                    .streams
                    .get(&format!("{surface}/{}", item.stream))
                    .is_none_or(|s| {
                        s.position < item.position
                            || s.owner != item.owner
                            || s.epoch != item.epoch
                            || s.provider != item.provider
                            || item
                                .chunk
                                .is_some_and(|c| s.chunks.get(&c).is_none_or(|v| v != id))
                    })
            {
                return Err("invalid recovered playback binding".into());
            }
        }
        for (key, s) in &self.streams {
            let (surface, stream) = key.split_once('/').ok_or("invalid stream key")?;
            if let Some(pair) = &s.pair {
                crate::catalog::check(
                    &crate::catalog::operation("speech.barge_in", "event")?["input"],
                    pair,
                )?;
                if pair["stream"] != stream
                    || self.barges.get(&format!(
                        "{surface}/{}",
                        pair["utterance"].as_str().unwrap()
                    )) != Some(pair)
                {
                    return Err("invalid recovered barge pair".into());
                }
            }
            validate("surface-id", &json!(surface))?;
            validate("correlation-id", &json!(stream))?;
            if s.chunks.len() > 64
                || s.chunks.keys().any(|c| *c >= 64)
                || !self
                    .items
                    .iter()
                    .any(|(k, i)| k.starts_with(&format!("{surface}/")) && i.stream == stream)
            {
                return Err("invalid recovered stream members".into());
            }
            for (chunk, id) in &s.chunks {
                if self
                    .items
                    .get(&format!("{surface}/{id}"))
                    .is_none_or(|i| i.chunk != Some(*chunk) || i.stream != stream)
                {
                    return Err("invalid recovered chunk binding".into());
                }
            }
        }
        for (key, p) in &self.barges {
            let (surface, utterance) = key.split_once('/').ok_or("invalid barge key")?;
            validate("surface-id", &json!(surface))?;
            validate("correlation-id", &json!(utterance))?;
            crate::catalog::check(
                &crate::catalog::operation("speech.barge_in", "event")?["input"],
                p,
            )?;
            if p["utterance"] != utterance
                || !self
                    .items
                    .contains_key(&format!("{surface}/{}", p["item"].as_str().unwrap()))
            {
                return Err("invalid recovered barge identity".into());
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn read_snapshot_highwater_survives_progress_and_restart() {
        let mut s = Playback::default();
        assert!(s.observe_snapshot("s", &"4".repeat(32), 2).is_err());
        s.admit("s", "c", 1, None, &event(10, 10, "playing"))
            .unwrap();
        assert!(s.observe_snapshot("s", &"4".repeat(32), 2).unwrap());
        s.admit("s", "c", 1, None, &event(20, 20, "playing"))
            .unwrap();
        let mut s: Playback = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(!s.observe_snapshot("s", &"4".repeat(32), 1).unwrap());
        assert_eq!(s.snapshots.values().next(), Some(&2));
        assert!(s.observe_snapshot("s", &"4".repeat(32), 3).unwrap());
    }
    #[test]
    fn barge_pair_matches_exact_cutoff_and_replay_never_rolls_back_progress() {
        let mut s = Playback::default();
        s.admit("s", "c", 1, None, &event(10, 10, "playing"))
            .unwrap();
        let barge = json!({"kind":"speech.barge_in","payload":{"utterance":"c".repeat(32),"item":"4".repeat(32),"stream":"a".repeat(32),"played_ms":10,"position_ms":10,"reflex":"ducked"}});
        assert!(s.admit("s", "c", 1, None, &barge).unwrap());
        assert!(
            s.admit("s", "c", 1, None, &event(11, 11, "ducked"))
                .is_err()
        );
        assert!(
            s.admit("s", "c", 1, None, &event(10, 10, "ducked"))
                .unwrap()
        );
        s.admit("s", "c", 1, None, &event(20, 20, "playing"))
            .unwrap();
        assert!(!s.admit("s", "c", 1, None, &barge).unwrap());
        assert_eq!(s.items.values().next().unwrap().played, 20);
        let mut conflict = barge.clone();
        conflict["payload"]["played_ms"] = json!(11);
        assert!(s.admit("s", "c", 1, None, &conflict).is_err());
    }
    #[test]
    fn stream_chunk_and_provider_bindings_cannot_be_reassigned() {
        let mut s = Playback::default();
        s.admit(
            "s",
            "c",
            1,
            Some("provider".into()),
            &event(10, 10, "finished"),
        )
        .unwrap();
        let mut second = event(0, 10, "playing");
        second["payload"]["item"] = json!("5".repeat(32));
        assert!(
            s.admit("s", "c", 1, Some("provider".into()), &second)
                .is_err()
        );
        second["payload"]["chunk"] = json!(1);
        assert!(
            s.admit("s", "c", 1, Some("provider".into()), &second)
                .unwrap()
        );
        let mut progress = second.clone();
        progress["payload"]["played_ms"] = json!(10);
        progress["payload"]["position_ms"] = json!(20);
        s.admit("s", "c", 1, Some("provider".into()), &progress)
            .unwrap();
        assert!(
            !s.admit(
                "s",
                "c",
                1,
                Some("provider".into()),
                &event(10, 10, "finished")
            )
            .unwrap()
        );
        second["payload"]["item"] = json!("6".repeat(32));
        second["payload"]["chunk"] = json!(2);
        assert!(
            s.admit("s", "other", 1, Some("provider".into()), &second)
                .is_err()
        );
        assert!(
            s.admit("s", "c", 1, Some("new-provider".into()), &second)
                .is_err()
        );
        assert!(
            s.admit("s", "c", 2, Some("provider".into()), &second)
                .is_err()
        );
    }
    #[test]
    fn recovery_rejects_orphan_stream_and_malformed_barge() {
        let surface = format!("surface:{}", "b".repeat(64));
        let owner = json!(format!("coglet:{}", "a".repeat(64)));
        let mut s = Playback::default();
        s.admit(
            &surface,
            owner.as_str().unwrap(),
            1,
            None,
            &event(10, 10, "playing"),
        )
        .unwrap();
        let epochs = BTreeMap::from([(surface.clone(), 1)]);
        s.validate(&epochs, &owner).unwrap();
        let raw = serde_json::to_value(&s).unwrap();
        let mut bad = raw.clone();
        bad["streams"]["bad/key"] = bad["streams"]
            .as_object()
            .unwrap()
            .values()
            .next()
            .unwrap()
            .clone();
        assert!(
            serde_json::from_value::<Playback>(bad)
                .unwrap()
                .validate(&epochs, &owner)
                .is_err()
        );
        let mut bad = raw;
        bad["barges"][format!("{surface}/{}", "c".repeat(32))] = json!({});
        assert!(
            serde_json::from_value::<Playback>(bad)
                .unwrap()
                .validate(&epochs, &owner)
                .is_err()
        );
    }
    fn event(played: u64, position: u64, state: &str) -> Value {
        json!({"kind":"speech.position","payload":{"item":"4".repeat(32),"stream":"a".repeat(32),"chunk":0,"played_ms":played,"position_ms":position,"state":state}})
    }
    #[test]
    fn duplicate_duck_stop_and_restart_fences() {
        let mut s = Playback::default();
        assert!(
            s.admit("s", "c", 1, None, &event(10, 10, "playing"))
                .unwrap()
        );
        assert!(
            !s.admit("s", "c", 1, None, &event(10, 10, "playing"))
                .unwrap()
        );
        assert!(
            s.admit("s", "c", 1, None, &event(10, 10, "ducked"))
                .unwrap()
        );
        assert!(
            s.admit("s", "c", 1, None, &event(20, 20, "playing"))
                .unwrap()
        );
        assert!(
            s.admit("s", "c", 1, None, &event(20, 20, "stopped"))
                .unwrap()
        );
        let mut s: Playback = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert!(
            s.admit("s", "c", 1, None, &event(21, 21, "stopped"))
                .is_err()
        );
        assert!(
            s.admit("s", "c", 1, None, &event(20, 20, "playing"))
                .is_err()
        );
        assert!(
            s.admit("s", "c", 2, None, &event(20, 20, "stopped"))
                .is_err()
        );
        let mut changed = event(20, 20, "stopped");
        changed["payload"]["stream"] = json!("b".repeat(32));
        assert!(s.admit("s", "c", 1, None, &changed).is_err());
    }
}
