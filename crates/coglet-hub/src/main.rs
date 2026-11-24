//! Operator-only startup configuration is not a wire RPC or a guest input.
use coglet_hub::{Kernel, Registration, operator::socket::StateLock};
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
};
use iroh::{Endpoint, SecretKey, endpoint::presets};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Config {
    #[serde(default)]
    peers: Vec<Peer>,
    #[serde(default)]
    coglets: Vec<Registration>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Peer {
    device: String,
    grants: Vec<Grant>,
}
fn identity(root: &Path) -> Result<(SecretKey, StateLock), Box<dyn std::error::Error>> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    fs::create_dir_all(root)?;
    if !fs::symlink_metadata(root)?.is_dir() {
        return Err("state directory must not be a symlink".into());
    }
    // The containing directory protects SQLite, journals and local provisioning.
    fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
    let lock = StateLock::acquire(root)?;
    let path = root.join("hub.key");
    let missing = match fs::symlink_metadata(&path) {
        Ok(meta) => {
            if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
                return Err("hub.key must be a private regular file".into());
            }
            false
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };
    if missing {
        let key = SecretKey::generate();
        let staging = root.join(format!(".hub.key.{}.pending", key.public()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staging)?;
        file.write_all(hex::encode(key.to_bytes()).as_bytes())?;
        file.sync_all()?;
        fs::rename(staging, &path)?;
        fs::File::open(root)?.sync_all()?;
    }
    let meta = fs::symlink_metadata(&path)?;
    if !meta.is_file() || meta.permissions().mode() & 0o077 != 0 {
        return Err("hub.key must be a private regular file".into());
    }
    let bytes: [u8; 32] = hex::decode(fs::read_to_string(path)?)?
        .try_into()
        .map_err(|_| "invalid identity key")?;
    Ok((SecretKey::from_bytes(&bytes), lock))
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3
        || !matches!(args[1].as_str(), "identity" | "serve")
        || (args[1] == "identity" && args.len() != 3)
        || (args[1] == "serve" && args.len() != 4)
    {
        return Err(
            "usage: coglet-hub identity STATE_DIR | coglet-hub serve STATE_DIR CONFIG.json".into(),
        );
    }
    let root = Path::new(&args[2]);
    let (key, _state_lock) = identity(root)?;
    if args[1] == "identity" {
        println!("{}", json!({"hub":key.public().to_string()}));
        return Ok(());
    }
    let config: Config = serde_json::from_slice(&fs::read(&args[3])?)?;
    let endpoint = Endpoint::builder(presets::Minimal)
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .bind()
        .await?;
    let kernel = Kernel::open(
        root,
        endpoint.id(),
        Arc::new(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .map(|d| d.as_secs())
        }),
    )?;
    let mut invites = BTreeMap::new();
    if !config.peers.is_empty() {
        kernel.open_window(Duration::from_secs(60)).await?;
    }
    for peer in config.peers {
        let id = peer.device.parse()?;
        let invite = kernel
            .invite(id, Duration::from_secs(60), peer.grants)
            .await?;
        invites.insert(peer.device, invite);
    }
    for coglet in config.coglets {
        kernel.register(coglet).await?;
    }
    // Explicit local operator bootstrap output, not a log: redirect stdout to a
    // private file. Invite values never enter tracing, errors or the audit store.
    println!(
        "{}",
        json!({"hub":endpoint.id().to_string(),"address":format!("{:?}",endpoint.addr()),"invites":invites})
    );
    std::io::stdout().flush()?;
    let limit = Arc::new(tokio::sync::Semaphore::new(128));
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _=tokio::signal::ctrl_c()=>break,
            Some(_)=tasks.join_next(), if !tasks.is_empty()=>{},
            incoming=endpoint.accept()=> {
                let Some(incoming)=incoming else {break};
                let Ok(permit)=limit.clone().try_acquire_owned() else {drop(incoming);continue};
                let kernel=kernel.clone(); let local=endpoint.id();
                tasks.spawn(async move {
                    let _permit=permit;
                    if let Ok(Ok(connection))=tokio::time::timeout(Duration::from_secs(10),incoming).await
                        && let Ok(session)=Session::new(connection,local,Role::Hub) {
                            let peer=session.peer();
                            if let Ok(token)=kernel.transport_connected(peer).await {
                                let _=session.serve(Arc::new(kernel.clone())).await;
                                let _=kernel.transport_disconnected(peer,token).await;
                            }
                        }
                });
            }
        }
    }
    endpoint.close().await;
    tasks.shutdown().await;
    Ok(())
}
