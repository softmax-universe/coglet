//! Authenticated router→device lifecycle. Authority and accepted refs are never
//! inferred from a remote proof; native execution is not implied by start.
use crate::{DurableStore, Result, RuntimeService, admit_manifest, digest, validate};
#[path = "initial_push.rs"]
mod initial_push;
#[path = "receipt.rs"]
mod receipt;
use coglet_proto::{
    authority::{
        Authority, Grant, Scope,
        accepted_refs::{Acceptance, AcceptedRefStore, Mode, PushOperation, StoreError},
    },
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    transfer::DurableSink,
    transport::{GitStream, OperationHandler, RequestContext, TransportError},
    validator::{Limits, ValidationError, Validator, VerifiedContents},
};
use iroh::EndpointId;
use receipt::receipt_scope;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

/// Raw `/code` bytes admitted into a WAL snapshot. Hex encoding roughly doubles
/// durable size; the WAL transaction cap is 64MiB.
pub const MAX_CODE_BYTES: usize = 8 * 1024 * 1024;
const QUEUE: usize = 128;

/// Immediate-before-effect gate: request deadline plus trusted-time recheck
/// against the authority windows captured under the SQL guard.
type Gate<'a> = &'a dyn Fn() -> Result<u64>;

/// Local operator pins. Private keys never belong here.
#[derive(Clone, Debug)]
pub struct DeviceConfig {
    pub hub: EndpointId,
    pub device: EndpointId,
    pub wal_root: PathBuf,
    pub quarantine: PathBuf,
    pub catalog: Value,
    pub grants: Value,
    pub kill_switch: Option<PathBuf>,
    pub targets: BTreeMap<String, TargetPin>,
    pub adapter: Option<AdapterConfig>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetPin {
    pub grant_generation: u64,
    pub delegation_generation: u64,
    #[serde(default)]
    pub native_schedule: Option<Value>,
}

/// Optional local adapter listener for one installed coglet. Sharing the service
/// `Arc` does not schedule Wasm by itself.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdapterConfig {
    pub socket: PathBuf,
    pub binding: PathBuf,
    pub uid: u32,
    pub coglet: String,
}

struct Inner {
    config: DeviceConfig,
    authority: Authority,
    store: AcceptedRefStore,
    validator: PathBuf,
    clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    services: Mutex<BTreeMap<String, Arc<Mutex<RuntimeService>>>>,
    adapter_started: AtomicBool,
}

/// Session handler for `hub.git.push` / install / start / stop / status.
#[derive(Clone)]
pub struct DeviceHandler {
    inner: Arc<Inner>,
}

impl DeviceHandler {
    /// `authority` must already contain peers, scopes, grants and the current
    /// delegation. This constructor never provisions or accepts a delegation.
    pub fn new(
        config: DeviceConfig,
        authority: Authority,
        validator: impl AsRef<Path>,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Result<Self> {
        if config.hub == config.device {
            return Err("hub and device pins must differ".into());
        }
        if !config.catalog.is_array() || !config.grants.is_object() {
            return Err("catalog must be an array and grants an object".into());
        }
        create_private_dir(&config.wal_root)?;
        create_private_dir(&config.quarantine)?;
        let store = AcceptedRefStore::new(authority.clone()).map_err(store_text)?;
        let handler = Self {
            inner: Arc::new(Inner {
                config,
                authority,
                store,
                validator: validator.as_ref().to_owned(),
                clock,
                services: Mutex::new(BTreeMap::new()),
                adapter_started: AtomicBool::new(false),
            }),
        };
        handler.resume_initial_pushes();
        Ok(handler)
    }

    pub fn config(&self) -> &DeviceConfig {
        &self.inner.config
    }

    /// Installed coordinator, if this process already opened it. Tests may share
    /// this `Arc` with an adapter listener; it is not a security bypass.
    pub fn share_service(&self, coglet: &str) -> Option<Arc<Mutex<RuntimeService>>> {
        self.inner.services.lock().ok()?.get(coglet).cloned()
    }

    fn pin(&self, coglet: &str) -> std::result::Result<&TargetPin, &'static str> {
        self.inner
            .config
            .targets
            .get(coglet)
            .ok_or("permission_denied")
    }

