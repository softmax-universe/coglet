//! Explicitly published, coglet-signed snapshots. No live private tree export.
//! Local provisioning supplies complete immutable bundles; this receiver never
//! holds a coglet signing key and never manufactures a publication from /data.
use super::*;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishedSnapshot {
    pub update: Value,
    pub bundle_hex: String,
    pub sha256: String,
}

impl DeviceHandler {
    fn fetch_authority(
        &self,
        context: &RequestContext,
        request: &Value,
        pin: &TargetPin,
    ) -> Result<()> {
        let coglet = request["target"].as_str().ok_or("invalid_argument")?;
        self.inner.authority.runtime_boundary(
            self.inner.config.hub,
            self.inner.config.device,
            coglet,
            &Self::scope(request),
            &Self::grant("hub.git.fetch", coglet, pin.grant_generation),
            pin.delegation_generation,
            None,
            self.clock_fn(),
            |_, recheck| deadline_gate(context, recheck).map(|_| ()),
        )
    }

    pub(super) async fn fetch_snapshot(
        &self,
        context: RequestContext,
        request: Value,
        mut stream: GitStream<'_>,
    ) -> std::result::Result<Value, TransportError> {
        if validate("hub-request", &request).is_err() {
            return Ok(fail(&request, "invalid_argument"));
        }
        if let Err(code) = self.pinned_peer(&context) {
            return Ok(fail(&request, code));
        }
        let Some(coglet) = request["target"].as_str() else {
            return Ok(fail(&request, "invalid_argument"));
        };
        let Ok(pin) = self.pin(coglet) else {
            return Ok(fail(&request, "permission_denied"));
        };
        // Scope is checked before even selecting a publication or its metadata.
        if let Err(error) = self.fetch_authority(&context, &request, pin) {
            return Ok(fail(&request, wire(error)));
        }
        let reference = request["params"]["ref"].as_str().unwrap_or_default();
        let kind = match reference {
            "refs/heads/logs" => RefKind::Logs,
            "refs/published/main" => RefKind::Published,
            _ => return Ok(fail(&request, "permission_denied")),
        };
        let Some(snapshot) = pin.fetch_snapshots.get(reference) else {
            return Ok(fail(&request, "not_found"));
        };
        let checked = async {
            let metadata = VerifiedRef::verify(
                &snapshot.update,
                kind,
                &self.inner.config.hub.to_string(),
                coglet,
            )
            .map_err(|_| "permission_denied".to_owned())?;
            if snapshot.bundle_hex.is_empty() || snapshot.bundle_hex.len() > 128 * 1024 * 1024 {
                return Err("resource_exhausted".into());
            }
            let bytes =
                hex::decode(&snapshot.bundle_hex).map_err(|_| "invalid_argument".to_owned())?;
            if digest(&bytes) != snapshot.sha256 {
                return Err("invalid_argument".into());
            }
            let mut sink = BundleSink::create(&self.inner.config.quarantine, bytes.len() as u64)
                .await
                .map_err(|e| e.to_string())?;
            for chunk in bytes.chunks(32768) {
                DurableSink::append(&mut sink, chunk)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            let envelope = sink
                .check_envelope(&metadata, &snapshot.sha256, Limits::default().objects)
                .await
                .map_err(|e| e.to_string())?;
            // Validates the exact signed archive, logs-only allowlist, object
            // bounds and provenance for code, not just the outer signature.
            let verified = envelope
                .validate(&Validator::new(&self.inner.validator), Limits::default())
                .await
                .map_err(validation_text)?;
            drop(verified);
            self.fetch_authority(&context, &request, pin)?;
            Ok::<_, String>((metadata, bytes))
        }
        .await;
        let (metadata, bytes) = match checked {
            Ok(v) => v,
            Err(e) => return Ok(fail(&request, wire(e))),
        };
        stream
            .send_fetch_guarded(
                &metadata,
                bytes.len() as u64,
                &snapshot.sha256,
                &mut bytes.as_slice(),
                &|| {
                    self.fetch_authority(&context, &request, pin)
                        .map_err(transport)
                },
            )
            .await?;
        self.fetch_authority(&context, &request, pin)
            .map_err(transport)?;
        Ok(reply(&request, json!({})))
    }
}
