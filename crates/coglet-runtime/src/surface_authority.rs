//! Authority established only by authenticated, negotiated surface messages.
use super::*;
use crate::catalog;

impl SurfaceSession {
    /// A live endpoint's epoch high-water mark belongs to the durable runtime,
    /// not a transport connection. Probe-only new() sessions have no persistence.
    pub fn with_service(
        binding: SurfaceBinding,
        service: std::sync::Arc<std::sync::Mutex<crate::RuntimeService>>,
    ) -> Result<Self> {
        let epoch = service
            .lock()
            .map_err(|_| "runtime service poisoned")?
            .surface_epoch(binding.surface());
        let mut session = Self::new(binding);
        session.epoch = epoch;
        session.service = Some(service);
        Ok(session)
    }
    pub(crate) fn check_reconciliation_binding(&self, coglet: &str, now: u64) -> Result<()> {
        if self.closed
            || !self.negotiated
            || now < self.runtime_clock
            || now.saturating_sub(self.last_received) >= 3_000_000
            || !self.binding.permitted_coglets.iter().any(|c| c == coglet)
        {
            return Err("inactive or unbound reconciliation link".into());
        }
        Ok(())
    }
    pub fn surface(&self) -> &str {
        self.binding.surface()
    }
    pub fn grant_epoch(&self) -> u64 {
        self.epoch
    }
    pub(crate) fn event_lease(&self, coglet: &str, operation: &str, now: u64) -> Result<u64> {
        self.authorize(coglet, operation, "event", now)?;
        now.checked_add(
            self.grants[coglet]["expires"]
                .as_u64()
                .unwrap()
                .checked_sub(self.surface_now(now)?)
                .ok_or("expired grant")?,
        )
        .ok_or("grant expiry overflow".into())
    }
    pub(super) fn surface_now(&self, now: u64) -> Result<u64> {
        if self.closed
            || !self.negotiated
            || now < self.runtime_clock
            || now.saturating_sub(self.last_received) >= 3_000_000
        {
            return Err("inactive surface link".into());
        }
        let (runtime, surface) = self.mapping.ok_or("missing surface clock mapping")?;
        surface
            .checked_add(now.checked_sub(runtime).ok_or("runtime clock rollback")?)
            .ok_or("surface clock overflow".into())
    }
    pub(super) fn install_capabilities(&mut self, body: &Value, now: u64) -> Result<()> {
        let revision = body["revision"].as_u64().ok_or("missing revision")?;
        let clock = body["clock"].as_u64().ok_or("missing clock")?;
        if revision <= self.revision || self.peer_clock.is_some_and(|old| clock < old) {
            return Err("capability revision/clock rollback".into());
        }
        let mut entries = BTreeSet::new();
        let mut operations = BTreeMap::new();
        let mut resources = BTreeSet::new();
        for entry in body["entries"].as_array().ok_or("missing entries")? {
            let name = entry["name"].as_str().ok_or("missing entry name")?;
            if matches!(name, "sim.gridbot" | "sim.cartpole")
                && self.binding.permitted_coglets.len() != 1
            {
                return Err("sandbox requires exactly one supervisor-bound pilot".into());
            }
            if !entries.insert(name) {
                return Err("duplicate capability entry".into());
            }
            let mut advertised = BTreeSet::new();
            for (field, direction) in [
                ("events", "event"),
                ("reads", "read"),
                ("actions", "action"),
            ] {
                for value in entry[field].as_array().ok_or("missing operation list")? {
                    let name = value.as_str().ok_or("invalid operation")?;
                    // Unsupported operations never gain authority. Fail closed rather
                    // than falling back to generic profile-map schema validation.
                    let op = catalog::operation(name, direction)?;
                    if op["entry"] != entry["name"] {
                        return Err("wrong catalog entry".into());
                    }
                    let rule = if direction == "read" {
                        &op["output"]
                    } else {
                        &op["input"]
                    };
                    if entry["schemas"][name] != catalog::uri(rule)? {
                        return Err("untrusted schema URI".into());
                    }
                    if !advertised.insert(name.to_owned())
                        || operations
                            .insert(name.to_owned(), direction.to_owned())
                            .is_some()
                    {
                        return Err("duplicate operation".into());
                    }
                }
            }
            if entry["schemas"]
                .as_object()
                .ok_or("missing schemas")?
                .keys()
                .cloned()
                .collect::<BTreeSet<_>>()
                != advertised
            {
                return Err("schema keys differ from operations".into());
            }
            for resource in entry["exclusive"].as_array().ok_or("missing resources")? {
                let resource = resource.as_str().ok_or("invalid resource")?;
                let known = match name {
                    "expression.control" => resource == "display.face",
                    "speech.control" | "speech.session" => {
                        matches!(resource, "audio.mic" | "audio.speaker")
                    }
                    _ => false,
                };
                if !known || !resources.insert(resource.to_owned()) {
                    return Err("unsupported/duplicate exclusive resource".into());
                }
            }
        }
        // Removal is legal only after the authoritative revoking grant snapshot.
        for grant in self.grants.values() {
            if grant["operations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|v| !operations.contains_key(v.as_str().unwrap()))
                || grant["resources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| !resources.contains(v.as_str().unwrap()))
            {
                return Err("capability removal before grant revocation".into());
            }
        }
        self.revision = revision;
        self.operations = operations;
        self.resources = resources;
        self.mapping = Some((now, clock));
        self.peer_clock = Some(clock);
        Ok(())
    }
    pub(super) fn install_grants(&mut self, body: &Value, now: u64) -> Result<()> {
        let epoch = body["epoch"].as_u64().ok_or("missing epoch")?;
        let clock = self.surface_now(now)?;
        if self.revision == 0 || epoch <= self.epoch {
            return Err("grant epoch rollback or missing capabilities".into());
        }
        let mut grants = BTreeMap::new();
        let mut owners = BTreeSet::new();
        for grant in body["entries"].as_array().ok_or("missing entries")? {
            let coglet = grant["coglet"].as_str().ok_or("missing coglet")?;
            if !self.binding.permitted_coglets.iter().any(|c| c == coglet)
                || grant["expires"].as_u64().ok_or("missing expiry")? <= clock
                || grants.insert(coglet.to_owned(), grant.clone()).is_some()
            {
                return Err("unbound, duplicate or expired grant".into());
            }
            let mut ops = BTreeSet::new();
            for value in grant["operations"].as_array().ok_or("missing operations")? {
                let op = value.as_str().ok_or("invalid operation")?;
                if !self.operations.contains_key(op) || !ops.insert(op) {
                    return Err("unknown/duplicate grant operation".into());
                }
            }
            for value in grant["resources"].as_array().ok_or("missing resources")? {
                let resource = value.as_str().ok_or("invalid resource")?;
                if !self.resources.contains(resource) || !owners.insert(resource) {
                    return Err("unknown resource or duplicate owner".into());
                }
            }
        }
        if let Some(service) = &self.service {
            service
                .lock()
                .map_err(|_| "runtime service poisoned")?
                .record_surface_epoch(self.surface(), epoch)?;
        }
        self.epoch = epoch;
        self.grants = if self.killed { BTreeMap::new() } else { grants };
        // Local readiness is authority, not the peer's health claim.
        self.stopped = self.killed || self.grants.is_empty();
        self.expire_reads(now);
        Ok(())
    }
    #[cfg(feature = "native-engine")]
    pub(crate) fn execution_grants(
        &self,
        manifest: &Value,
        now: u64,
    ) -> Result<Option<Vec<String>>> {
        if self.stopped || self.killed || self.surface_now(now).is_err() {
            return Ok(None);
        }
        let coglet = manifest["id"].as_str().ok_or("manifest id")?;
        let Some(grant) = self.grants.get(coglet) else {
            return Ok(None);
        };
        let clock = self.surface_now(now)?;
        if grant["expires"].as_u64().ok_or("grant expiry")? <= clock {
            return Ok(None);
        }
        for resource in manifest["exclusive_outputs"]
            .as_array()
            .ok_or("exclusive outputs")?
        {
            if !grant["resources"]
                .as_array()
                .ok_or("resources")?
                .contains(resource)
            {
                return Ok(None);
            }
        }
        let allowed: Vec<String> = self
            .operations
            .iter()
            .filter_map(|(op, direction)| {
                self.authorize(coglet, op, direction, now)
                    .ok()
                    .map(|_| op.clone())
            })
            .collect();
        for op in manifest["required_capabilities"]
            .as_array()
            .ok_or("required capabilities")?
            .iter()
            .chain(
                manifest["subscriptions"]
                    .as_array()
                    .ok_or("subscriptions")?,
            )
        {
            if !allowed.iter().any(|name| op == name) {
                return Ok(None);
            }
        }
        Ok(Some(allowed))
    }
    pub(crate) fn authorize(
        &self,
        coglet: &str,
        operation: &str,
        direction: &str,
        now: u64,
    ) -> Result<()> {
        let clock = self.surface_now(now)?;
        if self.stopped
            || self.killed
            || self.operations.get(operation).map(String::as_str) != Some(direction)
        {
            return Err("permission_denied: unavailable operation".into());
        }
        let grant = self
            .grants
            .get(coglet)
            .ok_or("permission_denied: missing grant")?;
        if grant["expires"].as_u64().unwrap() <= clock
            || !grant["operations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|op| op == operation)
            || catalog::resources(operation).iter().any(|resource| {
                !grant["resources"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|r| r == resource)
            })
        {
            return Err("permission_denied: expired or insufficient grant".into());
        }
        Ok(())
    }
    /// Calculate a NEW envelope only. Durable dispatch owns retry bytes; callers
    /// must not use this function to reconstruct an uncertain historical request.
    pub fn translate_action(&self, action: &Value, now: u64) -> Result<Value> {
        validate("action", action)?;
        let op = action["capability"].as_str().ok_or("missing capability")?;
        self.authorize(action["coglet"].as_str().unwrap(), op, "action", now)?;
        catalog::check(&catalog::operation(op, "action")?["input"], &action["args"])?;
        if op == "speech.speak_chunk"
            && action["args"]["text"]
                .as_str()
                .is_some_and(|s| s.len() > 4_096)
        {
            return Err("speech text exceeds UTF-8 byte limit".into());
        }
        if op == "speech.work_result" && cbor(&action["args"]["value"])?.len() > 32_000 {
            return Err("speech work result exceeds encoded byte limit".into());
        }
        let remaining = action["deadline"]
            .as_u64()
            .unwrap()
            .checked_sub(now)
            .filter(|v| *v > 0)
            .ok_or("deadline_exceeded")?;
        if remaining > 60_000_000 {
            return Err("invalid_argument: deadline exceeds 60 seconds".into());
        }
        let deadline = self
            .surface_now(now)?
            .checked_add(remaining)
            .ok_or("deadline overflow")?;
        let mut mapped = action.clone();
        mapped["deadline"] = json!(deadline);
        Ok(json!({"type":"act","body":{"grant_epoch":self.epoch,"action":mapped}}))
    }
}