    fn pinned_peer(&self, context: &RequestContext) -> std::result::Result<(), &'static str> {
        if context.peer != self.inner.config.hub || context.local != self.inner.config.device {
            Err("permission_denied")
        } else {
            Ok(())
        }
    }

    fn scope(request: &Value) -> Scope {
        Scope {
            actor: request["context"]["actor"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            ticket: request["context"]["ticket"].as_str().map(str::to_owned),
        }
    }

    fn grant(method: &str, coglet: &str, generation: u64) -> Grant {
        Grant {
            method: method.into(),
            target: Some(coglet.into()),
            expires: 0,
            generation,
        }
    }

    fn clock_fn(&self) -> impl Fn() -> Option<u64> + Send + 'static {
        let clock = self.inner.clock.clone();
        move || clock()
    }

    fn wal(&self, coglet: &str) -> PathBuf {
        self.inner
            .config
            .wal_root
            .join(digest(coglet.as_bytes()))
            .join("state.wal")
    }

    async fn setup_from_bundle(
        &self,
        bytes: &[u8],
        metadata: &VerifiedRef,
        coglet: &str,
    ) -> Result<Value> {
        if bytes.is_empty() || bytes.len() > 64 * 1024 * 1024 {
            return Err("resource_exhausted".into());
        }
        let mut sink = BundleSink::create(&self.inner.config.quarantine, bytes.len() as u64)
            .await
            .map_err(|e| e.to_string())?;
        for chunk in bytes.chunks(32768) {
            DurableSink::append(&mut sink, chunk)
                .await
                .map_err(|e| e.to_string())?;
        }
        let hash = hex::encode(Sha256::digest(bytes));
        let envelope = sink
            .check_envelope(metadata, &hash, Limits::default().objects)
            .await
            .map_err(|e| e.to_string())?;
        let bundle = envelope
            .validate(&Validator::new(&self.inner.validator), Limits::default())
            .await
            .map_err(validation_text)?;
        setup_from_contents(bundle.contents(), coglet)
    }

    fn load_accepted(&self, coglet: &str, expected: &Value) -> Result<(VerifiedRef, Vec<u8>)> {
        let current = self
            .inner
            .store
            .current(&self.inner.config.hub.to_string(), coglet, RefKind::Hub)
            .map_err(store_text)?
            .ok_or("permission_denied")?;
        if current.metadata.signed() != expected {
            return Err("permission_denied".into());
        }
        let bytes = self
            .inner
            .store
            .audit_bundle(current.audit_id)
            .map_err(store_text)?;
        Ok((current.metadata, bytes))
    }

    fn peek_identity(&self, coglet: &str) -> Result<Value> {
        if let Some(service) = self.share_service(coglet) {
            let locked = service.lock().map_err(|_| "unavailable".to_owned())?;
            return locked
                .managed_identity()
                .cloned()
                .ok_or_else(|| "permission_denied".into());
        }
        let path = self.wal(coglet);
        if !path.is_file() {
            return Err("not_installed".into());
        }
        let store = DurableStore::open(&path)?;
        let Some(state) = store.state() else {
            return Err("not_installed".into());
        };
        state
            .pointer("/managed/identity")
            .cloned()
            .filter(Value::is_object)
            .ok_or_else(|| "permission_denied".into())
    }

    async fn recover_setup(&self, coglet: &str) -> Result<Option<(Value, Value)>> {
        let handler = self.clone();
        let coglet_owned = coglet.to_owned();
        let identity =
            match tokio::task::spawn_blocking(move || handler.peek_identity(&coglet_owned))
                .await
                .map_err(|_| "unavailable".to_owned())?
            {
                Ok(identity) => identity,
                Err(error) if error == "not_installed" => return Ok(None),
                Err(error) => return Err(error),
            };
        let initial = identity
            .get("initial")
            .cloned()
            .ok_or_else(|| "permission_denied".to_owned())?;
        let handler = self.clone();
        let coglet_owned = coglet.to_owned();
        let loaded = tokio::task::spawn_blocking(move || {
            let metadata = VerifiedRef::verify(
                &initial,
                RefKind::Hub,
                &handler.inner.config.hub.to_string(),
                &coglet_owned,
            )
            .map_err(|_| "permission_denied".to_owned())?;
            handler.load_accepted(&coglet_owned, metadata.signed())
        })
        .await
        .map_err(|_| "unavailable".to_owned())??;
        Ok(Some((
            identity,
            self.setup_from_bundle(&loaded.1, &loaded.0, coglet).await?,
        )))
    }

    fn apply_install(
        &self,
        coglet: &str,
        request: &Value,
        setup: &Value,
        proof: &Value,
        gate: Gate<'_>,
    ) -> Result<Value> {
        if request["params"]["delegation"] != *proof {
            return Err("permission_denied".into());
        }
        let pin = self.pin(coglet)?;
        if proof["statement"]["generation"] != pin.delegation_generation {
            return Err("permission_denied".into());
        }
        admit_manifest(
            &setup["manifest"],
            &self.inner.config.catalog,
            &self.inner.config.grants,
        )?;
        if setup["manifest"]["id"] != coglet || self.inner.config.grants["coglet"] != coglet {
            return Err("permission_denied".into());
        }
        let wasm = hex::decode(
            setup["initial_files"]["/code/bin/step.wasm"]
                .as_str()
                .ok_or("permission_denied")?,
        )
        .map_err(|_| "permission_denied".to_owned())?;
        let identity = json!({
            "initial": request["params"]["initial"],
            "delegation": proof,
            "live_commit": request["params"]["initial"]["statement"]["new"],
            "config": {
                "catalog": self.inner.config.catalog,
                "grants": self.inner.config.grants,
                "wasm_sha256": digest(&wasm)
            }
        });
        let operation_id = request["correlation_id"].as_str().unwrap();
        let receipt = {
            let mut services = self.inner.services.lock().map_err(|_| "unavailable")?;
            if let Some(service) = services.get(coglet) {
                let mut locked = service.lock().map_err(|_| "unavailable")?;
                gate()?;
                // A cached coordinator must already have its immutable install
                // identity. Never bind new metadata to stale unmanaged bytes.
                if locked.managed_identity().is_none() {
                    return Err("permission_denied".into());
                }
                locked.managed_install_scoped(
                    &identity,
                    operation_id,
                    Some(&receipt_scope(request)),
                )?
            } else {
                let path = self.wal(coglet);
                create_private_dir(path.parent().ok_or("invalid WAL path")?)?;
                let opened = RuntimeService::open_for_install(
                    &path,
                    setup,
                    QUEUE,
                    &identity,
                    operation_id,
                    &receipt_scope(request),
                    gate,
                )?;
                let receipt = opened
                    .managed_install_receipt()
                    .ok_or("unavailable")?
                    .clone();
                services.insert(coglet.to_owned(), Arc::new(Mutex::new(opened)));
                receipt
            }
        };
        validate("hub-mutation-receipt", &receipt)?;
        self.start_adapter(coglet);
        Ok(receipt)
    }

    fn apply_start(
        &self,
        coglet: &str,
        request: &Value,
        setup: &Value,
        gate: Gate<'_>,
    ) -> Result<Value> {
        admit_manifest(
            &setup["manifest"],
            &self.inner.config.catalog,
            &self.inner.config.grants,
        )?;
        self.require_kill_clear()?;
        let service = self.open_service(coglet, setup)?;
        let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
        if locked.managed_identity().is_none() {
            return Err("permission_denied".into());
        }
        // Waiting for the service mutex may outlive the authority windows.
        gate()?;
        let intent = self.operation_intent(request)?;
        let id = request["correlation_id"]
            .as_str()
            .ok_or("invalid_argument")?;
        if let Some(receipt) = locked.managed_operation_receipt(id, &intent)? {
            return Ok(receipt);
        }
        self.enable_native(coglet, &mut locked)?;
        // enable_native can be expensive and the kill switch may latch while
        // waiting: re-read both immediately before the effect.
        self.require_kill_clear()?;
        gate()?;
        let receipt = locked.managed_operation(id, &intent, true, 0, &receipt_scope(request))?;
        validate("hub-mutation-receipt", &receipt)?;
        Ok(receipt)
    }

    fn open_service(&self, coglet: &str, setup: &Value) -> Result<Arc<Mutex<RuntimeService>>> {
        let path = self.wal(coglet);
        if let Some(parent) = path.parent() {
            create_private_dir(parent)?;
        }
        let mut services = self
            .inner
            .services
            .lock()
            .map_err(|_| "unavailable".to_owned())?;
        if let Some(existing) = services.get(coglet) {
            return Ok(existing.clone());
        }
        let opened = RuntimeService::open(&path, setup, QUEUE)?;
        let shared = Arc::new(Mutex::new(opened));
        services.insert(coglet.to_owned(), shared.clone());
        Ok(shared)
    }

    fn operation_intent(&self, request: &Value) -> Result<String> {
        Ok(digest(&crate::cbor(&json!({
            "peer":self.inner.config.hub.to_string(),
            "method":request["method"], "target":request["target"],
            "params":request["params"], "context":request["context"]
        }))?))
    }

    fn enable_native(&self, coglet: &str, service: &mut RuntimeService) -> Result<()> {
        let schedule = self
            .pin(coglet)?
            .native_schedule
            .clone()
            .ok_or("native schedule required; no mock fallback")?;
        #[cfg(feature = "native-engine")]
        {
            service.enable_native(serde_json::from_value(schedule).map_err(|e| e.to_string())?)
        }
        #[cfg(not(feature = "native-engine"))]
        {
            let _ = (schedule, service);
            Err("native-engine feature unavailable; no mock fallback".into())
        }
    }

    fn require_kill_clear(&self) -> Result<()> {
        let Some(path) = &self.inner.config.kill_switch else {
            return Err("unavailable".into());
        };
        let raw = std::fs::read(path).map_err(|_| "unavailable".to_owned())?;
        let value: Value = serde_json::from_slice(&raw).map_err(|_| "unavailable".to_owned())?;
        match value {
            Value::Object(map)
                if map.len() == 1 && map.get("latched") == Some(&Value::Bool(false)) =>
            {
                Ok(())
            }
            Value::Object(map)
                if map.len() == 1 && map.get("latched") == Some(&Value::Bool(true)) =>
            {
                Err("permission_denied".into())
            }
            _ => Err("unavailable".into()),
        }
    }

    fn apply_stop(&self, coglet: &str, request: &Value, gate: Gate<'_>) -> Result<Value> {
        // Original private setup (never the newest pushed ref), read only after
        // authorization, under the same blocking authority boundary.
        let setup = self
            .original_setup(coglet)?
            .ok_or_else(|| "permission_denied".to_owned())?;
        let service = self.open_service(coglet, &setup)?;
        let mut locked = service.lock().map_err(|_| "unavailable".to_owned())?;
        let now = gate()?;
        let intent = self.operation_intent(request)?;
        let receipt = locked.managed_operation(
            request["correlation_id"]
                .as_str()
                .ok_or("invalid_argument")?,
            &intent,
            false,
            now,
            &receipt_scope(request),
        )?;
        validate("hub-mutation-receipt", &receipt)?;
        Ok(receipt)
    }

    fn apply_status(&self, coglet: &str, gate: Gate<'_>) -> Result<Value> {
        // Status reads the original private setup/WAL like stop; it never needs
        // the newest pushed (or retained-bundle) bytes.
        let service = match (self.share_service(coglet), self.original_setup(coglet)?) {
            (Some(service), _) => service,
            (None, Some(setup)) => self.open_service(coglet, &setup)?,
            (None, None) => {
                let status = json!({"state":"stopped","live_commit":Value::Null,"step":0,"pending":0,"error":Value::Null});
                validate("hub-status", &status)?;
                return Ok(status);
            }
        };
        let locked = service.lock().map_err(|_| "unavailable".to_owned())?;
        gate()?;
        if locked.managed_identity().is_none() {
            return Err("permission_denied".into());
        }
        let status = locked.managed_status();
        validate("hub-status", &status)?;
        Ok(status)
    }

    /// Original private setup (never the newest pushed ref). Blocking I/O.
    fn original_setup(&self, coglet: &str) -> Result<Option<Value>> {
        if let Some(service) = self.share_service(coglet) {
            return Ok(Some(
                service
                    .lock()
                    .map_err(|_| "unavailable".to_owned())?
                    .managed_setup()
                    .clone(),
            ));
        }
        let path = self.wal(coglet);
        if !path.is_file() {
            return Ok(None);
        }
        let store = DurableStore::open(&path)?;
        let Some(state) = store.state() else {
            return Ok(None);
        };
        state
            .pointer("/runtime/initial_setup")
            .cloned()
            .map(Some)
            .ok_or_else(|| "unavailable".into())
    }

    fn start_adapter(&self, coglet: &str) {
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
        {
            let Some(adapter) = &self.inner.config.adapter else {
                return;
            };
            if adapter.coglet != coglet {
                return;
            }
            if self.inner.adapter_started.swap(true, Ordering::SeqCst) {
                return;
            }
            let Some(service) = self.share_service(coglet) else {
                self.inner.adapter_started.store(false, Ordering::SeqCst);
                return;
            };
            let adapter = adapter.clone();
            let _ = std::thread::Builder::new()
                .name("cogletd-adapter".into())
                .spawn(move || {
                    if let Err(error) = bind_adapter(adapter, service) {
                        eprintln!("cogletd adapter listener: {error}");
                    }
                });
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
        let _ = coglet;
    }

    async fn handle_lifecycle(&self, context: RequestContext, request: Value) -> Value {
        if let Err(code) = self.pinned_peer(&context) {
            return fail(&request, code);
        }
        let Some(method) = request["method"].as_str() else {
            return fail(&request, "invalid_argument");
        };
        let Some(coglet) = request["target"].as_str().map(str::to_owned) else {
            return fail(&request, "invalid_argument");
        };
        match method {
            "hub.install" | "hub.start" | "hub.stop" | "hub.status" | "hub.receipt" => {}
            "hub.git.push" | "hub.git.fetch" | "hub.actions" | "hub.events" | "hub.inbox"
            | "hub.health" | "hub.cancel" | "debug.read" | "debug.write" | "debug.pause"
            | "debug.step" | "hub.enroll" => {
                return fail(&request, "permission_denied");
            }
            _ => return fail(&request, "invalid_argument"),
        }
        let Ok(pin) = self.pin(&coglet) else {
            return fail(&request, "permission_denied");
        };
        let pin = pin.clone();
        let result = match method {
            "hub.install" => self.install(context, request.clone(), coglet, pin).await,
            "hub.start" => self.start(context, request.clone(), coglet, pin).await,
            "hub.stop" => self.stop(context, request.clone(), coglet, pin).await,
            "hub.status" => self.status(context, request.clone(), coglet, pin).await,
            "hub.receipt" => self.receipt(context, request.clone(), coglet, pin).await,
            _ => Err("permission_denied".into()),
        };
        match result {
            Ok(result) => reply(&request, result),
            Err(error) => fail(&request, wire(&error)),
        }
    }

    async fn install(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let handler = self.clone();
        let expected = request["params"]["initial"].clone();
        let loaded = tokio::task::spawn_blocking(move || {
            let metadata = VerifiedRef::verify(
                &expected,
                RefKind::Hub,
                &handler.inner.config.hub.to_string(),
                &coglet,
            )
            .map_err(|_| "permission_denied".to_owned())?;
            // Initial install requires explicit completion custody, never just
            // accepted-ref presence. Existing installs retain their conflict and
            // original-ID retry semantics. The later SQL boundary pins the same
            // exact signed ref; completion evidence is immutable.
            if handler
                .peek_identity(&coglet)
                .as_ref()
                .is_err_and(|e| e == "not_installed")
                && !handler
                    .inner
                    .store
                    .preinstall_applied(handler.inner.config.hub, &coglet, metadata.signed())
                    .map_err(store_text)?
            {
                return Err("permission_denied".into());
            }
            handler
                .load_accepted(&coglet, metadata.signed())
                .map(|(metadata, bytes)| (coglet, metadata, bytes))
        })
        .await
        .map_err(|_| "unavailable".to_owned())??;
        let (coglet, metadata, bytes) = loaded;
        let setup = self.setup_from_bundle(&bytes, &metadata, &coglet).await?;
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            let scope = Self::scope(&request);
            let grant = Self::grant("hub.install", &coglet, pin.grant_generation);
            handler.inner.authority.runtime_boundary(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &scope,
                &grant,
                pin.delegation_generation,
                Some(&request["params"]["initial"]),
                handler.clock_fn(),
                |proof, recheck| {
                    let gate = || deadline_gate(&context, recheck);
                    gate()?;
                    handler.apply_install(&coglet, &request, &setup, proof, &gate)
                },
            )
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    async fn start(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let recovered = self
            .recover_setup(&coglet)
            .await?
            .ok_or_else(|| "permission_denied".to_owned())?;
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            let (identity, setup) = recovered;
            let scope = Self::scope(&request);
            let grant = Self::grant("hub.start", &coglet, pin.grant_generation);
            handler.inner.authority.runtime_boundary(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &scope,
                &grant,
                pin.delegation_generation,
                Some(&identity["initial"]),
                handler.clock_fn(),
                |proof, recheck| {
                    let gate = || deadline_gate(&context, recheck);
                    gate()?;
                    if identity.get("delegation") != Some(proof) {
                        return Err("permission_denied".into());
                    }
                    handler.apply_start(&coglet, &request, &setup, &gate)
                },
            )
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    async fn stop(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            // Stopping must not require accepting or executing replacement code.
            // Recover the original pinned setup from the private WAL (blocking
            // I/O and a possibly contended service mutex), not the newest ref.
            if tokio::time::Instant::now() >= context.deadline {
                return Err("unavailable".into());
            }
            let scope = Self::scope(&request);
            let grant = Self::grant("hub.stop", &coglet, pin.grant_generation);
            handler.inner.authority.runtime_boundary(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &scope,
                &grant,
                pin.delegation_generation,
                None,
                handler.clock_fn(),
                |_, recheck| {
                    let gate = || deadline_gate(&context, recheck);
                    gate()?;
                    handler.apply_stop(&coglet, &request, &gate)
                },
            )
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }

    async fn status(
        &self,
        context: RequestContext,
        request: Value,
        coglet: String,
        pin: TargetPin,
    ) -> Result<Value> {
        let handler = self.clone();
        tokio::task::spawn_blocking(move || {
            // Status reports the installed identity from the original private
            // setup; it never depends on the newest pushed/verified bytes.
            if tokio::time::Instant::now() >= context.deadline {
                return Err("unavailable".into());
            }
            let scope = Self::scope(&request);
            let grant = Self::grant("hub.status", &coglet, pin.grant_generation);
            handler.inner.authority.runtime_boundary(
                handler.inner.config.hub,
                handler.inner.config.device,
                &coglet,
                &scope,
                &grant,
                pin.delegation_generation,
                None,
                handler.clock_fn(),
                |_, recheck| {
                    let gate = || deadline_gate(&context, recheck);
                    gate()?;
                    handler.apply_status(&coglet, &gate)
                },
            )
        })
        .await
        .map_err(|_| "unavailable".to_owned())?
    }
}

#[allow(unreachable_code)]
fn bind_adapter(adapter: AdapterConfig, service: Arc<Mutex<RuntimeService>>) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "macos", target_os = "ios"))]
    {
        let binding = crate::SurfaceBinding::from_config(
            &std::fs::read(&adapter.binding).map_err(|e| e.to_string())?,
        )?;
        let server = crate::socket::SurfaceListener::bind(&adapter.socket, adapter.uid, binding)?
            .with_service(service)?;
        loop {
            if let Err(error) = server.serve_one() {
                eprintln!("adapter connection: {error}");
            }
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "ios")))]
    {
        let _ = (adapter, service);
        Err("adapter listener requires a Unix socket transport".into())
    }
}

