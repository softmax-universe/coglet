//! Bounded local socket driver. No wire listener or deployment authority lives
//! here: a provisioned kernel service must own admission and durable execution.
use super::{
    PeerIdentity, Request,
    socket::{OperatorSocket, read_request, write_response},
};
use serde_json::Value;
use std::{future::Future, io, pin::Pin, sync::Arc, time::Duration};
use tokio::{net::UnixStream, sync::watch, task::JoinSet, time::Instant};

pub enum Reply {
    /// A common pre-admission error; no operation was admitted.
    Rejected(Value),
    /// Initial value MUST be a committed queued/terminal response. The service
    /// owns execution and recovery independently of this observation channel.
    Admitted(watch::Receiver<Value>),
}
pub trait Handler: Send + Sync + 'static {
    /// Authenticate explicit UID→actor provisioning and local policy before
    /// admission. Never derive an actor from the UID or borrow kernel authority.
    /// Use bounded blocking storage work; publish only committed responses.
    fn admit(
        &self,
        peer: PeerIdentity,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = io::Result<Reply>> + Send + '_>>;
}
async fn connection(
    mut stream: UnixStream,
    peer: PeerIdentity,
    handler: Arc<dyn Handler>,
) -> io::Result<()> {
    let request = read_request(&mut stream).await?;
    let budget = request.value()["timeout_ms"]
        .as_u64()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing wait budget"))?;
    let deadline = Instant::now() + Duration::from_millis(budget);
    // Do not cancel admission at the wait deadline: a transaction may already
    // have committed. The bounded service returns its durable observation first.
    let response = match handler.admit(peer, request).await? {
        Reply::Rejected(error) => error,
        Reply::Admitted(mut updates) => {
            let wait = async {
                loop {
                    if updates
                        .borrow()
                        .get("receipt")
                        .and_then(|v| v.get("state"))
                        .and_then(Value::as_str)
                        != Some("queued")
                    {
                        break;
                    }
                    if updates.changed().await.is_err() {
                        break;
                    }
                }
            };
            let _ = tokio::time::timeout_at(deadline, wait).await;
            updates.borrow().clone()
        }
    };
    write_response(&mut stream, &response).await
}
/// Every accepted client gets at most one parser/waiter. Saturated connections
/// are dropped before JSON allocation/admission. Shutdown drops waiters only;
/// the service's retained operations remain its recovery responsibility.
pub async fn serve(
    socket: OperatorSocket,
    handler: Arc<dyn Handler>,
    capacity: usize,
    shutdown: impl Future<Output = ()> + Send,
) -> io::Result<()> {
    if !(1..=128).contains(&capacity) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid operator capacity",
        ));
    }
    tokio::pin!(shutdown);
    let mut tasks = JoinSet::new();
    let result = loop {
        tokio::select! {
            _ = &mut shutdown => break Ok(()),
            Some(_) = tasks.join_next(), if !tasks.is_empty() => (),
            accepted = socket.accept() => match accepted {
                Ok((stream, peer)) if tasks.len() < capacity => {
                    let handler = handler.clone();
                    tasks.spawn(async move { let _ = connection(stream, peer, handler).await; });
                }
                Ok(_) => (),
                Err(error) if error.kind() == io::ErrorKind::PermissionDenied => (),
                Err(error) => break Err(error),
            }
        }
    };
    tasks.shutdown().await;
    result
}
