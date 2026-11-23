//! Bidirectional operation transport on an authenticated iroh connection.
//! Enrollment and operation authority are supplied by the service; this module
//! never turns transport identity or negotiation into a grant.
use crate::scheduler::{Classification, PeerScheduler};
use crate::wire::{read_frame, write_frame};
use iroh::{EndpointId, endpoint::Connection};
use serde_json::{Value, json};
use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::time::{Instant, timeout, timeout_at};

pub const ALPN: &[u8] = b"coglet/hub/0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Hub,
    Device,
}

impl Role {
    fn permits(self, method: &str) -> bool {
        match method {
            "hub.receipt" | "hub.cancel" => true,
            "hub.enroll" | "hub.events" | "hub.health" => self == Role::Device,
            _ => self == Role::Hub,
        }
    }
    fn opposite(self) -> Self {
        if self == Role::Hub {
            Role::Device
        } else {
            Role::Hub
        }
    }
}

#[derive(Debug, thiserror::Error)]
#[error("hub transport: {0}")]
pub struct TransportError(pub String);
fn error(e: impl std::fmt::Display) -> TransportError {
    TransportError(e.to_string())
}

/// Authority comes only from the iroh handshake. A request cannot supply this ID.
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub peer: EndpointId,
    pub local: EndpointId,
    pub deadline: Instant,
}

/// Returns a schema-valid terminal frame. Enrollment must commit its one-use
/// invite/allowlist/grants transaction before returning success. Implementors own
/// method/context/target authorization and durable side effects/receipts.
pub trait OperationHandler: Send + Sync + 'static {
    fn handle<'a>(
        &'a self,
        context: RequestContext,
        request: Value,
    ) -> Pin<Box<dyn Future<Output = Value> + Send + 'a>>;
}

#[derive(Clone)]
pub struct Session {
    connection: Connection,
    local: EndpointId,
    role: Role,
    selected: Arc<AtomicBool>,
    scheduler: PeerScheduler,
}

/// Iroh defaults to application reset 0 on Drop. Every abandoned operation,
/// including a cancelled future or a handler panic, must instead use code 1.
struct StreamGuard {
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
    completed: bool,
}
impl StreamGuard {
    fn new(send: iroh::endpoint::SendStream, recv: iroh::endpoint::RecvStream) -> Self {
        Self {
            send,
            recv,
            completed: false,
        }
    }
}
impl Drop for StreamGuard {
    fn drop(&mut self) {
        if !self.completed {
            let _ = self.send.reset(1u32.into());
            let _ = self.recv.stop(1u32.into());
        }
    }
}

fn validate(rule: &str, value: &Value) -> Result<(), TransportError> {
    let value = crate::wire::profile(value).map_err(error)?;
    coglet_types::validate(rule, &value).map_err(error)
}

fn failure(request: &Value, code: &str) -> Value {
    json!({"type":"error", "correlation_id":request["correlation_id"],
        "error":{"code":code,"message":code,"retryable":false}})
}

