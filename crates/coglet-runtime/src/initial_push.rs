//! ADR-0026 initial-deploy boundary driver. Never activates installed code.
use super::*;

impl DeviceHandler {
    /// Recovery is runtime-owned, independent of a receipt waiter. It retains
    /// original requests and revalidates bytes plus current authority. Failure
    /// leaves queued custody for explicit identical retry/restart reconciliation.
    pub(super) fn resume_initial_pushes(&self) -> Result<()> {
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|_| "device recovery requires an active Tokio runtime".to_owned())?;
        {
            let handler = self.clone();
            runtime.spawn(async move {
                for coglet in handler.inner.config.targets.keys() {
                    if let Err(error) = handler.finish_initial_pushes(coglet, None).await {
                        eprintln!("cogletd initial-push reconciliation: {error}");
                    }
                    if let Err(error) = handler.drain_receiver(coglet).await {
                        eprintln!("cogletd back-channel reconciliation: {error}");
                    }
                }
            });
        }
        Ok(())
    }

    pub(super) async fn finish_initial_pushes(&self, coglet: &str, id: Option<&str>) -> Result<()> {
        let handler = self.clone();
        let target = coglet.to_owned();
        if tokio::task::spawn_blocking(move || handler.original_setup(&target))
            .await
            .map_err(|_| "unavailable".to_owned())??
            .is_some()
        {
            return self.finish_installed_pushes(coglet).await;
        }
        let handler = self.clone();
        let target = coglet.to_owned();
        let pending = tokio::task::spawn_blocking(move || {
            handler
                .inner
                .store
                .pending_pushes(handler.inner.config.hub, &target)
                .map_err(store_text)
        })
        .await
        .map_err(|_| "unavailable".to_owned())??;
        for pending in pending {
            if id.is_some_and(|id| pending.request["correlation_id"] != id) {
                continue;
            }
            let pin = self.pin(coglet)?.clone();
            // A new validator pass creates a genuine VerifiedBundle after
            // admission (and after restart); retained SQL presence is not proof.
            let mut sink =
                BundleSink::create(&self.inner.config.quarantine, pending.bundle.len() as u64)
                    .await
                    .map_err(|_| "unavailable".to_owned())?;
            for chunk in pending.bundle.chunks(32768) {
                DurableSink::append(&mut sink, chunk)
                    .await
                    .map_err(|_| "unavailable".to_owned())?;
            }
            let envelope = sink
                .check_envelope(
                    &pending.metadata,
                    &hex::encode(Sha256::digest(&pending.bundle)),
                    Limits::default().objects,
                )
                .await
                .map_err(|_| "invalid_argument".to_owned())?;
            let bundle = envelope
                .validate(&Validator::new(&self.inner.validator), Limits::default())
                .await
                .map_err(validation_text)?;
            // Check exact target manifest and bounded installable file layout,
            // without instantiating Wasm or creating private state.
            setup_from_contents(bundle.contents(), coglet)?;
            let handler = self.clone();
            let target = coglet.to_owned();
            tokio::task::spawn_blocking(move || {
                let scope = Self::scope(&pending.request);
                let op = PushOperation::new(handler.inner.config.hub, &pending.request)
                    .map_err(store_text)?;
                let acceptance = Acceptance {
                    hub: handler.inner.config.hub,
                    coglet: target.clone(),
                    kind: RefKind::Hub,
                    mode: Mode::Authoritative,
                    peer: handler.inner.config.hub,
                    scope: &scope,
                    grant_generation: pin.grant_generation,
                    delegation_generation: None,
                };
                handler
                    .inner
                    .store
                    .complete_preinstall_push(
                        bundle,
                        &acceptance,
                        &op,
                        handler.inner.config.device,
                        pin.delegation_generation,
                        handler.clock_fn(),
                        || handler.original_setup(&target).map(|setup| setup.is_none()),
                    )
                    .map_err(store_text)
            })
            .await
            .map_err(|_| "unavailable".to_owned())??;
        }
        Ok(())
    }
}
