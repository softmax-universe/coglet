//! Authenticated back-channel serving on the production router connection.
use super::*;
use crate::execution::receiver::receiver_scope;

/// Local operator path limits. Empty lists export/write nothing. A trailing `/`
/// admits canonical descendants; other entries name one exact file/step record.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackChannelLimits {
    #[serde(default)]
    pub read_paths: Vec<String>,
    #[serde(default)]
    pub write_paths: Vec<String>,
}

impl BackChannelLimits {
    fn permits(&self, method: &str, request: &Value) -> bool {
        let entries = match method {
            "debug.read" => &self.read_paths,
            "debug.write" => &self.write_paths,
            _ => return true,
        };
        let Some(path) = request["params"]["path"].as_str() else {
            return false;
        };
        entries
            .iter()
            .any(|entry| entry == path || (entry.ends_with('/') && path.starts_with(entry)))
    }
}

impl DeviceHandler {
    #[cfg(feature = "native-engine")]
    fn receiver_step_unlocked(
        &self,
        coglet: &str,
        pin: &TargetPin,
        request: &Value,
        service: &std::sync::Arc<std::sync::Mutex<crate::RuntimeService>>,
        setup: &Value,
    ) -> Result<()> {
        let original = receiver_scope(request);
        let scope = Scope {
            actor: original.actor,
            ticket: original.ticket,
        };
        let id = request["correlation_id"]
            .as_str()
            .ok_or("invalid_argument")?;
        let grant = Self::grant("debug.step", coglet, pin.grant_generation);
        let prepared = self.inner.authority.runtime_boundary_operation(
            self.inner.config.hub,
            self.inner.config.device,
            coglet,
            &scope,
            &grant,
            pin.delegation_generation,
            None,
            Some(id),
            self.clock_fn(),
            |_, recheck| {
                recheck()?;
                let mut locked = service.lock().map_err(|_| "unavailable")?;
                let preflight = self.require_kill_clear().and_then(|()| {
                    admit_manifest(
                        &setup["manifest"],
                        &self.inner.config.catalog,
                        &self.inner.config.grants,
                    )
                });
                if let Err(error) = preflight {
                    locked.receiver_apply(id, &|| Err(error.clone()))?;
                    return Ok(None);
                }
                if self.enable_native(coglet, &mut locked).is_err() {
                    locked.receiver_apply(id, &|| Err("unavailable".into()))?;
                    return Ok(None);
                }
                locked.receiver_prepare_native_step(
                    id,
                    &self.inner.config.grants["operations"],
                    &|| {
                        recheck()?;
                        self.require_kill_clear()
                    },
                )
            },
        );
        let prepared = match prepared {
            Err(error) if error == "permission_denied" => {
                service
                    .lock()
                    .map_err(|_| "unavailable")?
                    .receiver_apply(id, &|| Err("permission_denied".into()))?;
                return Ok(());
            }
            other => other?,
        };
        let Some((run, config, mut engine)) = prepared else {
            return Ok(());
        };
        let outcome = crate::RuntimeService::execute_detached(
            service,
            run,
            &mut engine,
            &|fence| {
                self.inner.authority.runtime_boundary_operation(
                    self.inner.config.hub,
                    self.inner.config.device,
                    coglet,
                    &scope,
                    &grant,
                    pin.delegation_generation,
                    None,
                    Some(id),
                    self.clock_fn(),
                    |_, recheck| {
                        recheck()?;
                        self.require_kill_clear()?;
                        fence()?;
                        recheck().map(|_| ())
                    },
                )
            },
            &|_, _| Ok(()),
        );
        service
            .lock()
            .map_err(|_| "unavailable")?
            .receiver_restore_native(id, config, engine)?;
        outcome
    }

