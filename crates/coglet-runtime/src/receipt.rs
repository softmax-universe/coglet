//! `hub.receipt` for the lifecycle operations this device admits.
//!
//! A receipt is a durable record committed in the same frame/transaction as the
//! admission it describes (managed WAL frame for install/start/stop, accepted-ref
//! transaction for `hub.git.push`). This module only reads those records.
//!
//! * Lookup is bound to the original actor, ticket, target and method. Another
//!   identity, another ticket, another target, a caller without the original
//!   method's grant and a never-admitted ID all return the same `not_found`.
//! * Records are returned verbatim. The device reports no applied generation
//!   (ADR-0019). Initial pushes complete only through the separate verified
//!   pre-install boundary driver (ADR-0026); accepted-ref presence alone is not
//!   completion. Installed-target pushes remain queued in this slice.
//! * Reading a receipt never executes, retries or mutates an operation.
use super::*;
use crate::execution::ReceiptScope;

/// Original scope of an admitted lifecycle request, stored with its receipt.
pub(super) fn receipt_scope(request: &Value) -> ReceiptScope {
    ReceiptScope {
        actor: request["context"]["actor"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        ticket: request["context"]["ticket"].as_str().map(str::to_owned),
        method: request["method"].as_str().unwrap_or_default().to_owned(),
    }
}

impl DeviceHandler {
    pub(super) async fn receipt(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let operation_id = request["params"]["operation_id"]
            .as_str()
            .filter(|id| validate("correlation-id", &json!(id)).is_ok())
            .ok_or("invalid_argument")?
            .to_owned();
        if request["context"]["actor"].as_str().is_none() {
            return Err("invalid_argument".into());
        }
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            handler.receipt_blocking(&context, &request, &coglet, &pin, &operation_id)
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    fn receipt_blocking(
        &self,
        context: &RequestContext,
        request: &Value,
        coglet: &str,
        pin: &TargetPin,
        operation_id: &str,
    ) -> Result<Value> {
        if tokio::time::Instant::now() >= context.deadline {
            return Err("unavailable".into());
        }
        let scope = Self::scope(request);
        // Caller authority first and independent of the record, so a caller with
        // no receipt grant learns nothing about any ID.
        self.inner.authority.runtime_boundary(
            self.inner.config.hub,
            self.inner.config.device,
            coglet,
            &scope,
            &Self::grant("hub.receipt", coglet, pin.grant_generation),
            pin.delegation_generation,
            None,
            self.clock_fn(),
            |_, recheck| deadline_gate(context, recheck).map(|_| ()),
        )?;
        let Some((method, receipt)) =
            self.find_receipt(coglet, &scope, operation_id, pin.grant_generation)?
        else {
            return Err("not_found".into());
        };
        if method == "hub.git.push" {
            // The ref-store lookup already applied the push scope/grant under one
            // database snapshot; push is not a delegation-gated runtime method.
            return deadline_gate(context, &|| Ok(0)).map(|_| receipt);
        }
        // Authorize again after the read and before the reply, under the
        // original method's grant. Losing that scope is indistinguishable from
        // never having been admitted.
        let result = self.inner.authority.runtime_boundary(
            self.inner.config.hub,
            self.inner.config.device,
            coglet,
            &scope,
            &Self::grant(&method, coglet, pin.grant_generation),
            pin.delegation_generation,
            None,
            self.clock_fn(),
            |_, recheck| {
                deadline_gate(context, recheck)?;
                validate("hub-mutation-receipt", &receipt)?;
                Ok(receipt)
            },
        );
        match result {
            Err(error) if error == "permission_denied" => Err("not_found".into()),
            other => other,
        }
    }

    /// `(original method, receipt)` for this exact scope. Reads only; the
    /// authority lock must not be held (the git store takes it briefly).
    fn find_receipt(
        &self,
        coglet: &str,
        scope: &Scope,
        operation_id: &str,
        grant_generation: u64,
    ) -> Result<Option<(String, Value)>> {
        let service = match (self.share_service(coglet), self.original_setup(coglet)?) {
            (Some(service), _) => Some(service),
            (None, Some(setup)) => Some(self.open_service(coglet, &setup)?),
            (None, None) => None,
        };
        if let Some(service) = service {
            let locked = service.lock().map_err(|_| "unavailable".to_owned())?;
            if let Some(found) =
                locked.managed_receipt_for(operation_id, &scope.actor, scope.ticket.as_deref())
            {
                return Ok(Some(found));
            }
        }
        Ok(self
            .inner
            .store
            .push_receipt_for_scope(
                self.inner.config.hub,
                coglet,
                operation_id,
                scope,
                grant_generation,
                self.clock_fn(),
            )
            .map_err(store_text)?
            .map(|receipt| ("hub.git.push".to_owned(), receipt)))
    }
}
