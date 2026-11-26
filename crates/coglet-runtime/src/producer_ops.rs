//! ADR-0033 local producer operations behind the authenticated endpoint.
//!
//! Every method here assumes `policy_local::authenticate` already proved the peer
//! UID and principal/actor/target/ticket binding. None accepts policy thresholds,
//! rollback decisions, or caller-supplied producer records.
use super::*;
use crate::output_lease::{EmergencyCustody, EmergencyObligation, EmergencyStatus, LeaseState};
use crate::policy_local::{LocalOps, Method, Params, PrincipalBinding};

/// Opaque adapter from the transport to the handler.
struct LocalEndpoint {
    handler: DeviceHandler,
    runtime: tokio::runtime::Handle,
}

impl LocalOps for LocalEndpoint {
    fn daemon_instance(&self) -> &str {
        &self.handler.inner.daemon_instance
    }
    fn call(
        &self,
        binding: &PrincipalBinding,
        request: &crate::policy_local::Request,
    ) -> std::result::Result<Value, &'static str> {
        let h = &self.handler;
        let outcome = match request.method {
            // Synchronous and local: never enters the async runtime, SQL or a lock
            // shared with the guest.
            Method::RevokeOutputLease => h.local_revoke(binding),
            Method::AttestApplied => self
                .runtime
                .block_on(h.local_attest(binding, &request.params)),
            Method::ObserveHealth => self
                .runtime
                .block_on(h.local_health(binding, &request.params)),
            Method::EnableOutputLease => self
                .runtime
                .block_on(h.local_enable(binding, &request.params)),
            Method::ReconcileEmergencyStop => self.runtime.block_on(h.local_reconcile(binding)),
        };
        outcome.map_err(wire)
    }
}

/// `(generation, commit, executable_sha256)` pins from validated params.
fn pins(params: &Params) -> Result<(u64, &str, &str)> {
    match (
        params.generation,
        params.commit.as_deref(),
        params.executable_sha256.as_deref(),
    ) {
        (Some(generation), Some(commit), Some(executable)) => Ok((generation, commit, executable)),
        _ => Err("invalid_argument".into()),
    }
}

/// Attested executable digest must equal the caller's pin and the bytes of the
/// freshly re-verified installed archive (`setup` came from `recover_setup`).
fn check_executable(attested: &Value, pin: &str, setup: &Value) -> Result<()> {
    let wasm = hex::decode(
        setup["initial_files"]["/code/bin/step.wasm"]
            .as_str()
            .ok_or("permission_denied")?,
    )
    .map_err(|_| "permission_denied".to_owned())?;
    let live = digest(&wasm);
    if attested["executable_sha256"] != pin || live != pin {
        return Err("conflict: executable differs from expected generation".into());
    }
    Ok(())
}

/// Capabilities admitted by the current verified archive; never request JSON.
fn admitted_capabilities(handler: &DeviceHandler, setup: &Value) -> Result<Vec<String>> {
    admit_manifest(
        &setup["manifest"],
        &handler.inner.config.catalog,
        &handler.inner.config.grants,
    )?;
    let mut names = BTreeSet::new();
    for field in ["required_capabilities", "subscriptions"] {
        for name in setup["manifest"][field]
            .as_array()
            .ok_or("invalid manifest")?
        {
            names.insert(name.as_str().ok_or("invalid manifest")?.to_owned());
        }
    }
    Ok(names.into_iter().collect())
}

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
type BoundListeners = crate::policy_local::LocalListener;
#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
type BoundListeners = std::convert::Infallible;

/// The producer listener pair, bound but not yet accepting (ADR-0043). `None`
/// inside: no profile, so neither socket exists.
pub struct ProducerEndpoint(Option<BoundListeners>);
impl ProducerEndpoint {
    /// Start both accept workers, or neither. Errors abort startup; dropping
    /// the endpoint unlinks only the sockets this startup created.
    pub fn start(self) -> Result<()> {
        match self.0 {
            None => Ok(()),
            #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
            Some(listeners) => listeners.start(),
            #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
            Some(never) => match never {},
        }
    }
}

