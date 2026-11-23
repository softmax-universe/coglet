//! Trusted supervisor configuration: identity binding, operator grants and limits.
//!
//! Nothing in here is read from the wire. A claimed identity on the wire is never
//! authentication: the supervisor binds one surface key and a set of permitted coglet keys,
//! and the transport verifies the runtime peer's OS credentials (see `transport`).

use crate::catalog::{Direction, catalog};
use crate::error::ApiError;
use crate::ids::{is_capability_name, is_coglet_id, is_correlation_id, is_surface_id};
use serde_json::{Value as Json, json};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_DEADLINE_AHEAD_US: u64 = 60_000_000;
pub const HEALTH_INTERVAL_US: u64 = 1_000_000;
pub const SILENCE_TIMEOUT_US: u64 = 3_000_000;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantSpec {
    pub coglet: String,
    pub operations: Vec<String>,
    pub resources: Vec<String>,
    /// Exclusive expiry bound on the surface clock.
    pub expires: u64,
}

impl GrantSpec {
    pub fn to_json(&self) -> Json {
        let mut ops = self.operations.clone();
        ops.sort();
        ops.dedup();
        let mut res = self.resources.clone();
        res.sort();
        res.dedup();
        json!({"coglet": self.coglet, "operations": ops, "resources": res, "expires": self.expires})
    }
}

#[derive(Clone, Debug)]
pub struct Limits {
    /// Admitted-but-not-terminal actions.
    pub max_pending: usize,
    /// Dedup ledger entries (rejections included); beyond this new work is refused.
    pub max_ledger: usize,
    pub max_deadline_ahead_us: u64,
    pub health_interval_us: u64,
    pub silence_timeout_us: u64,
    /// Idle unfinalized speak stream closes after this long without an admitted chunk.
    pub stream_idle_us: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_pending: 64,
            max_ledger: 100_000,
            max_deadline_ahead_us: MAX_DEADLINE_AHEAD_US,
            health_interval_us: HEALTH_INTERVAL_US,
            silence_timeout_us: SILENCE_TIMEOUT_US,
            stream_idle_us: 60_000_000,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub surface: String,
    /// Permitted coglet keys (the trusted binding). Anything else is `permission_denied`.
    pub bound_coglets: Vec<String>,
    /// Supervisor-configured runtime UID the transport must verify (informational here).
    pub runtime_uid: Option<u32>,
    /// Version offer; only "0.1" is supported by this SDK.
    pub versions: Vec<String>,
    /// Surface clock reading at `start()` (microseconds).
    pub start_clock: u64,
    /// Fixed hello correlation id (fixtures); `None` allocates a random one.
    pub hello_correlation: Option<String>,
    /// Operator/hub-policy grants, intersected with what the backend implements.
    pub grants: Vec<GrantSpec>,
    /// Lowest epoch to publish on the first negotiation (the ledger may force higher).
    pub initial_grant_epoch: u64,
    pub limits: Limits,
}

