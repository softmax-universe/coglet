//! M3 local promotion components. Not registered as a hub wire method.
//!
//! Policy remains DRAFT until the freeze ADR, qualification and tag are accepted.
//! Parsing a policy does not install it or grant authority.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum GateKind {
    JournalReplay,
    HeadlessGame,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct GateRequirement {
    kind: GateKind,
    suite: String,
    max_age_ms: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ProbationLimits {
    window_ms: u64,
    health_interval_ms: u64,
    health_timeout_ms: u64,
    max_consecutive_failures: u64,
    max_step_failures: u64,
    max_budget_violations: u64,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PolicyDocument {
    version: String,
    status: String,
    coglet: String,
    revision: String,
    control_role: String,
    human_approval: bool,
    gates: Vec<GateRequirement>,
    probation: ProbationLimits,
}
/// A closed, semantically checked document. Cannot be constructed by deserialization.
#[derive(Clone, Debug)]
pub struct Policy(PolicyDocument);

/// Registry/build bindings supplied by the kernel, never trusted from a factory report.
#[derive(Clone, Debug)]
pub struct GateBinding {
    pub coglet: String,
    pub repo: String,
    pub candidate: String,
    pub base: Option<String>,
    pub policy_revision: String,
    pub wasm_sha256: String,
}
/// The existing draft promotion report shape. The evidence authority must resolve
/// and authenticate its immutable artifact, including the executable/repository binding.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GateReport {
    pub kind: GateKind,
    pub suite: String,
    pub candidate: String,
    pub base: Option<String>,
    pub policy_revision: String,
    pub checked_at_ms: u64,
    pub status: String,
    pub evidence_ref: String,
}
/// Trusted-local service boundary. No default implementation and no caller boolean.
/// Implementations must verify the authority and immutable artifact against every
/// report field and the kernel's complete binding; a reference alone is not proof.
pub trait GateAuthority {
    fn verify(&self, report: &GateReport, binding: &GateBinding) -> Result<()>;
}
/// Pure local decision, not a durable transition, authority check or output fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbationDecision {
    Observe,
    Stable,
    Rollback,
}
/// Unauthenticated input to the pure reducer. Production callers must verify its
/// source, epoch mapping and cumulative per-activation counters independently.
#[derive(Clone, Debug)]
pub struct ProbationSample {
    pub coglet: String,
    pub generation: u64,
    pub commit: String,
    pub observed_at_ms: u64,
    pub healthy: bool,
    pub step_failures: u64,
    pub budget_violations: u64,
}
/// Pinned decision state. Only the kernel may feed authenticated, clock-mapped
/// observations after a verified activation. No serialization/recovery API yet;
/// this component alone cannot establish durable probation or grant authority.
#[derive(Clone, Debug)]
pub struct Probation {
    coglet: String,
    limits: ProbationLimits,
    epoch: String,
    generation: u64,
    commit: String,
    started_at: u64,
    expires_at: u64,
    last_now: u64,
    last_sample: Option<(u64, bool)>,
    consecutive_failures: u64,
    step_failures: u64,
    budget_violations: u64,
    decision: ProbationDecision,
}
impl Probation {
    pub fn start(
        policy: &Policy,
        epoch: &str,
        now_ms: u64,
        generation: u64,
        commit: &str,
    ) -> Result<Self> {
        if epoch.is_empty() || generation == 0 || !lower_hex(commit, &[40, 64]) {
            return Err(Error::Invalid);
        }
        let expires_at = now_ms
            .checked_add(policy.0.probation.window_ms)
            .ok_or(Error::Invalid)?;
        Ok(Self {
            coglet: policy.0.coglet.clone(),
            limits: policy.0.probation.clone(),
            epoch: epoch.to_owned(),
            generation,
            commit: commit.to_owned(),
            started_at: now_ms,
            expires_at,
            last_now: now_ms,
            last_sample: None,
            consecutive_failures: 0,
            step_failures: 0,
            budget_violations: 0,
            decision: ProbationDecision::Observe,
        })
    }
    /// Fresh hub supervisor time is required; uncertain time latches rollback.
    fn advance(&mut self, epoch: &str, now_ms: u64) {
        if self.decision != ProbationDecision::Observe {
            return;
        }
        let health_at = self.last_sample.map_or(self.started_at, |s| s.0);
        if epoch != self.epoch
            || now_ms < self.last_now
            || now_ms - health_at >= self.limits.health_timeout_ms
        {
            self.decision = ProbationDecision::Rollback;
        }
        self.last_now = now_ms;
    }
    fn finish(&mut self, now_ms: u64) -> ProbationDecision {
        if self.decision == ProbationDecision::Observe && now_ms >= self.expires_at {
            self.decision = if self.last_sample.is_some_and(|s| s.1) {
                ProbationDecision::Stable
            } else {
                ProbationDecision::Rollback
            };
        }
        self.decision
    }
    pub fn check(&mut self, epoch: &str, now_ms: u64) -> ProbationDecision {
        self.advance(epoch, now_ms);
        self.finish(now_ms)
    }
    /// Inputs must be ordered hub observations, not peer wall timestamps. Callers
    /// serialize observation delivery with checks: a timestamp older than the last
    /// reducer call is conservatively dropped, even if delayed delivery was honest.
    /// Counters start at activation (zero baseline), not device lifetime. Rejected
    /// inputs never extend health freshness; time still advances and can expire it.
    pub fn observe(
        &mut self,
        epoch: &str,
        now_ms: u64,
        sample: &ProbationSample,
    ) -> ProbationDecision {
        let previous_now = self.last_now;
        self.advance(epoch, now_ms);
        if self.decision == ProbationDecision::Observe
            && sample.coglet == self.coglet
            && sample.generation == self.generation
            && sample.commit == self.commit
            && sample.observed_at_ms >= previous_now
            && sample.observed_at_ms <= now_ms
            && self.last_sample.is_none_or(|s| sample.observed_at_ms > s.0)
        {
            if sample.step_failures < self.step_failures
                || sample.budget_violations < self.budget_violations
            {
                self.decision = ProbationDecision::Rollback;
            } else {
                self.consecutive_failures = if sample.healthy {
                    0
                } else {
                    self.consecutive_failures.saturating_add(1)
                };
                self.step_failures = sample.step_failures;
                self.budget_violations = sample.budget_violations;
                self.last_sample = Some((sample.observed_at_ms, sample.healthy));
                if self.consecutive_failures >= self.limits.max_consecutive_failures
                    || self.step_failures >= self.limits.max_step_failures
                    || self.budget_violations >= self.limits.max_budget_violations
                {
                    self.decision = ProbationDecision::Rollback;
                }
            }
        }
        self.finish(now_ms)
    }
}
fn lower_hex(value: &str, lengths: &[usize]) -> bool {
    lengths.contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
impl Policy {
    /// Validate against the separately enrolled role; self-declared role is not authority.
    pub fn parse(value: &Value, enrolled_role: &str) -> Result<Self> {
        let doc: PolicyDocument =
            serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        if doc.version != "0.1"
            || doc.status != "DRAFT"
            || !super::valid("coglet-id", &Value::String(doc.coglet.clone()))
            || !super::valid("correlation-id", &Value::String(doc.revision.clone()))
            || doc.control_role != enrolled_role
            || !["application", "fast_loop", "factory"].contains(&enrolled_role)
            || (enrolled_role != "application" && !doc.human_approval)
            || doc.gates.is_empty()
            || doc.gates.len() > 2
        {
            return Err(Error::Invalid);
        }
        let mut kinds = std::collections::BTreeSet::new();
        for gate in &doc.gates {
            if gate.suite.is_empty() || gate.max_age_ms == 0 || !kinds.insert(gate.kind) {
                return Err(Error::Invalid);
            }
        }
        if !kinds.contains(&GateKind::JournalReplay) {
            return Err(Error::Invalid);
        }
        let p = &doc.probation;
        if p.health_interval_ms == 0
            || p.health_interval_ms > p.health_timeout_ms
            || p.health_timeout_ms > p.window_ms
            || p.max_consecutive_failures == 0
            || p.max_step_failures == 0
            || p.max_budget_violations == 0
        {
            return Err(Error::Invalid);
        }
        Ok(Self(doc))
    }
    /// Check shape/bindings/freshness and independently authenticate every required
    /// report. Call again at promotion admission with fresh trusted supervisor time.
    pub fn verify_reports(
        &self,
        value: &Value,
        binding: &GateBinding,
        now_ms: u64,
        authority: &dyn GateAuthority,
    ) -> Result<Vec<GateReport>> {
        if binding.coglet != self.0.coglet
            || binding.policy_revision != self.0.revision
            || binding.repo.is_empty()
            || !lower_hex(&binding.candidate, &[40, 64])
            || !lower_hex(&binding.wasm_sha256, &[64])
            || binding
                .base
                .as_ref()
                .is_some_and(|base| !lower_hex(base, &[binding.candidate.len()]))
        {
            return Err(Error::Invalid);
        }
        let reports: Vec<GateReport> =
            serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        if reports.len() != self.0.gates.len() {
            return Err(Error::Invalid);
        }
        let mut seen = std::collections::BTreeSet::new();
        for report in &reports {
            let required = self
                .0
                .gates
                .iter()
                .find(|gate| gate.kind == report.kind)
                .ok_or(Error::Invalid)?;
            if !seen.insert(report.kind)
                || report.suite != required.suite
                || report.candidate != binding.candidate
                || report.base != binding.base
                || report.policy_revision != binding.policy_revision
                || report.status != "passed"
                || report.evidence_ref.is_empty()
                || now_ms
                    .checked_sub(report.checked_at_ms)
                    .is_none_or(|age| age >= required.max_age_ms)
            {
                return Err(Error::Denied);
            }
            authority.verify(report, binding)?;
        }
        Ok(reports)
    }
    pub fn coglet(&self) -> &str {
        &self.0.coglet
    }
    pub fn revision(&self) -> &str {
        &self.0.revision
    }
}
