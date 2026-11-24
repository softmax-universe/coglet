//! Installed router pushes: verify outside SQL custody, then serialize with the
//! runtime's step mutex under the authority write guard. No guest runs here.
use super::*;
use crate::execution::CodeRevision;

impl DeviceHandler {
    pub(super) async fn finish_installed_pushes(&self, coglet: &str) -> Result<()> {
        let handler = self.clone();
        let target = coglet.to_owned();
        let pending = tokio::task::spawn_blocking(move || {
            handler
                .inner
                .store
                .pending_installed_pushes(handler.inner.config.hub, &target)
                .map_err(store_text)
        })
        .await
        .map_err(|_| "unavailable".to_owned())??;
        for pending in pending {
            let pin = self.pin(coglet)?.clone();
            let bundle = async {
                let mut sink =
                    BundleSink::create(&self.inner.config.quarantine, pending.bundle.len() as u64)
                        .await
                        .map_err(|_| "unavailable".to_owned())?;
                for chunk in pending.bundle.chunks(32768) {
                    DurableSink::append(&mut sink, chunk)
                        .await
                        .map_err(|_| "unavailable".to_owned())?;
                }
                sink.check_envelope(
                    &pending.metadata,
                    &hex::encode(Sha256::digest(&pending.bundle)),
                    Limits::default().objects,
                )
                .await
                .map_err(|_| "invalid_argument".to_owned())?
                .validate(&Validator::new(&self.inner.validator), Limits::default())
                .await
                .map_err(validation_text)
            }
            .await;
            // Transient validator/storage failures retain admission custody for
            // recovery. Deterministic invalid artifacts get a durable failure.
            let bundle = match bundle {
                Ok(bundle) => Some(bundle),
                Err(error) if error == "invalid_argument" => None,
                Err(error) => return Err(error),
            };
            let setup = bundle
                .as_ref()
                .map(|b| setup_from_contents(b.contents(), coglet))
                .transpose();
            let (bundle, setup) = match setup {
                Ok(Some(setup)) => (bundle, setup),
                Ok(None) | Err(_) => (None, Value::Null),
            };
            let handler = self.clone();
            let target = coglet.to_owned();
            tokio::task::spawn_blocking(move || {
                let scope = Self::scope(&pending.request);
                let op = PushOperation::new(handler.inner.config.hub, &pending.request)
                    .map_err(store_text)?;
                let revision = CodeRevision {
                    setup,
                    signed_ref: pending.metadata.signed().clone(),
                    operation_id: pending.request["correlation_id"]
                        .as_str()
                        .ok_or("invalid_argument")?
                        .to_owned(),
                    intent: handler.operation_intent(&pending.request)?,
                    scope: receipt_scope(&pending.request),
                };
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
                    .reconcile_installed_push(
                        bundle,
                        &acceptance,
                        &op,
                        handler.inner.config.device,
                        pin.delegation_generation,
                        handler.clock_fn(),
                        |gate| {
                            let root = handler.original_setup(&target)?.ok_or("not installed")?;
                            let service = handler.open_service(&target, &root)?;
                            let mut locked =
                                service.lock().map_err(|_| "unavailable".to_owned())?;
                            // Copy immutable identity before the mutable transition.
                            // Rotation cannot silently adopt a pending application's
                            // old installed delegation generation.
                            let generation_ok = locked
                                .managed_identity()
                                .and_then(|i| i.pointer("/delegation/statement/generation"))
                                .and_then(Value::as_u64)
                                == Some(pin.delegation_generation);
                            let fresh = || {
                                let now = gate()?;
                                if !generation_ok {
                                    return Err("permission_denied".into());
                                }
                                admit_manifest(
                                    &revision.setup["manifest"],
                                    &handler.inner.config.catalog,
                                    &handler.inner.config.grants,
                                )
                                .map_err(|_| "permission_denied".to_owned())?;
                                Ok(now)
                            };
                            locked.complete_code_revision(&revision, &fresh)
                        },
                    )
                    .map_err(store_text)
            })
            .await
            .map_err(|_| "unavailable".to_owned())??;
        }
        Ok(())
    }
}