impl Config {
    pub fn new(surface: &str) -> Self {
        Self {
            surface: surface.to_owned(),
            bound_coglets: Vec::new(),
            runtime_uid: None,
            versions: vec!["0.1".into()],
            start_clock: 0,
            hello_correlation: None,
            grants: Vec::new(),
            initial_grant_epoch: 1,
            limits: Limits::default(),
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if !is_surface_id(&self.surface) {
            return Err("surface id violates grammar".into());
        }
        if self.versions != ["0.1"] {
            return Err("only protocol 0.1 is supported".into());
        }
        if let Some(c) = &self.hello_correlation
            && !is_correlation_id(c)
        {
            return Err("hello correlation id violates grammar".into());
        }
        if self.initial_grant_epoch == 0 {
            return Err("grant epoch starts at 1".into());
        }
        for c in &self.bound_coglets {
            if !is_coglet_id(c) {
                return Err(format!("bound coglet {c:?} violates grammar"));
            }
        }
        if self.limits.max_pending == 0 || self.limits.max_ledger == 0 {
            return Err("limits must be positive".into());
        }
        // Operator grants must be well formed and bound. Operations the backend lacks are
        // dropped (and reported) later; unknown or malformed names are config errors.
        check_grants(&self.grants, None, &self.bound_coglets).map_err(|e| e.message)?;
        Ok(())
    }
}

/// Validate a full grant snapshot. `now` enables the expiry check (expired entries are
/// rejected, atomically, never silently accepted).
pub fn check_grants(
    entries: &[GrantSpec],
    now: Option<u64>,
    bound: &[String],
) -> Result<(), ApiError> {
    let cat = catalog().map_err(ApiError::internal)?;
    let mut seen_coglets = BTreeSet::new();
    let mut owners: BTreeMap<&str, &str> = BTreeMap::new();
    for e in entries {
        if !is_coglet_id(&e.coglet) || !bound.contains(&e.coglet) {
            return Err(ApiError::denied(
                "grant names a coglet outside the trusted binding",
            ));
        }
        if !seen_coglets.insert(e.coglet.as_str()) {
            return Err(ApiError::conflict("duplicate grant entry for coglet"));
        }
        if let Some(now) = now
            && e.expires <= now
        {
            return Err(ApiError::invalid("grant entry already expired"));
        }
        for op in &e.operations {
            let known = cat
                .op(op)
                .is_some_and(|o| o.direction != Direction::Gateway && op != "action.outcome");
            if !is_capability_name(op) || !known {
                return Err(ApiError::invalid(format!(
                    "unknown operation {op:?} in grant"
                )));
            }
        }
        for r in &e.resources {
            if !is_capability_name(r) {
                return Err(ApiError::invalid("resource name violates grammar"));
            }
            if let Some(prev) = owners.insert(r.as_str(), e.coglet.as_str())
                && prev != e.coglet
            {
                return Err(ApiError::conflict(format!("resource {r} has two owners")));
            }
        }
    }
    // Duplex requires one owner of mic and speaker (speech.md); split ownership would
    // leak barge-in metadata across owners.
    if let (Some(m), Some(s)) = (owners.get("audio.mic"), owners.get("audio.speaker"))
        && m != s
    {
        return Err(ApiError::conflict(
            "audio.mic and audio.speaker need the same owner",
        ));
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpeechConfig {
    pub stt_placement: String,
    pub stt_route: String,
    pub tts_placement: String,
    pub tts_route: String,
    /// `stop` or `duck`
    pub barge: String,
    /// `text` or `speech_to_speech`
    pub session_mode: String,
}

impl SpeechConfig {
    pub fn to_json(&self) -> Json {
        json!({
            "stt": {"placement": self.stt_placement, "route": self.stt_route},
            "tts": {"placement": self.tts_placement, "route": self.tts_route},
            "barge": self.barge,
            "session_mode": self.session_mode,
        })
    }
    pub fn from_json(j: &Json) -> Result<Self, String> {
        crate::catalog::validate_rule("speech:speech-config", j)?;
        let s = |p: &str| {
            j.pointer(p)
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned()
        };
        Ok(Self {
            stt_placement: s("/stt/placement"),
            stt_route: s("/stt/route"),
            tts_placement: s("/tts/placement"),
            tts_route: s("/tts/route"),
            barge: s("/barge"),
            session_mode: s("/session_mode"),
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExpressionState {
    pub expression: String,
    pub intensity: u64,
    pub remaining_ms: u64,
}

impl Default for ExpressionState {
    fn default() -> Self {
        Self {
            expression: "neutral".into(),
            intensity: 0,
            remaining_ms: 0,
        }
    }
}

/// Parsed `spec/fixtures/surface/adapter-config.json`.
#[derive(Clone, Debug)]
pub struct AdapterFixture {
    pub config: Config,
    pub speech: SpeechConfig,
    pub expression: ExpressionState,
}

impl AdapterFixture {
    /// Build the trusted binding from the public fixture. `case` selects
    /// `startup_epoch_by_case`. The fixture's grant coglets form the trusted binding;
    /// any other coglet (for example in per-coglet-denial) is unbound.
    pub fn parse(src: &str, case: Option<&str>) -> Result<Self, String> {
        let j: Json = serde_json::from_str(src).map_err(|e| format!("fixture: {e}"))?;
        let u = |p: &str| {
            j.pointer(p)
                .and_then(Json::as_u64)
                .ok_or(format!("fixture: {p}"))
        };
        let st = |p: &str| {
            j.pointer(p)
                .and_then(Json::as_str)
                .map(str::to_owned)
                .ok_or(format!("fixture: {p}"))
        };
        let mut cfg = Config::new(&st("/surface")?);
        cfg.start_clock = u("/clock")?;
        cfg.hello_correlation = Some(st("/hello/body/correlation_id")?);
        cfg.initial_grant_epoch = u("/grant/epoch")?;
        if let Some(case) = case
            && let Some(e) = j
                .pointer("/startup_epoch_by_case")
                .and_then(|m| m.get(case))
        {
            cfg.initial_grant_epoch = e.as_u64().ok_or("fixture: startup epoch")?;
        }
        for g in j
            .pointer("/grant/entries")
            .and_then(Json::as_array)
            .ok_or("fixture: grant entries")?
        {
            let list = |k: &str| -> Result<Vec<String>, String> {
                g.get(k)
                    .and_then(Json::as_array)
                    .ok_or(format!("fixture: {k}"))?
                    .iter()
                    .map(|x| x.as_str().map(str::to_owned).ok_or(format!("fixture: {k}")))
                    .collect()
            };
            let coglet = g["coglet"].as_str().ok_or("fixture: coglet")?.to_owned();
            if !cfg.bound_coglets.contains(&coglet) {
                cfg.bound_coglets.push(coglet.clone());
            }
            cfg.grants.push(GrantSpec {
                coglet,
                operations: list("operations")?,
                resources: list("resources")?,
                expires: g["expires"].as_u64().ok_or("fixture: expires")?,
            });
        }
        cfg.validate()?;
        let speech =
            SpeechConfig::from_json(j.get("speech_config").ok_or("fixture: speech_config")?)?;
        let ie = j
            .get("initial_expression")
            .ok_or("fixture: initial_expression")?;
        crate::catalog::validate_rule("expression:expression-state", ie)?;
        let expression = ExpressionState {
            expression: ie["expression"].as_str().unwrap_or_default().to_owned(),
            intensity: ie["intensity"].as_u64().unwrap_or(0),
            remaining_ms: ie["remaining_ms"].as_u64().unwrap_or(0),
        };
        Ok(Self {
            config: cfg,
            speech,
            expression,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &str = include_str!("../../../spec/fixtures/surface/adapter-config.json");

    #[test]
    fn parses_public_fixture() {
        let f = AdapterFixture::parse(FIX, None).unwrap();
        assert_eq!(f.config.initial_grant_epoch, 1);
        assert_eq!(f.config.bound_coglets.len(), 1);
        assert_eq!(f.config.grants[0].operations.len(), 15);
        let f = AdapterFixture::parse(FIX, Some("grant-epoch-fence")).unwrap();
        assert_eq!(f.config.initial_grant_epoch, 2);
        assert_eq!(f.speech.session_mode, "text");
    }

    fn base() -> Config {
        let c = format!("coglet:{}", "b".repeat(64));
        let mut cfg = Config::new(&format!("surface:{}", "a".repeat(64)));
        cfg.bound_coglets.push(c.clone());
        cfg.grants.push(GrantSpec {
            coglet: c,
            operations: vec!["speech.stop".into()],
            resources: vec!["audio.speaker".into()],
            expires: 10,
        });
        cfg
    }

    #[test]
    fn invalid_configs_are_rejected() {
        base().validate().unwrap();
        let mut c = base();
        c.grants[0].operations.push("runtime.inbox".into());
        assert!(c.validate().is_err(), "reserved name cannot be granted");
        let mut c = base();
        c.grants[0].operations.push("test.echo".into());
        assert!(
            c.validate().is_err(),
            "illustrative names are not production ops"
        );
        let mut c = base();
        c.grants[0].operations.push("llm.complete".into());
        assert!(c.validate().is_err(), "gateway op is not a surface grant");
        let mut c = base();
        c.grants[0].coglet = format!("coglet:{}", "c".repeat(64));
        assert!(c.validate().is_err(), "grant outside binding");
        let mut c = base();
        c.versions = vec!["0.1".into(), "0.2".into()];
        assert!(c.validate().is_err());
        let mut c = base();
        c.surface = "surface:xyz".into();
        assert!(c.validate().is_err());
    }

    #[test]
    fn grant_snapshot_rules() {
        let a = format!("coglet:{}", "b".repeat(64));
        let b = format!("coglet:{}", "c".repeat(64));
        let bound = vec![a.clone(), b.clone()];
        let g = |c: &str, res: &[&str], exp| GrantSpec {
            coglet: c.into(),
            operations: vec!["speech.stop".into()],
            resources: res.iter().map(|s| s.to_string()).collect(),
            expires: exp,
        };
        check_grants(&[g(&a, &["audio.speaker"], 5)], Some(4), &bound).unwrap();
        // expired (exclusive bound), duplicate owner, duplicate entry, split duplex
        assert!(check_grants(&[g(&a, &[], 5)], Some(5), &bound).is_err());
        assert!(
            check_grants(
                &[g(&a, &["audio.speaker"], 9), g(&b, &["audio.speaker"], 9)],
                Some(1),
                &bound
            )
            .is_err()
        );
        assert!(check_grants(&[g(&a, &[], 9), g(&a, &[], 9)], Some(1), &bound).is_err());
        assert!(
            check_grants(
                &[g(&a, &["audio.mic"], 9), g(&b, &["audio.speaker"], 9)],
                Some(1),
                &bound
            )
            .is_err()
        );
        check_grants(
            &[g(&a, &["audio.mic", "audio.speaker"], 9)],
            Some(1),
            &bound,
        )
        .unwrap();
    }
}
