//! Test-only hub over real iroh. All wire policy belongs to the shared Kernel.
//!
//! Fixed identity and clock are fixture defaults, not deployment credentials.
//! Enrollment secrets remain OS-random, one-use and bound to a supplied device.
use coglet_hub::Kernel;
use coglet_proto::{
    authority::Grant,
    transport::{ALPN, Role, Session},
};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, endpoint::presets};
use std::{collections::HashMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::task::{JoinHandle, JoinSet};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
type Sessions = Arc<Mutex<HashMap<EndpointId, (u64, Session, iroh::endpoint::Connection)>>>;

/// Owns a local-only endpoint, shared authority/kernel and bounded serving tasks.
/// Call `shutdown` for an orderly close. Dropping aborts the accept task and its
/// children; the endpoint is not leaked into an independently running server.
pub struct MockHub {
    endpoint: Endpoint,
    kernel: Kernel,
    serving: JoinHandle<()>,
    sessions: Sessions,
    _state: tempfile::TempDir,
}

impl MockHub {
    /// Fresh isolated state, closed enrollment window, fixed test identity/clock.
    pub async fn start() -> Result<Self> {
        Self::with_clock(
            SecretKey::from_bytes(&[2; 32]),
            Arc::new(|| Some(1_800_000_000)),
        )
        .await
    }

    /// Inject identity and trusted Unix clock (e.g. an atomic fixture clock).
    pub async fn with_clock(
        key: SecretKey,
        clock: Arc<dyn Fn() -> Option<u64> + Send + Sync>,
    ) -> Result<Self> {
        let state = tempfile::tempdir()?;
        let endpoint = Endpoint::builder(presets::Minimal)
            .secret_key(key)
            .alpns(vec![ALPN.to_vec()])
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>()?)?
            .bind()
            .await?;
        let root = state.path().to_owned();
        let id = endpoint.id();
        let kernel = tokio::task::spawn_blocking(move || Kernel::open(root, id, clock)).await??;
        let server = endpoint.clone();
        let handler = kernel.clone();
        let sessions = Sessions::default();
        let active = sessions.clone();
        let serving = tokio::spawn(async move {
            let mut tasks = JoinSet::new();
            loop {
                tokio::select! {
                    Some(_) = tasks.join_next(), if !tasks.is_empty() => {},
                    incoming = server.accept() => {
                        let Some(incoming) = incoming else { break };
                        if tasks.len() >= 128 { drop(incoming); continue; }
                        let kernel = handler.clone();
                        let active = active.clone();
                        let local = server.id();
                        tasks.spawn(async move {
                            if let Ok(Ok(connection)) = tokio::time::timeout(Duration::from_secs(10), incoming).await
                                && let Ok(session) = Session::new(connection.clone(), local, Role::Hub) {
                                let peer = session.peer();
                                if let Ok(token) = kernel.transport_connected(peer).await {
                                    active.lock().await.insert(peer, (token, session.clone(), connection));
                                    let _ = session.serve(Arc::new(kernel.clone())).await;
                                    let mut sessions = active.lock().await;
                                    if sessions.get(&peer).is_some_and(|(current, _, _)| *current == token) {
                                        sessions.remove(&peer);
                                    }
                                    drop(sessions);
                                    let _ = kernel.transport_disconnected(peer, token).await;
                                }
                            }
                        });
                    }
                }
            }
            while tasks.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            kernel,
            serving,
            sessions,
            _state: state,
        })
    }

    pub fn addr(&self) -> EndpointAddr {
        self.endpoint.addr()
    }
    pub fn id(&self) -> EndpointId {
        self.endpoint.id()
    }
    /// Local test/operator access; never exposed as an ALPN method.
    pub fn kernel(&self) -> &Kernel {
        &self.kernel
    }
    /// Trusted local access to the current hub side of a device-opened session.
    /// Useful for reverse-direction session conformance; not a wire method.
    pub async fn session(&self, device: EndpointId) -> Option<Session> {
        self.sessions
            .lock()
            .await
            .get(&device)
            .map(|(_, s, _)| s.clone())
    }
    /// Trusted local fault injection: close the HUB side, preserving enrollment.
    pub async fn disconnect_device(&self, device: EndpointId) -> bool {
        if let Some((_, _, connection)) = self.sessions.lock().await.get(&device) {
            connection.close(0u32.into(), b"mock hub fault injection");
            true
        } else {
            false
        }
    }
    pub fn state_path(&self) -> &Path {
        self._state.path()
    }

    /// Open an explicit enrollment window and issue a peer-bound one-use secret.
    /// Treat the returned value as private bootstrap output, never as a log.
    pub async fn invite(
        &self,
        device: EndpointId,
        ttl: Duration,
        grants: Vec<Grant>,
    ) -> Result<String> {
        self.kernel.open_window(ttl).await?;
        Ok(self.kernel.invite(device, ttl, grants).await?)
    }

    pub async fn shutdown(mut self) {
        self.endpoint.close().await;
        // Borrowing retains the handle for Drop's abort on a teardown timeout.
        let _ = tokio::time::timeout(Duration::from_secs(12), &mut self.serving).await;
    }
}

impl Drop for MockHub {
    fn drop(&mut self) {
        self.serving.abort();
    }
}