fn deadline_gate(
    context: &RequestContext,
    recheck: &dyn Fn() -> std::result::Result<u64, String>,
) -> Result<u64> {
    if tokio::time::Instant::now() >= context.deadline {
        return Err("unavailable".into());
    }
    recheck()
}

fn create_private_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn setup_from_contents(contents: &VerifiedContents, coglet: &str) -> Result<Value> {
    let manifest: Value =
        serde_json::from_slice(contents.file("manifest.json").ok_or("permission_denied")?)
            .map_err(|_| "invalid_argument".to_owned())?;
    if manifest["id"] != coglet {
        return Err("permission_denied".into());
    }
    let mut initial_files = serde_json::Map::new();
    let mut total = 0usize;
    for (path, bytes) in contents.files() {
        if path.is_empty()
            || path.starts_with('/')
            || path
                .split('/')
                .any(|c| c.is_empty() || c == "." || c == "..")
        {
            return Err("permission_denied".into());
        }
        total = total
            .checked_add(bytes.len())
            .filter(|n| *n <= MAX_CODE_BYTES)
            .ok_or("resource_exhausted")?;
        initial_files.insert(format!("/code/{path}"), json!(hex::encode(bytes)));
    }
    if initial_files.get("/code/bin/step.wasm").is_none()
        || initial_files.get("/code/build/provenance.cbor").is_none()
    {
        return Err("permission_denied".into());
    }
    Ok(json!({
        "manifest": manifest,
        "initial_files": initial_files,
        "initial_effect_ids": {"actions":[],"timers":[],"llm_requests":[]},
        "initial_llm_requests": [],
        "initial_inbox_wakes": [],
        "initial_timers": [],
        "initial_actions": []
    }))
}

