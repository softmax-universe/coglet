//! ADR-0033 producer state. This is part of ManagedState's existing WAL frame,
//! never a separate producer database. Local authentication belongs to DeviceHandler.
use super::*;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Admission {
    signed_ref: Value,
    operation_id: String,
    intent: String,
    scope: ReceiptScope,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallContext {
    operation_id: String,
    intent: String,
    scope: ReceiptScope,
}
/// Must be captured by the admission owner, NOT reconstructed from an install
/// reader or accepted tip. Missing old metadata deliberately stays unavailable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InitialAdmission {
    router: String,
    code: Admission,
    install: InstallContext,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AppliedGeneration {
    version: u32,
    target: String,
    router: String,
    generation: u64,
    commit: Value,
    executable_sha256: String,
    code: Admission,
    initial: InitialAdmission,
    receipt: Value,
    boundary: Value,
    /// Global immutable journal offset of this activation's first attempt.
    record_offset: usize,
}
#[derive(Clone, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Counters {
    attempted: u64,
    committed: u64,
    failed: u64,
    budget_exceeded: u64,
}
impl Counters {
    fn begin(&mut self) -> Result<()> {
        // Reserve room for EVERY possible observed result before guest entry.
        if [
            self.attempted,
            self.committed,
            self.failed,
            self.budget_exceeded,
        ]
        .contains(&u64::MAX)
        {
            return Err("resource_exhausted: policy health counter exhausted".into());
        }
        self.attempted += 1;
        Ok(())
    }
    fn result(&mut self, record: &Value) -> Result<()> {
        let counter = match record["status"].as_str() {
            Some("committed") => &mut self.committed,
            Some("aborted") => &mut self.failed,
            _ => return Err("invalid policy attempt outcome".into()),
        };
        *counter = counter
            .checked_add(1)
            .ok_or("policy result counter exhausted")?;
        if is_budget_failure(record) {
            self.budget_exceeded = self
                .budget_exceeded
                .checked_add(1)
                .ok_or("policy budget counter exhausted")?;
        }
        if self
            .committed
            .checked_add(self.failed)
            .is_none_or(|n| n > self.attempted)
            || self.budget_exceeded > self.failed
        {
            return Err("policy outcome has no retained attempt".into());
        }
        Ok(())
    }
}
fn is_budget_failure(record: &Value) -> bool {
    record["status"] == "aborted"
        && matches!(
            record["failure"]["kind"].as_str(),
            Some("compute" | "memory" | "wall" | "host_bytes")
        )
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PolicyState {
    applied: AppliedGeneration,
    counters: Counters,
    observation_sequence: u64,
}
impl PolicyState {
    pub(super) fn target(&self) -> &str {
        &self.applied.target
    }
}
fn check_context(id: &str, intent: &str, scope: &ReceiptScope, method: &str) -> Result<()> {
    validate("correlation-id", &json!(id))?;
    if intent.len() != 64
        || !intent
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || scope.method != method
    {
        return Err("invalid original policy admission context".into());
    }
    validate("method-name", &json!(scope.method))?;
    if scope.actor.is_empty() {
        return Err("missing original policy actor".into());
    }
    Ok(())
}
fn verify_ref(signed: &Value, router: &str, target: &str) -> Result<()> {
    #[cfg(feature = "device")]
    {
        coglet_proto::git_metadata::VerifiedRef::verify(
            signed,
            coglet_proto::git_metadata::RefKind::Hub,
            router,
            target,
        )
        .map_err(|_| "invalid retained policy signed generation".to_owned())?;
        Ok(())
    }
    #[cfg(not(feature = "device"))]
    {
        let _ = (signed, router, target);
        Err("policy signature verification unavailable".into())
    }
}
fn executable(runtime: &Runtime) -> Result<String> {
    Ok(digest(
        &hex::decode(
            runtime
                .files
                .files()
                .get("/code/bin/step.wasm")
                .ok_or("missing policy executable")?,
        )
        .map_err(|e| e.to_string())?,
    ))
}
impl ManagedState {
    pub(crate) fn policy_install(&mut self, runtime: &Runtime) -> Result<()> {
        let Some(value) = self.identity.get("policy_admission") else {
            return Ok(());
        };
        let initial: InitialAdmission =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        let target = runtime.manifest["id"]
            .as_str()
            .ok_or("missing policy target")?;
        check_context(
            &initial.code.operation_id,
            &initial.code.intent,
            &initial.code.scope,
            "hub.git.push",
        )?;
        check_context(
            &initial.install.operation_id,
            &initial.install.intent,
            &initial.install.scope,
            "hub.install",
        )?;
        verify_ref(&initial.code.signed_ref, &initial.router, target)?;
        if initial.code.signed_ref != self.identity["initial"]
            || initial.install.operation_id != self.install_receipt["operation_id"]
            || Some(&initial.install.scope) != self.install_scope.as_ref()
            || initial.code.signed_ref["statement"]["new"] != self.live_commit
            || initial.code.operation_id == initial.install.operation_id
        {
            return Err("policy install admission differs from retained identity".into());
        }
        self.policy = Some(PolicyState {
            applied: AppliedGeneration {
                version: 1,
                target: target.into(),
                router: initial.router.clone(),
                generation: initial.code.signed_ref["statement"]["generation"]
                    .as_u64()
                    .ok_or("missing policy generation")?,
                commit: self.live_commit.clone(),
                executable_sha256: executable(runtime)?,
                code: initial.code.clone(),
                initial,
                receipt: self.install_receipt.clone(),
                boundary: json!({"kind":"install","operation_id":self.install_receipt["operation_id"],"step":0}),
                record_offset: 0,
            },
            counters: Counters::default(),
            observation_sequence: 0,
        });
        Ok(())
    }
    pub(crate) fn policy_revision(
        &mut self,
        revision: &CodeRevision,
        runtime: &Runtime,
        receipt: &Value,
        record_offset: usize,
    ) -> Result<()> {
        let Some(policy) = &mut self.policy else {
            return Ok(());
        };
        verify_ref(
            &revision.signed_ref,
            &policy.applied.router,
            &policy.applied.target,
        )?;
        let generation = revision.signed_ref["statement"]["generation"]
            .as_u64()
            .ok_or("missing policy generation")?;
        if generation <= policy.applied.generation {
            return Err("policy generation did not advance".into());
        }
        policy.applied.generation = generation;
        policy.applied.commit = revision.signed_ref["statement"]["new"].clone();
        policy.applied.executable_sha256 = executable(runtime)?;
        policy.applied.code = Admission {
            signed_ref: revision.signed_ref.clone(),
            operation_id: revision.operation_id.clone(),
            intent: revision.intent.clone(),
            scope: revision.scope.clone(),
        };
        policy.applied.receipt = receipt.clone();
        policy.applied.boundary =
            json!({"kind":"code","trace_id":runtime.trace_id,"operation_id":revision.operation_id});
        policy.applied.record_offset = record_offset;
        policy.counters = Counters::default();
        // Ordering never resets, including an A -> B -> A code sequence.
        Ok(())
    }
    pub(crate) fn policy_begin(&mut self) -> Result<()> {
        if let Some(p) = &mut self.policy {
            p.counters.begin()?;
        }
        Ok(())
    }
    pub(crate) fn policy_result(&mut self, record: &Value) -> Result<()> {
        if let Some(p) = &mut self.policy {
            p.counters.result(record)?;
        }
        Ok(())
    }
    pub(crate) fn policy_validate(&self, state: &ServiceState, records: &[Value]) -> Result<()> {
        let Some(policy) = &self.policy else {
            if self.identity.get("policy_admission").is_some() {
                return Err("retained policy admission lacks atomic applied record".into());
            }
            return Ok(());
        };
        // Derive provenance again from original admission and immutable code cuts.
        let mut expected = self.clone();
        expected.policy = None;
        expected.live_commit = self.identity["initial"]["statement"]["new"].clone();
        let initial_setup = state
            .completed_segments
            .first()
            .map(|s| &s.initial_setup)
            .unwrap_or(&state.runtime.initial_setup);
        expected.policy_install(&Runtime::from_fixture(initial_setup)?)?;
        for segment in &state.completed_segments {
            if let Some(revision) = &segment.code_revision {
                let successor = revision.successor(
                    &Runtime::from_fixture(&segment.initial_setup)?,
                    segment
                        .successor_trace
                        .as_ref()
                        .ok_or("missing policy successor trace")?,
                )?;
                let receipt = &self
                    .operations
                    .get(&revision.operation_id)
                    .ok_or("missing policy code receipt")?
                    .receipt;
                expected.policy_revision(revision, &successor, receipt, segment.end)?;
            }
        }
        let expected = expected
            .policy
            .ok_or("policy record has no original admission")?;
        if expected.applied != policy.applied
            || policy.applied.commit != self.live_commit
            || executable(&state.runtime)? != policy.applied.executable_sha256
            || policy.applied.record_offset > records.len()
        {
            return Err("recovered policy applied provenance differs".into());
        }
        let mut counters = Counters::default();
        for record in records[policy.applied.record_offset..]
            .iter()
            .filter(|r| r.get("step").is_some())
        {
            counters.begin()?;
            counters.result(record)?;
        }
        if state.runtime.unresolved_attempt.is_some() {
            counters.begin()?;
        }
        if counters != policy.counters {
            return Err("recovered policy counters differ from actual boundaries".into());
        }
        Ok(())
    }
}
impl RuntimeService {
    /// Caller MUST validate the configured local principal before this seam.
    pub(crate) fn policy_attest_applied(
        &self,
        target: &str,
        generation: u64,
        commit: &str,
    ) -> Result<Value> {
        if !self.store.is_healthy() {
            return Err("unavailable: policy WAL unhealthy".into());
        }
        let managed = self
            .state
            .managed
            .as_ref()
            .ok_or("unavailable: not installed")?;
        managed.validate_code_history(&self.state.completed_segments)?;
        managed.policy_validate(&self.state, self.store.records())?;
        let applied = &managed
            .policy
            .as_ref()
            .ok_or("unavailable: original policy provenance missing")?
            .applied;
        if applied.target != target || applied.generation != generation || applied.commit != commit
        {
            return Err("conflict: expected policy generation differs".into());
        }
        serde_json::to_value(applied).map_err(|e| e.to_string())
    }
    /// Capabilities and lease state must come from the freshly checked receiving
    /// owner, not request JSON. Instance identity is minted by this coordinator.
    pub(crate) fn policy_observe_health(
        &mut self,
        target: &str,
        generation: u64,
        commit: &str,
        admitted_capabilities: &[String],
        output_lease_state: &str,
    ) -> Result<Value> {
        let applied = self.policy_attest_applied(target, generation, commit)?;
        let instance = self
            .policy_instance
            .as_ref()
            .ok_or("unavailable: producer instance missing")?
            .clone();
        let mapping = self.policy_clock_mapping()?;
        for capability in admitted_capabilities {
            validate("capability-name", &json!(capability))?;
        }
        if !matches!(
            output_lease_state,
            "closed" | "enabled" | "revoked" | "uncertain" | "unavailable"
        ) {
            return Err("invalid policy output lease state".into());
        }
        let mut next = self.state.clone();
        let managed = next.managed.as_mut().unwrap();
        let policy = managed.policy.as_mut().unwrap();
        // An exhausted domain cannot supply a healthy observation while unable
        // to retain its next begin/result. Probe capacity without changing it.
        policy.counters.clone().begin()?;
        policy.observation_sequence = policy
            .observation_sequence
            .checked_add(1)
            .ok_or("resource_exhausted: policy observation sequence exhausted")?;
        let sample = json!({"version":1,"target":target,"generation":generation,"commit":commit,
            "applied_operation_id":applied["code"]["operation_id"],
            "executable_sha256":applied["executable_sha256"],
            "activation":applied["boundary"], "counters":policy.counters,
            "observation_sequence":policy.observation_sequence,"producer_instance":instance,
            "clock":mapping,"admitted_capabilities":admitted_capabilities,
            "integrity":"verified","storage":"available",
            "attempt_fence":if next.runtime.unresolved_attempt.is_some() {"uncertain"} else {"clear"},
            "lifecycle":if managed.recovery_fault.is_some() {"faulted"} else if managed.running && !managed.cancellation_pending {"running"} else {"stopped"},
            "code_pending":managed.code_pending,"output_lease_state":output_lease_state});
        // No sample escapes if its ordering sequence could not be retained.
        self.store.commit(
            &serde_json::to_value(&next).map_err(|e| e.to_string())?,
            None,
        )?;
        self.state = next;
        Ok(sample)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_counters_reserve_before_entry_and_keep_unknown_uncertain() {
        let mut counters = Counters::default();
        counters.begin().unwrap();
        assert_eq!(counters.attempted, 1);
        assert_eq!(counters.committed, 0);
        counters
            .result(&json!({"status":"aborted","failure":{"kind":"compute"}}))
            .unwrap();
        assert_eq!(counters.failed, 1);
        assert_eq!(counters.budget_exceeded, 1);
        counters.begin().unwrap();
        counters.result(&json!({"status":"committed"})).unwrap();
        assert_eq!(counters.committed, 1);
    }
    #[test]
    fn policy_counter_exhaustion_refuses_before_entry() {
        for field in ["attempted", "committed", "failed", "budget_exceeded"] {
            let mut value = serde_json::to_value(Counters::default()).unwrap();
            value[field] = json!(u64::MAX);
            let mut counters: Counters = serde_json::from_value(value).unwrap();
            let before = counters.clone();
            assert!(counters.begin().is_err());
            assert_eq!(counters, before);
        }
    }
    #[cfg(feature = "device")]
    fn installed() -> (tempfile::TempDir, Value, RuntimeService) {
        let cases: Value =
            serde_json::from_str(include_str!("../../../conformance/abi/cases.json")).unwrap();
        let mut setup = cases
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == "no-effects")
            .unwrap()["steps"][0]["send"]
            .clone();
        setup["initial_files"]
            .as_object_mut()
            .unwrap()
            .retain(|p, _| !p.starts_with("/data/"));
        let key = iroh::SecretKey::from_bytes(&[37; 32]);
        let router = key.public().to_string();
        let statement = json!({"purpose":"hub-ref","hub":router,"coglet":setup["manifest"]["id"],
            "ref":"refs/heads/hub","old":null,"new":"a".repeat(40),
            "archive_sha256":"b".repeat(64),"generation":1});
        let signature = key.sign(&coglet_proto::signing::preimage("hub-ref", &statement).unwrap());
        let signed = json!({"statement":statement,"signer":router,"signature":hex::encode(signature.to_bytes())});
        let scope = ReceiptScope {
            actor: "install-actor".into(),
            ticket: None,
            method: "hub.install".into(),
        };
        let identity = json!({"initial":signed,"policy_admission":{"router":router,
            "code":{"signed_ref":signed,"operation_id":"1".repeat(32),"intent":"c".repeat(64),
                "scope":{"actor":"original-code-actor","ticket":"original-ticket","method":"hub.git.push"}},
            "install":{"operation_id":"2".repeat(32),"intent":"d".repeat(64),"scope":scope}}});
        let dir = tempfile::tempdir().unwrap();
        let mut service = RuntimeService::open(&dir.path().join("policy.wal"), &setup, 8).unwrap();
        service
            .managed_install_scoped(&identity, &"2".repeat(32), Some(&scope))
            .unwrap();
        (dir, setup, service)
    }

    #[cfg(feature = "device")]
    #[test]
    fn policy_install_attestation_health_sequence_and_reopen_are_atomic() {
        let (dir, setup, mut service) = installed();
        let target = setup["manifest"]["id"].as_str().unwrap();
        let commit = "a".repeat(40);
        let applied = service.policy_attest_applied(target, 1, &commit).unwrap();
        crate::policy_local::reply_tests::assert_result_conforms(
            "attest_applied",
            target,
            &applied,
        );
        assert_eq!(applied["code"]["scope"]["actor"], "original-code-actor");
        assert_eq!(
            applied["initial"]["install"]["scope"]["actor"],
            "install-actor"
        );
        assert!(service.policy_attest_applied(target, 2, &commit).is_err());
        assert!(
            service
                .policy_observe_health(target, 1, &commit, &[], "closed")
                .is_err()
        );
        service.supervised_now().unwrap();
        let sample = service
            .policy_observe_health(target, 1, &commit, &[], "closed")
            .unwrap();
        crate::policy_local::reply_tests::assert_result_conforms("observe_health", target, &sample);
        assert_eq!(sample["observation_sequence"], 1);
        assert_eq!(sample["counters"]["attempted"], 0);
        assert_eq!(sample["lifecycle"], "stopped");
        drop(service);
        let mut service = RuntimeService::open(&dir.path().join("policy.wal"), &setup, 8).unwrap();
        assert_eq!(
            service.policy_attest_applied(target, 1, &commit).unwrap(),
            applied
        );
        let next = service
            .policy_observe_health(target, 1, &commit, &[], "closed")
            .unwrap();
        assert_eq!(next["observation_sequence"], 2);
        assert_ne!(next["producer_instance"], sample["producer_instance"]);
        assert_eq!(
            next["clock"]["runtime_anchor"],
            sample["clock"]["runtime_anchor"]
        );
        service
            .state
            .managed
            .as_mut()
            .unwrap()
            .policy
            .as_mut()
            .unwrap()
            .observation_sequence = u64::MAX;
        assert!(
            service
                .policy_observe_health(target, 1, &commit, &[], "closed")
                .is_err()
        );
    }

    #[cfg(feature = "device")]
    #[test]
    fn policy_wal_coordinator_keeps_unknown_and_reconciles_once() {
        // This is a WAL/coordinator unit test with the fixture engine, NOT native
        // WAMR or ADR-0033 daemon acceptance evidence.
        let (dir, setup, mut service) = installed();
        service.managed_start().unwrap();
        service
            .enqueue(setup["inputs"]["events"][0].clone())
            .unwrap();
        struct Unknown;
        impl Engine for Unknown {
            fn execute(&mut self, _: &Value, _: &mut StepHost) -> Result<EngineReport> {
                Err("unknown guest outcome".into())
            }
        }
        assert!(service.run_next(&setup["inputs"], &mut Unknown).is_err());
        let counters = &service
            .state
            .managed
            .as_ref()
            .unwrap()
            .policy
            .as_ref()
            .unwrap()
            .counters;
        assert_eq!(counters.attempted, 1);
        assert_eq!(counters.committed, 0);
        drop(service);
        let mut service = RuntimeService::open(&dir.path().join("policy.wal"), &setup, 8).unwrap();
        let inputs = service.state.runtime.unresolved_attempt.clone().unwrap();
        let mut runtime = service.state.runtime.clone();
        runtime.unresolved_attempt = None;
        let result = runtime
            .step(&inputs, &mut MockEngine::new(setup["guest_script"].clone()))
            .unwrap();
        service.recover_attempt(&result["journal"]).unwrap();
        assert!(service.recover_attempt(&result["journal"]).is_err());
        let counters = &service
            .state
            .managed
            .as_ref()
            .unwrap()
            .policy
            .as_ref()
            .unwrap()
            .counters;
        assert_eq!(counters.attempted, 1);
        assert_eq!(counters.committed, 1);
        drop(service);
        let service = RuntimeService::open(&dir.path().join("policy.wal"), &setup, 8).unwrap();
        assert_eq!(
            service
                .state
                .managed
                .as_ref()
                .unwrap()
                .policy
                .as_ref()
                .unwrap()
                .counters
                .committed,
            1
        );
    }

    #[cfg(feature = "device")]
    #[test]
    fn policy_missing_legacy_provenance_is_not_backfilled() {
        let (_dir, _setup, mut service) = installed();
        let managed = service.state.managed.as_mut().unwrap();
        managed
            .identity
            .as_object_mut()
            .unwrap()
            .remove("policy_admission");
        managed.policy = None;
        assert!(
            service
                .policy_attest_applied("any-target", 1, &"a".repeat(40))
                .unwrap_err()
                .contains("provenance missing")
        );
    }

    #[test]
    fn policy_admission_is_closed_and_retains_absent_ticket() {
        let scope = ReceiptScope {
            actor: "original".into(),
            ticket: None,
            method: "hub.git.push".into(),
        };
        let admission = Admission {
            signed_ref: json!({"signature":"original"}),
            operation_id: "1".repeat(32),
            intent: "a".repeat(64),
            scope,
        };
        let mut value = serde_json::to_value(&admission).unwrap();
        assert_eq!(
            serde_json::from_value::<Admission>(value.clone()).unwrap(),
            admission
        );
        value["reader"] = json!("later-reader");
        assert!(serde_json::from_value::<Admission>(value).is_err());
    }
}
