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
                    let gate = || deadline_gate(&context, recheck);
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
                    let result = match method {
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

    /// Process a single target in receipt order. Original actor/ticket/method
    /// authority is rechecked under the same SQL guard as each WAL publication.
    /// This is also the restart driver; no caller-chosen replacement operation.
    pub async fn drain_receiver(&self, coglet: &str) -> Result<()> {
        let coglet = coglet.to_owned();
        let pin = self.pin(&coglet)?.clone();
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            let Some(setup) = handler.original_setup(&coglet)? else {
                return Ok(());
            };
            let service = handler.open_service(&coglet, &setup)?;
            loop {
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
                            #[cfg(feature = "native-engine")]
                            return locked.receiver_native_step(
                                &id,
                                &handler.inner.config.grants["operations"],
                                &|| {
                                    gate()?;
                                    handler.require_kill_clear()
                                },
                            );
                            #[cfg(not(feature = "native-engine"))]
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
        })
        .await
        .map_err(|_| "unavailable")?
    }
}