fn reply(request: &Value, result: Value) -> Value {
    json!({"type":"reply","correlation_id":request["correlation_id"],"result":result})
}
fn fail(request: &Value, code: &str) -> Value {
    json!({"type":"error","correlation_id":request["correlation_id"],
        "error":{"code":code,"message":code,"retryable":matches!(code,"unavailable"|"resource_exhausted")}})
}
fn transport(error: impl std::fmt::Display) -> TransportError {
    TransportError(error.to_string())
}
fn store_text(error: StoreError) -> String {
    store_code(&error).into()
}
fn store_code(error: &StoreError) -> &'static str {
    match error {
        StoreError::Conflict => "conflict",
        StoreError::Storage => "unavailable",
        StoreError::Authority(coglet_proto::authority::AuthorityError::PermissionDenied) => {
            "permission_denied"
        }
        StoreError::Authority(coglet_proto::authority::AuthorityError::Unauthenticated) => {
            "unauthenticated"
        }
        StoreError::Authority(coglet_proto::authority::AuthorityError::ResourceExhausted) => {
            "resource_exhausted"
        }
        StoreError::Authority(coglet_proto::authority::AuthorityError::InvalidArgument) => {
            "invalid_argument"
        }
        StoreError::Authority(coglet_proto::authority::AuthorityError::Storage) => "unavailable",
    }
}
fn validation_text(error: ValidationError) -> String {
    match error {
        ValidationError::Resource(_) => "resource_exhausted".into(),
        ValidationError::Unavailable(_) => "unavailable".into(),
        _ => "invalid_argument".into(),
    }
}
fn wire(error: impl std::fmt::Display) -> &'static str {
    match error.to_string().as_str() {
        "permission_denied" => "permission_denied",
        "unauthenticated" => "unauthenticated",
        "invalid_argument" => "invalid_argument",
        "conflict" => "conflict",
        "not_found" => "not_found",
        "resource_exhausted" => "resource_exhausted",
        "unavailable" | "authority storage unavailable" => "unavailable",
        other if other.starts_with("conflict:") => "conflict",
        _ => "unavailable",
    }
}

