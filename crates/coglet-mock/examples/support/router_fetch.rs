//! Real observer fetches, byte-transparent fixture peer, independently reopened
//! accepted-ref/audit oracles. This qualifies observation, never activation.
use super::*;
use coglet_proto::{
    authority::{
        Scope,
        accepted_refs::{Acceptance, AcceptedRefStore, Mode},
    },
    git_metadata::{RefKind, VerifiedRef},
    quarantine::BundleSink,
    signing,
    transfer::DurableSink,
    transport::{FetchHandler, FetchResult, ReceivedTransfer, TransportError},
    validator::{Limits, Validator, VerifiedBundle},
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicUsize, Ordering},
};

type Work<'a, T> =
    Pin<Box<dyn Future<Output = std::result::Result<T, TransportError>> + Send + 'a>>;
fn error(e: impl std::fmt::Display) -> TransportError {
    TransportError(e.to_string())
}
const NOW: u64 = 1_800_000_000;

pub fn supported(case: &str) -> bool {
    ["published", "logs"].iter().any(|kind| {
        ["catch-up", "identical", "equal-conflict", "older"]
            .iter()
            .any(|mode| case == format!("fetch-{kind}-{mode}"))
    })
}

struct Observer {
    store: AcceptedRefStore,
    path: PathBuf,
    hub: EndpointId,
    peer: EndpointId,
    coglet: String,
    scope: Scope,
    kind: RefKind,
    accepted: AtomicUsize,
    precredit_conflicts: AtomicUsize,
}
impl Observer {
    fn context(&self) -> Acceptance<'_> {
        Acceptance {
            hub: self.hub,
            coglet: self.coglet.clone(),
            kind: self.kind,
            mode: Mode::Observation,
            peer: self.peer,
            scope: &self.scope,
            grant_generation: 0,
            delegation_generation: Some(1),
        }
    }
}
impl FetchHandler for Observer {
    fn authorize<'a>(
        &'a self,
        c: RequestContext,
        _: &'a Value,
        metadata: &'a VerifiedRef,
    ) -> Work<'a, ()> {
        Box::pin(async move {
            if c.local != self.hub || c.peer != self.peer {
                return Err(error("observer transport binding"));
            }
            let store = self.store.clone();
            let metadata = metadata.clone();
            let owned = self.clone_context();
            let result = tokio::task::spawn_blocking(move || {
                store.precheck(&metadata, &owned.context(), || Some(NOW))
            })
            .await
            .map_err(error)?;
            if matches!(
                result,
                Err(coglet_proto::authority::accepted_refs::StoreError::Conflict)
            ) {
                self.precredit_conflicts.fetch_add(1, Ordering::SeqCst);
            }
            result.map_err(error)
        })
    }
    fn accept<'a>(
        &'a self,
        _: RequestContext,
        _: Value,
        metadata: VerifiedRef,
        transfer: ReceivedTransfer,
        sink: BundleSink,
    ) -> Work<'a, Value> {
        Box::pin(async move {
            let bundle = sink
                .check_envelope(&metadata, transfer.sha256(), 100000)
                .await
                .map_err(error)?
                .validate(&validator(), Limits::default())
                .await
                .map_err(error)?;
            let owned = self.clone_context();
            let receipt = tokio::task::spawn_blocking(move || {
                owned
                    .store
                    .accept(bundle, &owned.context(), || Some(NOW))
                    .map_err(error)
            })
            .await
            .map_err(error)??;
            self.accepted.fetch_add(1, Ordering::SeqCst);
            Ok(json!({"changed":receipt.changed,"audit_id":receipt.audit_id}))
        })
    }
}
impl Observer {
    fn clone_context(&self) -> Self {
        Self {
            store: self.store.clone(),
            path: self.path.clone(),
            hub: self.hub,
            peer: self.peer,
            coglet: self.coglet.clone(),
            scope: self.scope.clone(),
            kind: self.kind,
            accepted: AtomicUsize::new(0),
            precredit_conflicts: AtomicUsize::new(0),
        }
    }
}
fn validator() -> Validator {
    Validator::new(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/coglet-git-validator"),
    )
}
async fn validated(root: &Path, row: &Value, metadata: &VerifiedRef) -> Result<VerifiedBundle> {
    let bytes = hex::decode(row["bundle_hex"].as_str().ok_or("missing bundle")?)?;
    let mut sink = BundleSink::create(root, bytes.len() as u64).await?;
    sink.append(&bytes).await?;
    Ok(sink
        .check_envelope(
            metadata,
            row["sha256"].as_str().ok_or("missing sha")?,
            100000,
        )
        .await?
        .validate(&validator(), Limits::default())
        .await?)
}