impl Session {
    pub fn new(
        connection: Connection,
        local: EndpointId,
        role: Role,
    ) -> Result<Self, TransportError> {
        if connection.alpn() != ALPN {
            return Err(error("wrong ALPN"));
        }
        connection.set_max_concurrent_bi_streams(16u32.into());
        Ok(Self {
            scheduler: PeerScheduler::shared(local, connection.remote_id()).map_err(error)?,
            connection,
            local,
            role,
            selected: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn peer(&self) -> EndpointId {
        self.connection.remote_id()
    }

    /// Opens a new stream per operation. Run `serve` concurrently to receive
    /// operations initiated by the other endpoint on the same connection.
    pub async fn request(&self, request: Value) -> Result<Value, TransportError> {
        validate("hub-request", &request)?;
        if !self.role.permits(request["method"].as_str().unwrap()) {
            return Err(error("wrong method direction"));
        }
        let duration = Duration::from_millis(request["timeout_ms"].as_u64().unwrap());
        let _permit = self
            .scheduler
            .admit(
                request["method"].as_str().unwrap(),
                serde_json::to_vec(&request).map_err(error)?.len(),
            )
            .map_err(error)?;
        timeout(duration, async {
            let (send, recv) = self.connection.open_bi().await.map_err(error)?;
            let mut streams = StreamGuard::new(send, recv);
            let StreamGuard { send, recv, .. } = &mut streams;
            write_frame(send, &request).await.map_err(error)?;
            send.finish().map_err(error)?;
            let reply = read_frame(recv).await.map_err(error)?;
            self.check_reply(&request, &reply)?;
            let mut extra = [0];
            if tokio::io::AsyncReadExt::read(recv, &mut extra)
                .await
                .map_err(error)?
                != 0
            {
                let _ = recv.stop(1u32.into());
                return Err(error("trailing reply"));
            }
            if request["method"] == "hub.enroll" && reply["type"] == "reply" {
                self.selected.store(true, Ordering::Release);
            }
            streams.completed = true;
            Ok(reply)
        })
        .await
        .map_err(error)?
    }

    fn check_reply(&self, request: &Value, reply: &Value) -> Result<(), TransportError> {
        validate("hub-frame", reply)?;
        if reply["correlation_id"] != request["correlation_id"] {
            return Err(error("correlation mismatch"));
        }
        if reply["type"] == "error" {
            return Ok(());
        }
        if reply["type"] != "reply" {
            return Err(error("expected terminal frame"));
        }
        let rule = match request["method"].as_str().unwrap() {
            "hub.enroll" => "hub-enroll-result",
            "hub.status" => "hub-status",
            "debug.read" => "hub-read-result",
            "hub.events" => "hub-events-result",
            "hub.actions" => "action-accept",
            "debug.step" => "hub-step-result",
            "hub.git.fetch" | "hub.health" => "hub-empty",
            _ => "hub-mutation-receipt",
        };
        validate(rule, &reply["result"])?;
        if request["method"] == "hub.enroll" {
            let (device, hub) = if self.role == Role::Device {
                (self.local, self.peer())
            } else {
                (self.peer(), self.local)
            };
            if reply["result"]["device"] != device.to_string()
                || reply["result"]["hub"] != hub.to_string()
            {
                return Err(error("enrollment identity mismatch"));
            }
            if reply["result"]["selection"]["version"] != "0.1"
                || !request["params"]["offer"]["versions"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("0.1"))
            {
                return Err(error("unoffered selection"));
            }
        }
        Ok(())
    }

    pub async fn serve(self, handler: Arc<dyn OperationHandler>) -> Result<(), TransportError> {
        let mut tasks = tokio::task::JoinSet::new();
        loop {
            // Completed tasks must not accumulate behind a continuously ready
            // accept branch. At most the admitted peer budget remains in the set.
            while tasks.try_join_next().is_some() {}
            tokio::select! {
                next = self.connection.accept_bi() => {
                    let (send, recv) = next.map_err(error)?;
                    let streams = StreamGuard::new(send, recv);
                    let Ok(classification) = self.scheduler.classify() else { continue; };
                    let session = self.clone(); let handler = handler.clone();
                    tasks.spawn(async move { session.respond(streams, classification, handler).await });
                }
                _ = tasks.join_next(), if !tasks.is_empty() => {}
            }
        }
    }

    async fn respond(
        &self,
        mut streams: StreamGuard,
        mut classification: Classification,
        handler: Arc<dyn OperationHandler>,
    ) {
        let StreamGuard { send, recv, .. } = &mut streams;
        let result: Result<(), TransportError> = async {
            let request = timeout(Duration::from_secs(2), read_frame(recv))
                .await
                .map_err(error)?
                .map_err(error)?;
            let received = Instant::now();
            validate("hub-request", &request)?;
            // Charge both retained request and the owned handler copy. Raw frame
            // buffers are separately bounded by the two classification slots.
            let bytes = serde_json::to_vec(&request).map_err(error)?.len() * 2;
            let permit = classification.admit(request["method"].as_str().unwrap(), bytes);
            let _permit = match permit {
                Ok(permit) => permit,
                Err(_) => {
                    let mut reply = failure(&request, "resource_exhausted");
                    reply["error"]["retryable"] = json!(true);
                    timeout(Duration::from_secs(2), write_frame(send, &reply))
                        .await
                        .map_err(error)?
                        .map_err(error)?;
                    send.finish().map_err(error)?;
                    // Preserve the terminal rejection while explicitly stopping
                    // unread input with the protocol error code (Drop uses 0).
                    let _ = recv.stop(1u32.into());
                    return Ok(());
                }
            };
            let deadline =
                received + Duration::from_millis(request["timeout_ms"].as_u64().unwrap());
            timeout_at(deadline, async {
                let mut extra = [0];
                if recv.read(&mut extra).await.map_err(error)?.is_some() {
                    return Err(error("trailing request"));
                }
                let enroll = request["method"] == "hub.enroll";
                let offers = request["params"]["offer"]["versions"].as_array();
                let reply = if !self
                    .role
                    .opposite()
                    .permits(request["method"].as_str().unwrap())
                {
                    failure(&request, "permission_denied")
                } else if !enroll && !self.selected.load(Ordering::Acquire) {
                    failure(&request, "unauthenticated")
                } else if enroll
                    && (!offers.unwrap().contains(&json!("0.1"))
                        || (self.selected.load(Ordering::Acquire)
                            && offers.unwrap().iter().any(|v| v != "0.1")))
                {
                    failure(&request, "unsupported_version")
                } else {
                    handler
                        .handle(
                            RequestContext {
                                peer: self.peer(),
                                local: self.local,
                                deadline,
                            },
                            request.clone(),
                        )
                        .await
                };
                self.check_reply(&request, &reply)?;
                if enroll && reply["type"] == "reply" {
                    self.selected.store(true, Ordering::Release);
                }
                write_frame(send, &reply).await.map_err(error)?;
                send.finish().map_err(error)?;
                Ok(())
            })
            .await
            .map_err(error)?
        }
        .await;
        if result.is_ok() {
            streams.completed = true;
        }
    }
}