impl OperationHandler for DeviceHandler {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>> {
        Box::pin(self.handle_lifecycle(context, request))
    }

    fn handle_git<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
        mut stream: GitStream<'a>,
    ) -> Pin<Box<dyn Future<Output = std::result::Result<Value, TransportError>> + Send + 'a>> {
        Box::pin(async move {
            if request["method"] != "hub.git.push" {
                return Ok(fail(&request, "permission_denied"));
            }
            if let Err(code) = self.pinned_peer(&context) {
                return Ok(fail(&request, code));
            }
            let Some(coglet) = request["target"].as_str().map(str::to_owned) else {
                return Ok(fail(&request, "invalid_argument"));
            };
            let Ok(pin) = self.pin(&coglet) else {
                return Ok(fail(&request, "permission_denied"));
            };
            let metadata = match VerifiedRef::verify(
                &request["params"]["update"],
                RefKind::Hub,
                &self.inner.config.hub.to_string(),
                &coglet,
            ) {
                Ok(metadata) => metadata,
                Err(_) => return Ok(fail(&request, "permission_denied")),
            };
            let operation = match PushOperation::new(context.peer, &request) {
                Ok(operation) => operation,
                Err(error) => return Ok(fail(&request, store_code(&error))),
            };
            let handler = self.clone();
            let pre = metadata.clone();
            let op = operation.clone();
            let generation = pin.grant_generation;
            let request_for_pre = request.clone();
            let coglet_for_pre = coglet.clone();
            let precheck = tokio::task::spawn_blocking(move || {
                let scope = DeviceHandler::scope(&request_for_pre);
                let acceptance = Acceptance {
                    hub: handler.inner.config.hub,
                    coglet: coglet_for_pre,
                    kind: RefKind::Hub,
                    mode: Mode::Authoritative,
                    peer: handler.inner.config.hub,
                    scope: &scope,
                    grant_generation: generation,
                    delegation_generation: None,
                };
                handler
                    .inner
                    .store
                    .precheck_push(&pre, &acceptance, &op, handler.clock_fn())
                    .map_err(store_text)
            })
            .await
            .map_err(transport)?;
            if let Err(error) = precheck {
                return Ok(fail(&request, wire(&error)));
            }
            let sink = stream
                .receive_push(&metadata, &self.inner.config.quarantine)
                .await?;
            let envelope = sink
                .check_envelope(
                    &metadata,
                    request["params"]["sha256"].as_str().unwrap_or_default(),
                    Limits::default().objects,
                )
                .await
                .map_err(transport)?;
            let bundle = envelope
                .validate(&Validator::new(&self.inner.validator), Limits::default())
                .await
                .map_err(|e| transport(validation_text(e)))?;
            let handler = self.clone();
            let request_for_accept = request.clone();
            let target = coglet.clone();
            let receipt = tokio::task::spawn_blocking(move || {
                let scope = DeviceHandler::scope(&request_for_accept);
                let acceptance = Acceptance {
                    hub: handler.inner.config.hub,
                    coglet,
                    kind: RefKind::Hub,
                    mode: Mode::Authoritative,
                    peer: handler.inner.config.hub,
                    scope: &scope,
                    grant_generation: generation,
                    delegation_generation: None,
                };
                handler
                    .inner
                    .store
                    .accept_push(bundle, &acceptance, &operation, handler.clock_fn())
                    .map_err(store_text)
            })
            .await
            .map_err(transport)?
            .map_err(transport)?;
            let driver = self.clone();
            let id = request["correlation_id"]
                .as_str()
                .ok_or_else(|| transport("invalid_argument"))?
                .to_owned();
            // Dropping the requester/waiter does not cancel durable work.
            if let Err(error) =
                tokio::spawn(async move { driver.finish_initial_pushes(&target, Some(&id)).await })
                    .await
                    .map_err(transport)?
            {
                // Admission is durable even when boundary authority is absent.
                eprintln!("cogletd initial-push remains queued: {error}");
            }
            Ok(reply(&request, receipt))
        })
    }
}