    pub(super) async fn back_channel(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            let method = request["method"].as_str().ok_or("invalid_argument")?;
            let scope = receiver_scope(&request);
            let grant = Self::grant(method, &coglet, pin.grant_generation);
            handler.inner.authority.runtime_boundary_operation(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &Scope {
                    actor: scope.actor.clone(),
                    ticket: scope.ticket.clone(),
                },
                &grant,
                pin.delegation_generation,
                None,
                matches!(
                    method,
                    "debug.write" | "hub.inbox" | "debug.pause" | "debug.step" | "hub.cancel"
                )
                .then(|| request["correlation_id"].as_str())
                .flatten(),
                handler.clock_fn(),
                |_, recheck| {
                    let gate = || deadline_gate(&context, recheck).map(|_| ());
                    gate()?;
                    if !pin.back_channel.permits(method, &request) {
                        return Err("permission_denied".into());
                    }
                    let setup = handler
                        .original_setup(&coglet)?
                        .ok_or("permission_denied")?;
                    let service = handler.open_service(&coglet, &setup)?;
                    let mut locked = service.lock().map_err(|_| "unavailable")?;
                    gate()?;
                    if locked.managed_identity().is_none() {
                        return Err("permission_denied".into());
                    }
                    let mut result = match method {
                        "debug.read" => locked.receiver_read(
                            request["params"]["path"]
                                .as_str()
                                .ok_or("invalid_argument")?,
                            request["params"]["offset"]
                                .as_u64()
                                .ok_or("invalid_argument")?,
                            request["params"]["limit"]
                                .as_u64()
                                .ok_or("invalid_argument")?,
                        ),
                        "debug.write" | "hub.inbox" | "debug.pause" | "debug.step" => {
                            locked.receiver_admit(&request)
                        }
                        "hub.cancel" => locked.receiver_cancel_request(&request),
                        _ => Err("invalid_argument".into()),
                    }?;
                    if matches!(method, "hub.inbox" | "debug.write")
                        && result["state"] == "queued"
                        && locked
                            .receiver_pending()
                            .is_some_and(|head| head["correlation_id"] == request["correlation_id"])
                    {
                        // An idle boundary can commit immediately. Do not wait
                        // for an in-progress guest or leapfrog earlier work.
                        let id = request["correlation_id"]
                            .as_str()
                            .ok_or("invalid_argument")?;
                        locked.receiver_apply(id, &gate)?;
                        result = locked.receiver_admit(&request)?;
                    }
                    gate()?;
                    if method == "debug.step" && result["state"] != "queued" {
                        return locked.receiver_step_result(
                            request["correlation_id"]
                                .as_str()
                                .ok_or("invalid_argument")?,
                        );
                    }
                    Ok(result)
                },
            )
        })
        .await
        .map_err(|_| "unavailable")?
    }

    /// Bounded, runtime-owned retries also cover work admitted while an ordinary
    /// adapter guest holds the execution reservation. No new network request is
    /// needed to reach the next boundary. Unknown guest attempts remain fenced:
    /// `receiver_pending` never selects them for automatic re-execution.
    pub(super) fn resume_receiver(&self) -> Result<()> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "device recovery requires an active Tokio runtime".to_owned())?;
        let weak = std::sync::Arc::downgrade(&self.inner);
        runtime.spawn(async move {
            let mut delay = std::time::Duration::from_millis(250);
            loop {
                tokio::time::sleep(delay).await;
                let Some(inner) = weak.upgrade() else {
                    break;
                };
                let handler = Self { inner };
                let mut failed = false;
                for coglet in handler.inner.config.targets.keys() {
                    // Unopened WALs are deliberately retried too: initial startup
                    // recovery may fail transiently before it caches a service.
                    // Do filesystem/SQL/WAL work on the blocking pool below.
                    if handler.drain_receiver(coglet).await.is_err() {
                        failed = true;
                    }
                }
                delay = if failed {
                    (delay * 2).min(std::time::Duration::from_secs(5))
                } else {
                    std::time::Duration::from_millis(250)
                };
                // Only the caller owns handler lifetime between retry batches.
                drop(handler);
            }
        });
        Ok(())
    }

    /// Process a single target in receipt order. Original actor/ticket/method
    /// authority is rechecked under the same SQL guard as each WAL publication.
    /// This is also the restart driver; no caller-chosen replacement operation.
    pub async fn drain_receiver(&self, coglet: &str) -> Result<()> {
        let coglet = coglet.to_owned();
        let pin = self.pin(&coglet)?.clone();
        let permit = self.inner.receiver_drains[&coglet]
            .clone()
            .lock_owned()
            .await;
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            // Keep custody in the blocking worker even if its awaiting future is
            // cancelled. A second driver must never prepare the same step.
            let _permit = permit;
            let Some(setup) = handler.original_setup(&coglet)? else {
                return Ok(());
            };
            let service = handler.open_service(&coglet, &setup)?;
            // At most one full queue per batch; continuing admissions cannot
            // monopolize the worker or starve another configured target.
            for _ in 0..256 {
                let Some(request) = service
                    .lock()
                    .map_err(|_| "unavailable")?
                    .receiver_pending()
                else {
                    return Ok(());
                };
                let scope = receiver_scope(&request);
                let method = scope.method.clone();
                let id = request["correlation_id"]
                    .as_str()
                    .ok_or("invalid_argument")?
                    .to_owned();
                #[cfg(feature = "native-engine")]
                if method == "debug.step" {
                    handler.receiver_step_unlocked(&coglet, &pin, &request, &service, &setup)?;
                    continue;
                }
                let outcome = handler.inner.authority.runtime_boundary_operation(
                    handler.inner.config.hub,
                    handler.inner.config.device,
                    &coglet,
                    &Scope {
                        actor: scope.actor,
                        ticket: scope.ticket,
                    },
                    &Self::grant(&method, &coglet, pin.grant_generation),
                    pin.delegation_generation,
                    None,
                    Some(&id),
                    handler.clock_fn(),
                    |_, recheck| {
                        let mut locked = service.lock().map_err(|_| "unavailable")?;
                        let gate = || {
                            recheck()?;
                            if !pin.back_channel.permits(&method, &request) {
                                return Err("permission_denied".into());
                            }
                            Ok(())
                        };
                        if method == "debug.step" {
                            gate()?;
                            let preflight = handler.require_kill_clear().and_then(|()| {
                                admit_manifest(
                                    &setup["manifest"],
                                    &handler.inner.config.catalog,
                                    &handler.inner.config.grants,
                                )
                            });
                            if let Err(error) = preflight {
                                return locked.receiver_apply(&id, &|| Err(error.clone()));
                            }
                            if handler.enable_native(&coglet, &mut locked).is_err() {
                                return locked.receiver_apply(&id, &|| Err("unavailable".into()));
                            }
                            return locked.receiver_apply(&id, &|| Err("unavailable".into()));
                        }
                        locked.receiver_apply(&id, &gate)
                    },
                );
                match outcome {
                    Ok(()) => (),
                    Err(error) if error == "permission_denied" => {
                        service
                            .lock()
                            .map_err(|_| "unavailable")?
                            .receiver_apply(&id, &|| Err("permission_denied".into()))?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| "unavailable")?
    }
}
