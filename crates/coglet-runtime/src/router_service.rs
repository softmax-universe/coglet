//! Device-side router session. Operator config only; never log private keys.
use coglet_proto::{
    authority::Authority,
    transport::{ALPN, Role, Session},
};
use coglet_runtime::{AdapterConfig, DeviceConfig, DeviceHandler, ProducerProfile, TargetPin};
use iroh::{Endpoint, EndpointAddr, SecretKey, endpoint::presets};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    hub: String,
    device: String,
    identity_path: PathBuf,
    authority_path: PathBuf,
    validator_path: PathBuf,
    wal_root: PathBuf,
    quarantine: PathBuf,
    catalog: Value,
    grants: Value,
    #[serde(default)]
    kill_switch: Option<PathBuf>,
    #[serde(default)]
    invite: Option<String>,
    hub_addr: SocketAddr,
    targets: BTreeMap<String, TargetPin>,
    #[serde(default)]
    adapter: Option<AdapterConfig>,
    /// ADR-0033 opt-in local producer interface; absent = legacy behavior.
    #[serde(default)]
    profile: Option<ProducerProfile>,
}

pub fn run(path: &str) -> Result<(), String> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_io()
        .enable_time()
        .build()
        .map_err(|e| e.to_string())?
        .block_on(run_async(Path::new(path)))
}

async fn run_async(path: &Path) -> Result<(), String> {
    let config: FileConfig = serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let hub = config.hub.parse().map_err(|e| format!("{e}"))?;
    let device = config.device.parse().map_err(|e| format!("{e}"))?;
    let key = load_key(&config.identity_path)?;
    if key.public() != device {
        return Err("identity does not match pinned device".into());
    }
    let authority = Authority::open(&config.authority_path).map_err(|e| e.to_string())?;
    let handler = DeviceHandler::new(
        DeviceConfig {
            hub,
            device,
            wal_root: config.wal_root,
            quarantine: config.quarantine,
            catalog: config.catalog,
            grants: config.grants,
            kill_switch: config.kill_switch,
            targets: config.targets,
            adapter: config.adapter,
            profile: config.profile,
        },
        authority,
        &config.validator_path,
        Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }),
    )?;
    handler.recover_running().await?;
    // Bind the emergency revoke endpoint BEFORE contacting the hub: custody must
    // not depend on router connectivity. All output gates are already closed.
    handler.start_producer_endpoint(tokio::runtime::Handle::current())?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| e.to_string())?;
    if endpoint.id() != device {
        return Err("bound endpoint does not match pinned device".into());
    }
    let connection = tokio::time::timeout(
        Duration::from_secs(15),
        endpoint.connect(EndpointAddr::new(hub).with_ip_addr(config.hub_addr), ALPN),
    )
    .await
    .map_err(|_| "hub connect timed out".to_owned())?
    .map_err(|e| e.to_string())?;
    let session =
        Session::new(connection, endpoint.id(), Role::Device).map_err(|e| e.to_string())?;
    enroll(&session, config.invite.as_deref()).await?;
    println!(
        "{}",
        json!({"device":device.to_string(),"hub":hub.to_string()})
    );
    let serve = session.serve(Arc::new(handler));
    let result = serve.await;
    endpoint.close().await;
    result.map_err(|e| e.to_string())
}

async fn enroll(session: &Session, invite: Option<&str>) -> Result<(), String> {
    if let Some(invite) = invite {
        coglet_runtime::validate("hub-secret", &json!(invite))?;
    }
    let id = hex::encode(&SecretKey::generate().to_bytes()[..16]);
    let reply = session
        .request(
            json!({"method":"hub.enroll","correlation_id":id,"target":Value::Null,"timeout_ms":5000,
            "params":{"offer":{"versions":["0.1"]},"invite":invite}}),
        )
        .await
        .map_err(|_| "enrollment transport failed (details redacted)".to_owned())?;
    if reply["type"] != "reply" {
        return Err(
            "enrollment refused (use null invite to recover a lost committed success)".into(),
        );
    }
    if reply["result"]["hub"] != session.peer().to_string() {
        return Err("enrollment hub binding mismatch".into());
    }
    Ok(())
}

fn load_key(path: &Path) -> Result<SecretKey, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
            return Err("identity must be a private regular file".into());
        }
    }
    let raw = fs::read(path).map_err(|e| e.to_string())?;
    let bytes: [u8; 32] = if raw.len() == 32 {
        raw.try_into().map_err(|_| "invalid identity key")?
    } else {
        let text = String::from_utf8(raw).map_err(|_| "invalid identity key")?;
        hex::decode(text.trim())
            .map_err(|_| "invalid identity key")?
            .try_into()
            .map_err(|_| "invalid identity key")?
    };
    Ok(SecretKey::from_bytes(&bytes))
}