impl DeviceHandler {
    /// Validate and bind BOTH producer sockets. Call before `recover_running`
    /// (which may expose an adapter listener) and before the hub connection, so
    /// no adapter or lease exists without the emergency endpoint. Output gates
    /// already exist and are closed. Errors abort startup.
    pub fn bind_producer_endpoint(
        &self,
        runtime: tokio::runtime::Handle,
    ) -> Result<ProducerEndpoint> {
        let Some(profile) = self.inner.config.profile.clone() else {
            return Ok(ProducerEndpoint(None));
        };
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
        {
            if self.inner.producer_started.swap(true, Ordering::SeqCst) {
                return Err("producer endpoint already started".into());
            }
            let ops = Arc::new(LocalEndpoint {
                handler: self.clone(),
                runtime,
            });
            crate::policy_local::LocalListener::bind(Arc::new(profile), ops)
                .map(|listeners| ProducerEndpoint(Some(listeners)))
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
        {
            let _ = (profile, runtime);
            Err("producer endpoint requires a Unix socket transport".into())
        }
    }

    fn gate(&self, target: &str) -> Result<&Arc<OutputGate>> {
        self.inner
            .gates
            .get(target)
            .ok_or_else(|| "permission_denied".into())
    }

    fn local_service(&self, target: &str) -> Result<Arc<Mutex<RuntimeService>>> {
        if let Some(service) = self.share_service(target) {
            return Ok(service);
        }
        let setup = self
            .original_setup(target)?
            .ok_or("unavailable: not installed")?;
        self.open_service(target, &setup)
    }

    /// Re-verified current archive (executable bytes cross-checked against the
    /// retained install identity) before any disclosure or grant.
    async fn verified_setup(&self, target: &str) -> Result<Value> {
        let (_, setup, _) = self
            .recover_setup(target)
            .await?
            .ok_or("unavailable: not installed")?;
        Ok(setup)
    }

    fn stop_request(binding: &PrincipalBinding) -> Value {
        let mut context = json!({"actor": binding.stop.actor});
        if let Some(ticket) = &binding.stop.ticket {
            context["ticket"] = json!(ticket);
        }
        json!({"method":"hub.stop","target":binding.target,
            "correlation_id":binding.stop.operation_id,"timeout_ms":30_000,
            "params":binding.stop.params,"context":context})
    }

    pub(super) async fn local_attest(
        &self,
        binding: &PrincipalBinding,
        params: &Params,
    ) -> Result<Value> {
        let (generation, commit, executable) = pins(params)?;
        let (commit, executable) = (commit.to_owned(), executable.to_owned());
        let setup = self.verified_setup(&binding.target).await?;
        let handler = self.clone();
        let target = binding.target.clone();
        tokio::task::spawn_blocking(move || {
            let service = handler.local_service(&target)?;
            let locked = service.lock().map_err(|_| "unavailable".to_owned())?;
            let attested = locked.policy_attest_applied(&target, generation, &commit)?;
            check_executable(&attested, &executable, &setup)?;
            Ok(attested)
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    pub(super) async fn local_health(
        &self,
        binding: &PrincipalBinding,
        params: &Params,
    ) -> Result<Value> {
        let (generation, commit, executable) = pins(params)?;
        let (commit, executable) = (commit.to_owned(), executable.to_owned());
        let setup = self.verified_setup(&binding.target).await?;
        let capabilities = admitted_capabilities(self, &setup)?;
        let handler = self.clone();
        let target = binding.target.clone();
        tokio::task::spawn_blocking(move || {
            let gate = handler.gate(&target)?.clone();
            let service = handler.local_service(&target)?;
            let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
            let attested = locked.policy_attest_applied(&target, generation, &commit)?;
            check_executable(&attested, &executable, &setup)?;
            // Sampled inside the serialized observation, not before waiting.
            let lease = gate.state().as_str();
            locked.policy_observe_health(&target, generation, &commit, &capabilities, lease)
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    pub(super) async fn local_enable(
        &self,
        binding: &PrincipalBinding,
        params: &Params,
    ) -> Result<Value> {
        let (generation, commit, executable) = pins(params)?;
        if self.gate(&binding.target)?.state() != LeaseState::Closed {
            return Err(
                "permission_denied: lease cannot be enabled in this daemon instance".into(),
            );
        }
        if params.daemon_instance.as_deref() != Some(self.inner.daemon_instance.as_str()) {
            return Err("conflict: daemon instance differs".into());
        }
        let (commit, executable) = (commit.to_owned(), executable.to_owned());
        let setup = self.verified_setup(&binding.target).await?;
        let handler = self.clone();
        let binding = binding.clone();
        tokio::task::spawn_blocking(move || {
            handler.enable_blocking(&binding, generation, &commit, &executable, &setup)
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    fn enable_blocking(
        &self,
        binding: &PrincipalBinding,
        generation: u64,
        commit: &str,
        executable: &str,
        setup: &Value,
    ) -> Result<Value> {
        let target = binding.target.as_str();
        let gate = self.gate(target)?.clone();
        let pin = self.pin(target)?.clone();
        admit_manifest(
            &setup["manifest"],
            &self.inner.config.catalog,
            &self.inner.config.grants,
        )?;
        let request = Self::stop_request(binding);
        let obligation = EmergencyObligation {
            version: 1,
            target: target.to_owned(),
            operation_id: binding.stop.operation_id.clone(),
            intent: self.operation_intent(&request)?,
            request,
            actor: binding.stop.actor.clone(),
            ticket: binding.stop.ticket.clone(),
            grant_generation: pin.grant_generation,
            delegation_generation: pin.delegation_generation,
        };
        let service = self
            .share_service(target)
            .ok_or_else(|| "unavailable: not installed".to_owned())?;
        // 1. The stop must be authorizable NOW under its own original context,
        //    otherwise the obligation could never be honored. Read-only; the SQL
        //    guard is released before the start boundary (no nesting).
        let stop_scope = Scope {
            actor: obligation.actor.clone(),
            ticket: obligation.ticket.clone(),
        };

        // 2. Fresh start authority and integrity, custody retained, then grant.
        let recovery = service
            .lock()
            .map_err(|_| "unavailable".to_owned())?
            .managed_recovery()?;
        if recovery.grant_generation != pin.grant_generation
            || recovery.delegation_generation != pin.delegation_generation
        {
            return Err("permission_denied: changed start authority generations".into());
        }
        let scope = Scope {
            actor: recovery.scope.actor.clone(),
            ticket: recovery.scope.ticket.clone(),
        };
        self.inner.authority.runtime_boundary_obligation(
            self.inner.config.hub,
            self.inner.config.device,
            target,
            &scope,
            &Self::grant("hub.start", target, recovery.grant_generation),
            recovery.delegation_generation,
            None,
            Some(&recovery.operation_id),
            Some((
                &stop_scope,
                &Self::grant("hub.stop", target, pin.grant_generation),
                &obligation.operation_id,
            )),
            self.clock_fn(),
            |proof, recheck| {
                if recovery.delegation != *proof {
                    return Err("permission_denied".into());
                }
                let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
                let attested = locked.policy_attest_applied(target, generation, commit)?;
                check_executable(&attested, executable, setup)?;
                locked.managed_verify_code(setup)?;
                locked.managed_recovery()?;
                if locked.managed_status()["state"] != "running" {
                    return Err("unavailable: coglet is not running".into());
                }
                self.require_kill_clear()?;
                // Durable custody BEFORE any grant, in the original WAL.
                let retain = match locked.emergency_status(target)? {
                    EmergencyStatus::None => true,
                    // Retained by this very process and never granted: retry.
                    EmergencyStatus::Armed(existing)
                        if existing == obligation
                            && self
                                .inner
                                .retained_here
                                .lock()
                                .map_err(|_| "unavailable".to_owned())?
                                .contains(target) =>
                    {
                        false
                    }
                    // Resolved earlier; a NEW operation ID is required.
                    EmergencyStatus::Stopped(done)
                        if done.operation_id != obligation.operation_id =>
                    {
                        true
                    }
                    // Armed from an earlier process (a failed revoke write is
                    // indistinguishable from none), pending, or reused ID.
                    _ => {
                        return Err(
                            "conflict: unresolved emergency custody; reconcile the stop first"
                                .into(),
                        );
                    }
                };
                if retain {
                    locked.emergency_retain(&obligation)?;
                }
                self.inner
                    .retained_here
                    .lock()
                    .map_err(|_| "unavailable".to_owned())?
                    .insert(target.to_owned());
                self.require_kill_clear()?;
                recheck()?;
                // The grant is the last act, immediately after the final recheck.
                gate.enable()
            },
        )?;
        Ok(
            json!({"lease":"enabled","generation":generation,"commit":commit,
            "executable_sha256":executable,"daemon_instance":self.inner.daemon_instance,
            "custody":"retained"}),
        )
    }

    /// Never fails once the target is configured: the gate is closed first, then
    /// the durable record is attempted without ever waiting for the guest.
    pub(super) fn local_revoke(&self, binding: &PrincipalBinding) -> Result<Value> {
        let target = binding.target.as_str();
        let gate = self.gate(target)?.clone();
        let drained = gate.revoke().is_ok();
        // Record even after a drain overrun: the stop-pending state must exist.
        let durable = self.persist_revoke(target, &gate);
        let lease = gate.state().as_str();
        Ok(
            json!({"lease":lease,"gate":"closed","in_flight_drained":drained,
            "durable":durable,"daemon_instance":self.inner.daemon_instance}),
        )
    }

    /// `recorded` | `not_required` | `not_opened` | `pending` | `uncertain`.
    fn persist_revoke(&self, target: &str, gate: &Arc<OutputGate>) -> &'static str {
        let opened = match self.inner.services.try_lock() {
            Ok(services) => Some(services.get(target).cloned()),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                gate.mark_uncertain();
                return "uncertain";
            }
            Err(std::sync::TryLockError::WouldBlock) => None,
        };
        let service = match opened {
            // No coordinator in this process: this instance retained nothing.
            Some(None) => return "not_opened",
            Some(Some(service)) => service,
            None => return self.persist_revoke_later(target, gate),
        };
        match service.try_lock() {
            Ok(mut locked) => {
                Self::record_revoke(&mut locked, target, &self.inner.daemon_instance, gate)
            }
            Err(std::sync::TryLockError::WouldBlock) => self.persist_revoke_later(target, gate),
            Err(std::sync::TryLockError::Poisoned(_)) => {
                gate.mark_uncertain();
                "uncertain"
            }
        }
    }

    /// The runtime mutex is held by a guest or WAL write. The gate is already
    /// closed; ONE worker per target (ADR-0043) records durable stop-pending when
    /// custody frees up. Until then (and if it fails) this instance reports
    /// pending/uncertain and never re-enables.
    fn persist_revoke_later(&self, target: &str, gate: &Arc<OutputGate>) -> &'static str {
        let handler = self.clone();
        let target = target.to_owned();
        defer_record(
            gate,
            |work| {
                std::thread::Builder::new()
                    .name("cogletd-revoke-record".into())
                    .spawn(work)
                    .map(drop)
            },
            move |gate| {
                let recorded = handler
                    .local_service(&target)
                    .and_then(|service| {
                        let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
                        Ok(Self::record_revoke(
                            &mut locked,
                            &target,
                            &handler.inner.daemon_instance,
                            gate,
                        ))
                    })
                    .unwrap_or("uncertain");
                if recorded == "uncertain" {
                    eprintln!("cogletd: revoke for {target} could not be durably recorded");
                }
                recorded
            },
        )
    }

    fn record_revoke(
        locked: &mut RuntimeService,
        target: &str,
        daemon_instance: &str,
        gate: &OutputGate,
    ) -> &'static str {
        let outcome = match locked.emergency_status(target) {
            Ok(EmergencyStatus::None | EmergencyStatus::Stopped(_)) => return "not_required",
            Ok(EmergencyStatus::Armed(_) | EmergencyStatus::StopPending(_)) => locked
                .emergency_record_revoked(target, daemon_instance)
                .map(|()| "recorded"),
            Err(error) => Err(error),
        };
        outcome.unwrap_or_else(|_| {
            gate.mark_uncertain();
            "uncertain"
        })
    }

    pub(super) async fn local_reconcile(&self, binding: &PrincipalBinding) -> Result<Value> {
        let handler = self.clone();
        let target = binding.target.clone();
        tokio::task::spawn_blocking(move || handler.reconcile_blocking(&target))
            .await
            .map_err(|_| "unavailable".to_owned())?
    }

    /// Only the obligation retained in the WAL is used: its original actor,
    /// ticket, operation ID, intent and authority pins. The gate is never opened
    /// or held while waiting for authority or custody.
    fn reconcile_blocking(&self, target: &str) -> Result<Value> {
        if self.gate(target)?.state() == LeaseState::Enabled {
            return Err("conflict: revoke the output lease before reconciling".into());
        }
        let service = self.local_service(target)?;
        let status = service
            .lock()
            .map_err(|_| "unavailable".to_owned())?
            .emergency_status(target)?;
        let obligation = match status {
            EmergencyStatus::None => return Ok(json!({"state":"none"})),
            EmergencyStatus::Stopped(done) => {
                return Ok(json!({"state":"stopped","operation_id":done.operation_id}));
            }
            EmergencyStatus::Armed(o) | EmergencyStatus::StopPending(o) => o,
        };
        if obligation.target != target
            || obligation.request["method"] != "hub.stop"
            || obligation.request["target"] != target
            || obligation.request["correlation_id"] != obligation.operation_id
            || obligation.intent != self.operation_intent(&obligation.request)?
            || receipt_scope(&obligation.request).actor != obligation.actor
            || receipt_scope(&obligation.request).ticket != obligation.ticket
        {
            return Err("conflict: retained obligation does not match its intent".into());
        }
        let scope = Scope {
            actor: obligation.actor.clone(),
            ticket: obligation.ticket.clone(),
        };
        let applied = self.inner.authority.runtime_boundary_operation(
            self.inner.config.hub,
            self.inner.config.device,
            target,
            &scope,
            &Self::grant("hub.stop", target, obligation.grant_generation),
            obligation.delegation_generation,
            None,
            Some(&obligation.operation_id),
            self.clock_fn(),
            |_, recheck| {
                let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
                // Waiting for the guest/WAL may outlive the authority windows.
                let now = recheck()?;
                let receipt = locked.managed_operation(
                    &obligation.operation_id,
                    &obligation.intent,
                    false,
                    now,
                    &receipt_scope(&obligation.request),
                )?;
                validate("hub-mutation-receipt", &receipt)?;
                if receipt["state"] != "applied" {
                    return Err("unavailable: stop receipt is not applied".into());
                }
                // Refuses (and stays fenced) while a guest attempt is unresolved.
                locked.emergency_record_stopped(target, &receipt)?;
                Ok(obligation.operation_id.clone())
            },
        )?;
        // Receipt bodies are never disclosed here (ADR-0032).
        Ok(json!({"state":"stopped","operation_id":applied}))
    }
}

/// ADR-0043 bounded deferred recording, shared by both listeners: the gate's
/// single recorder slot is reserved atomically BEFORE any spawn. While a
/// recorder is queued or running, further revokes report `pending` without
/// another worker. The slot is released after completion, error or panic; a
/// failed spawn marks the gate uncertain and releases it. Nothing here can
/// reopen the gate or clear uncertainty.
fn defer_record(
    gate: &Arc<OutputGate>,
    spawn: impl FnOnce(Box<dyn FnOnce() + Send>) -> std::io::Result<()>,
    record: impl FnOnce(&OutputGate) -> &'static str + Send + 'static,
) -> &'static str {
    struct Release(Arc<OutputGate>);
    impl Drop for Release {
        fn drop(&mut self) {
            self.0.release_recorder();
        }
    }
    if !gate.reserve_recorder() {
        return "pending";
    }
    let worker = gate.clone();
    let work = Box::new(move || {
        let slot = Release(worker);
        if record(&slot.0) == "uncertain" {
            slot.0.mark_uncertain();
        }
    });
    if spawn(work).is_err() {
        gate.mark_uncertain();
        gate.release_recorder();
        return "uncertain";
    }
    "pending"
}

#[cfg(test)]
mod recorder_tests {
    //! Deterministic unit seam for ADR-0043 recorder coalescing. The native
    //! proof is `repeated_revoke_coalesces_pending_recorder` (real cogletd).
    use super::*;
    use std::sync::mpsc;

    type Work = Box<dyn FnOnce() + Send>;

    fn revoked() -> Arc<OutputGate> {
        let gate = OutputGate::new_closed();
        gate.enable().unwrap();
        gate.revoke().unwrap();
        gate
    }

    #[test]
    fn one_pending_recorder_is_coalesced_and_its_slot_reclaimed() {
        let gate = revoked();
        let (queue, queued) = mpsc::channel::<Work>();
        let spawn = |work: Work| {
            queue.send(work).unwrap();
            Ok(())
        };
        assert_eq!(defer_record(&gate, spawn, |_| "recorded"), "pending");
        // Queued, not yet run: every repeat coalesces without another worker.
        for _ in 0..32 {
            let never = |_: Work| -> std::io::Result<()> { panic!("second recorder spawned") };
            assert_eq!(defer_record(&gate, never, |_| "recorded"), "pending");
        }
        queued.try_recv().unwrap()();
        assert!(
            queued.try_recv().is_err(),
            "exactly one recorder was queued"
        );
        assert_eq!(gate.state(), LeaseState::Revoked);
        // Completion reclaimed the slot: a later recorder may be queued again.
        assert_eq!(defer_record(&gate, spawn, |_| "uncertain"), "pending");
        queued.try_recv().unwrap()();
        assert_eq!(
            gate.state(),
            LeaseState::Uncertain,
            "recorder error is sticky"
        );
        // The error also reclaimed the slot, and nothing clears uncertainty.
        assert_eq!(defer_record(&gate, spawn, |_| "recorded"), "pending");
        queued.try_recv().unwrap()();
        assert_eq!(gate.state(), LeaseState::Uncertain);
        assert!(gate.enable().is_err());
    }

    #[test]
    fn spawn_failure_marks_uncertain_and_releases_the_slot() {
        let gate = revoked();
        let fail = |_: Work| Err(std::io::Error::other("no thread"));
        assert_eq!(defer_record(&gate, fail, |_| "recorded"), "uncertain");
        assert_eq!(gate.state(), LeaseState::Uncertain);
        assert!(gate.enable().is_err(), "no retry may reopen the gate");
        let (queue, queued) = mpsc::channel::<Work>();
        let spawn = |work: Work| {
            queue.send(work).unwrap();
            Ok(())
        };
        assert_eq!(defer_record(&gate, spawn, |_| "recorded"), "pending");
        queued.try_recv().unwrap()();
        assert_eq!(
            gate.state(),
            LeaseState::Uncertain,
            "never silently cleared"
        );
    }

    #[test]
    fn panicking_recorder_releases_the_slot() {
        let gate = revoked();
        let (queue, queued) = mpsc::channel::<Work>();
        let spawn = |work: Work| {
            queue.send(work).unwrap();
            Ok(())
        };
        assert_eq!(
            defer_record(&gate, spawn, |_| panic!("recorder")),
            "pending"
        );
        let work = queued.try_recv().unwrap();
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).is_err());
        assert!(
            gate.reserve_recorder(),
            "slot leaked by a panicking recorder"
        );
    }
}

pub(super) fn random_hex32() -> Result<String> {
    let mut bytes = [0u8; 32];
    #[cfg(unix)]
    {
        // SAFETY: writable buffer of the stated length (<= 256).
        if unsafe { libc::getentropy(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
            return Err("daemon instance entropy unavailable".into());
        }
        Ok(hex::encode(bytes))
    }
    #[cfg(not(unix))]
    {
        let _ = &mut bytes;
        Err("daemon instance entropy unavailable".into())
    }
}
