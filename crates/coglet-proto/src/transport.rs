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

    /// Git dispatch retains the peer's Git permit and reset guard through this
    /// entire future, including validation and durable commit. Authorize current
    /// grants and CAS BEFORE issuing credit, and recheck in the commit transaction.
    /// `receive_push` fences the input FIN before returning raw quarantine custody.
    /// A successful push requires that helper; merely returning a receipt cannot
    /// accept an unread transfer. Returning an error stops unread input with 1.
    fn handle_git<'a>(
        &'a self,
        _context: RequestContext,
        request: Value,
        _stream: GitStream<'a>,
    ) -> Pin<Box<dyn Future<Output = Result<Value, TransportError>> + Send + 'a>> {
        // Never invoke a legacy unary mutation handler before receiving its
        // transfer. A post-handler success gate cannot undo those side effects.
        Box::pin(async move { Ok(failure(&request, "permission_denied")) })
    }
}

/// Restricted responder-side streaming phase. Raw iroh streams never escape the
/// Session guard. A received file is NOT a verified Git artifact or an authority
/// grant: envelope validation, bounded Git validation and transactional acceptance
/// must still happen inside `handle_git`, before returning a success receipt.
pub struct GitStream<'a> {
    send: &'a mut iroh::endpoint::SendStream,
    recv: &'a mut iroh::endpoint::RecvStream,
    request: &'a Value,
    peer: EndpointId,
    input_finished: &'a mut bool,
    started: bool,
}
impl GitStream<'_> {
    /// Receive exactly the signed push's declared bytes into fsync-backed private
    /// storage, then require end + FIN. Caller must have authorized this request
    /// and prechecked latest CAS before calling: the first credit is emitted here.
    /// Cancellation discards all custody; no partial resume is permitted.
    pub async fn receive_push(
        &mut self,
        metadata: &crate::git_metadata::VerifiedRef,
        root: &std::path::Path,
    ) -> Result<crate::quarantine::BundleSink, TransportError> {
        if self.started || self.request["method"] != "hub.git.push" {
            return Err(error("not an unused push stream"));
        }
        self.started = true;
        if metadata.signed() != &self.request["params"]["update"]
            || metadata.signed()["statement"]["coglet"] != self.request["target"]
            || metadata.signed()["statement"]["hub"] != self.peer.to_string()
            || metadata.signed()["statement"]["ref"] != "refs/heads/hub"
        {
            return Err(error("push metadata pins mismatch"));
        }
        let params = &self.request["params"];
        let size = params["size"]
            .as_u64()
            .ok_or_else(|| error("missing size"))?;
        let mut sink = crate::quarantine::BundleSink::create(root, size)
            .await
            .map_err(error)?;
        crate::transfer::receive_chunks(
            self.send,
            self.recv,
            self.request["correlation_id"].as_str().unwrap(),
            size,
            params["sha256"].as_str().unwrap(),
            &mut sink,
        )
        .await
        .map_err(error)?;
        if self.recv.read(&mut [0]).await.map_err(error)?.is_some() {
            return Err(error("trailing push data"));
        }
        *self.input_finished = true;
        Ok(sink)
    }

    /// Send an authorized snapshot. Caller obtains current metadata and retained
    /// bundle bytes from trusted storage, not a request-supplied path. No source
    /// bytes are read before credit. Require initiator FIN after all credit and
    /// end, before allowing a terminal success. This is transport, not permission
    /// to disclose logs/code or proof that the supplied source is valid Git.
    pub async fn send_fetch<B: tokio::io::AsyncRead + Unpin>(
        &mut self,
        metadata: &crate::git_metadata::VerifiedRef,
        size: u64,
        hash: &str,
        source: &mut B,
    ) -> Result<(), TransportError> {
        if self.started || self.request["method"] != "hub.git.fetch" {
            return Err(error("not an unused fetch stream"));
        }
        self.started = true;
        if metadata.signed()["statement"]["coglet"] != self.request["target"]
            || metadata.signed()["statement"]["hub"] != self.peer.to_string()
            || metadata.signed()["statement"]["ref"] != self.request["params"]["ref"]
        {
            return Err(error("fetch metadata pins mismatch"));
        }
        let id = self.request["correlation_id"].as_str().unwrap();
        let transfer = json!({"type":"transfer", "correlation_id":id,
            "size":size,"sha256":hash,"update":metadata.signed()});
        validate("hub-transfer", &transfer)?;
        write_frame(self.send, &transfer).await.map_err(error)?;
        crate::transfer::send_chunks(self.send, self.recv, id, size, source)
            .await
            .map_err(error)?;
        if self.recv.read(&mut [0]).await.map_err(error)?.is_some() {
            return Err(error("trailing fetch credit"));
        }
        *self.input_finished = true;
        Ok(())
    }
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
                let push = request["method"] == "hub.git.push";
                let git = push || request["method"] == "hub.git.fetch";
                let mut input_finished = false;
                if !git {
                    if recv.read(&mut [0]).await.map_err(error)?.is_some() {
                        return Err(error("trailing request"));
                    }
                    input_finished = true;
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
                } else if git {
                    handler
                        .handle_git(
                            RequestContext {
                                peer: self.peer(),
                                local: self.local,
                                deadline,
                            },
                            request.clone(),
                            GitStream {
                                send,
                                recv,
                                request: &request,
                                peer: self.peer(),
                                input_finished: &mut input_finished,
                                started: false,
                            },
                        )
                        .await?
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
                if !input_finished {
                    if reply["type"] != "error" {
                        return Err(error("Git success before verified input FIN"));
                    }
                    let _ = recv.stop(1u32.into());
                }
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
