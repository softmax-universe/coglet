//! Router action rejection at the actual receiver. These rejections never enter
//! the adapter dispatch ledger and never create an action outcome.
use super::*;

impl DeviceHandler {
    pub(super) async fn receive_action(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            handler.inner.authority.runtime_boundary_operation(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &Self::scope(&request),
                &Self::grant("hub.actions", &coglet, pin.grant_generation),
                pin.delegation_generation,
                None,
                Some(
                    request["correlation_id"]
                        .as_str()
                        .ok_or("invalid_argument")?,
                ),
                handler.clock_fn(),
                |_, recheck| {
                    deadline_gate(&context, recheck)?;
                    if request["params"]["action"]["coglet"] != coglet {
                        return Err("permission_denied".into());
                    }
                    let setup = handler
                        .original_setup(&coglet)?
                        .ok_or("permission_denied")?;
                    let service = handler.open_service(&coglet, &setup)?;
                    let mut locked = service.lock().map_err(|_| "unavailable")?;
                    deadline_gate(&context, recheck)?;
                    let result = locked.receiver_action_rejection(&request)?;
                    deadline_gate(&context, recheck)?;
                    Ok(result)
                },
            )
        })
        .await
        .map_err(|_| "unavailable")?
    }
}