pub async fn run(router: Backend, case: &str) -> Result<()> {
    let data: Value = serde_json::from_slice(&std::fs::read("spec/fixtures/hub/fetch.json")?)?;
    let kind = if case.starts_with("fetch-logs-") {
        RefKind::Logs
    } else {
        RefKind::Published
    };
    let rows = data.as_array().ok_or("fetch fixture array")?;
    let profile = rows
        .iter()
        .find(|row| row["ref"] == kind.reference())
        .ok_or("fetch profile")?;
    let catch_up = case.ends_with("catch-up");
    let negative = case.ends_with("older") || case.ends_with("equal-conflict");
    let seed = &profile["history"][if catch_up { 0 } else { 2 }];
    let final_row = &profile["history"][2];
    let target = final_row["update"]["statement"]["coglet"]
        .as_str()
        .ok_or("target")?
        .to_owned();
    let device = SecretKey::from_bytes(&[3; 32]).public();
    router.kernel().open_window(Duration::from_secs(60)).await?;
    let grant = Grant {
        method: "hub.git.fetch".into(),
        target: Some(target.clone()),
        expires: 4102444800,
        generation: 0,
    };
    router
        .kernel()
        .test_invite(
            device,
            "11".repeat(32),
            Duration::from_secs(60),
            vec![grant.clone()],
        )
        .await?;
    let provisioning = Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[3; 32]))
        .bind()
        .await?;
    let connection = provisioning.connect(router.addr(), ALPN).await?;
    let initial = Session::new(connection, device, Role::Device)?;
    enroll(&initial, Some(&"11".repeat(32)), "ff").await?;
    provisioning.close().await;
    let hub = router.addr().id;
    let path = router.state_path().join("authority.db");
    let scope = Scope {
        actor: format!("hub:{hub}"),
        ticket: None,
    };
    let authority = Authority::open(&path)?;
    authority.provision_scope(device, &scope, &grant)?;
    let key = SecretKey::from_bytes(&[1; 32]);
    let statement = json!({"purpose":"delegation","hub":hub.to_string(),"coglet":target,"device":device.to_string(),"generation":1,"expires":4102444800u64,"methods":["hub.git.fetch"]});
    let delegation = json!({"statement":statement,"signer":key.public().to_string(),"signature":hex::encode(key.sign(&signing::preimage("delegation", &statement)?).to_bytes())});
    authority.accept_delegation(hub, device, &target, &delegation, || Some(NOW))?;
    let observer = Arc::new(Observer {
        store: AcceptedRefStore::new(authority)?,
        path,
        hub,
        peer: device,
        coglet: target.clone(),
        scope,
        kind,
        accepted: AtomicUsize::new(0),
        precredit_conflicts: AtomicUsize::new(0),
    });
    let quarantine = tempfile::tempdir()?;
    let metadata = VerifiedRef::verify(&seed["update"], kind, &hub.to_string(), &target)?;
    let bundle = validated(quarantine.path(), seed, &metadata).await?;
    let owned = observer.clone();
    tokio::task::spawn_blocking(move || owned.store.accept(bundle, &owned.context(), || Some(NOW)))
        .await??;
    // Runner starts the real iroh driver only after trusted fixture preparation.
    // It sends GO only after the unchanged enrollment match has completed, so
    // cross-stream scheduling cannot race that reply with the reverse request.
    let addr = router.addr();
    let address = addr
        .ip_addrs()
        .find(|a| a.ip().is_loopback())
        .or_else(|| addr.ip_addrs().next())
        .ok_or("missing direct address")?;
    println!(
        "{}",
        json!({"endpoint":hub.to_string(),"address":address.to_string()})
    );
    let mut trigger = [0u8; 1];
    tokio::io::AsyncReadExt::read_exact(&mut tokio::io::stdin(), &mut trigger).await?;
    if trigger != *b"G" {
        return Err("missing post-enrollment trigger".into());
    }
    let hs = router
        .session(device)
        .await
        .ok_or("missing enrolled transport")?;
    let request = json!({"method":"hub.git.fetch","target":target,"correlation_id":"aa".repeat(16),"timeout_ms":5000,"context":{"actor":observer.scope.actor},"params":{"ref":kind.reference()}});
    let result = hs
        .fetch_git(request, quarantine.path(), observer.as_ref())
        .await;
    if negative {
        let error = result.err().ok_or("conflict unexpectedly accepted")?;
        if !error.0.contains("conflict")
            || observer.precredit_conflicts.load(Ordering::SeqCst) != 1
            || observer.accepted.load(Ordering::SeqCst) != 0
        {
            return Err("pre-credit conflict oracle failed".into());
        }
    } else {
        let FetchResult::Accepted(receipt) = result? else {
            return Err("fetch rejected".into());
        };
        if receipt["changed"] != catch_up || observer.accepted.load(Ordering::SeqCst) != 1 {
            return Err("fetch acceptance oracle failed".into());
        }
    }
    let path = observer.path.clone();
    let hub = hub.to_string();
    let target = target.clone();
    let expected = final_row.clone();
    let expected_seed = seed.clone();
    let expected_peer = device.to_string();
    let expected_actor = observer.scope.actor.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let store = AcceptedRefStore::new(Authority::open(&path)?)?;
        let current = store.current(&hub, &target, kind)?.ok_or("missing accepted ref")?;
        if current.metadata.generation() != 3 || *current.metadata.signed() != expected["update"] { return Err("durable watermark/statement oracle".into()); }
        if store.audit_bundle(current.audit_id)? != hex::decode(expected["bundle_hex"].as_str().ok_or("bundle")?)? { return Err("durable bundle oracle".into()); }
        let db = rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let mut query = db.prepare("SELECT hub,coglet,ref,mode,signed,peer,actor,ticket,grant_generation,delegation_generation,accepted_at FROM proto_ref_audit ORDER BY id")?;
        let rows = query.query_map([], |r| (0..11).map(|i| r.get::<_, Option<String>>(i)).collect::<std::result::Result<Vec<_>, _>>())?.collect::<std::result::Result<Vec<_>, _>>()?;
        let statements = if catch_up { vec![&expected_seed["update"], &expected["update"]] } else { vec![&expected["update"]] };
        if rows.len() != statements.len() { return Err("durable accepted-observation audit count".into()); }
        for (row, signed) in rows.iter().zip(statements) {
            let expected_fields = vec![Some(hub.clone()),Some(target.clone()),Some(kind.reference().into()),Some("observation".into()),row[4].clone(),Some(expected_peer.clone()),Some(expected_actor.clone()),None,Some("0".into()),Some("1".into()),Some(NOW.to_string())];
            if *row != expected_fields || serde_json::from_str::<Value>(row[4].as_deref().ok_or("missing signed audit")?)? != *signed { return Err("durable audit provenance/statement oracle".into()); }
        }
        Ok(())
    }).await??;
    if std::fs::read_dir(quarantine.path())?.next().is_some() {
        return Err("quarantine not cleaned".into());
    }
    // Retain the transport until the driver has observed FIN/reset; endpoint
    // shutdown is not an acceptable substitute for the operation reset oracle.
    tokio::io::AsyncReadExt::read_exact(&mut tokio::io::stdin(), &mut trigger).await?;
    if trigger != *b"D" {
        return Err("missing driver completion".into());
    }
    router.close().await?;
    Ok(())
}
