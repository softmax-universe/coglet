//! Hermes Board service transport. See PHASE2.md for pinned source provenance.
//! No private SQLite access and no guessed task REST endpoints.
use crate::adapter::{
    AdapterError, HermesAdapter, Lease, Outcome, RemoteTask, TaskSpec, WorkerReport,
};
use reqwest::{Method, blocking::Client, redirect::Policy};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub struct HttpKanban {
    base: String,
    board: String,
    bearer: String,
    gateway: String,
    assignee: String,
    client: Client,
}

impl HttpKanban {
    /// Unconfigured connections fail closed. Production must supply enrolled
    /// gateway credentials and a board explicitly through `authenticated`.
    pub fn new(base: impl Into<String>) -> Self {
        Self::authenticated(base, "", "", "", "").expect("HTTP client")
    }

    pub fn authenticated(
        base: impl Into<String>,
        board: &str,
        bearer: &str,
        gateway: &str,
        assignee: &str,
    ) -> Result<Self, AdapterError> {
        let base = base.into();
        let url = reqwest::Url::parse(&base).map_err(|_| failed("invalid Board URL"))?;
        // Never send gateway credentials over cleartext except loopback fixtures.
        if url.scheme() != "https"
            && !(url.scheme() == "http"
                && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")))
        {
            return Err(failed("Board requires HTTPS (loopback fixtures excepted)"));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(failed(
                "Board URL may not contain credentials, query or fragment",
            ));
        }
        if !board
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
        {
            return Err(failed("invalid board identifier"));
        }
        // Pin this client's provider rather than changing process-global TLS
        // configuration. Ring supports the SDK-less Apple cross-check; the
        // platform verifier still supplies native trust and hostname checks.
        use rustls_platform_verifier::BuilderVerifierExt;
        let tls = rustls::ClientConfig::builder_with_provider(std::sync::Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(|_| failed("TLS protocol initialization"))?
        .with_platform_verifier()
        .map_err(|_| failed("TLS platform verifier initialization"))?
        .with_no_client_auth();
        let client = Client::builder()
            .tls_backend_preconfigured(tls)
            .timeout(Duration::from_secs(10))
            .redirect(Policy::none())
            .retry(reqwest::retry::never())
            .build()
            .map_err(|_| failed("HTTP client initialization"))?;
        Ok(Self {
            base: base.trim_end_matches('/').into(),
            board: board.into(),
            bearer: bearer.into(),
            gateway: gateway.into(),
            assignee: assignee.into(),
            client,
        })
    }

    fn call(
        &self,
        method: Method,
        suffix: &str,
        body: Option<Value>,
    ) -> Result<Value, AdapterError> {
        if self.board.is_empty()
            || self.bearer.is_empty()
            || self.gateway.is_empty()
            || self.assignee.is_empty()
        {
            return Err(AdapterError::Conflict(
                "Board, enrolled gateway bearer, gateway and assignee are required".into(),
            ));
        }
        let mut request = self
            .client
            .request(
                method,
                format!("{}/v2/boards/{}{}", self.base, self.board, suffix),
            )
            .bearer_auth(&self.bearer)
            .header("user-agent", "hermes-kanban-sync/1");
        if let Some(body) = body {
            request = request.json(&body);
        }
        // Never retry a POST automatically; the caller reconciles durable identities.
        let response = request.send().map_err(|_| AdapterError::Uncertain)?;
        let status = response.status();
        if status.is_server_error() {
            return Err(AdapterError::Uncertain);
        }
        if !status.is_success() {
            // Auth, admission, rate limiting and protocol refusal are not
            // evidence that a worker executed and failed the requested task.
            return Err(AdapterError::Uncertain);
        }
        // Bound external result size, including chunked bodies.
        use std::io::Read;
        let mut bytes = Vec::new();
        response
            .take(1_048_577)
            .read_to_end(&mut bytes)
            .map_err(|_| AdapterError::Uncertain)?;
        if bytes.len() > 1_048_576 {
            return Err(AdapterError::Uncertain);
        }
        serde_json::from_slice(&bytes).map_err(|_| AdapterError::Uncertain)
    }

    fn op(&self, body: Value) -> Result<Value, AdapterError> {
        let id = body["client_op_id"]
            .as_str()
            .ok_or_else(|| failed("operation identity missing"))?
            .to_owned();
        let response = self.call(Method::POST, "/ops", Some(json!({"ops":[body]})))?;
        let results = response["results"]
            .as_array()
            .ok_or(AdapterError::Uncertain)?;
        if results.len() != 1 || results[0]["client_op_id"] != id {
            return Err(AdapterError::Uncertain);
        }
        verdict(&results[0])?;
        Ok(results[0].clone())
    }

    fn evidence(&self, table: &str, uid: &str) -> Result<Option<Value>, AdapterError> {
        identifier(uid)?;
        let response = self.call(
            Method::GET,
            &format!("/evidence?table={table}&uid={uid}"),
            None,
        )?;
        if response["board"] != self.board || response["table"] != table || response["uid"] != uid {
            return Err(AdapterError::Uncertain);
        }
        match response["state"].as_str() {
            Some("live") if response["row"]["uid"] == uid => Ok(Some(response["row"].clone())),
            Some("never_existed") => Ok(None),
            _ => Err(AdapterError::Uncertain),
        }
    }

    fn run(remote: &str, attempt: u64) -> Result<String, AdapterError> {
        identifier(remote)?;
        if attempt == 0 {
            return Err(failed("zero attempt"));
        }
        Ok(format!("factory:{remote}:{attempt}"))
    }

    fn lease(
        &self,
        action: &str,
        remote: &str,
        attempt: u64,
        until: u64,
    ) -> Result<Value, AdapterError> {
        let run = Self::run(remote, attempt)?;
        let response = self.call(Method::POST, "/leases", Some(json!({
            "action":action, "task":remote,"run_uid":run,"ttl_s":60,"lane":"ready",
            "client_op_id":format!("factory:{action}:{remote}:{attempt}:{}", if action == "renew" { until } else { 0 })
        })))?;
        verdict(&response)?;
        let lease = &response["lease"];
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| failed("clock unavailable"))?
            .as_millis();
        if lease["task_id"] != remote
            || lease["run_uid"] != run
            || lease["holder"] != self.gateway
            || !lease["expires"]
                .as_u64()
                .is_some_and(|expires| u128::from(expires) > now)
        {
            return Err(AdapterError::Uncertain);
        }
        Ok(lease.clone())
    }
}

impl HermesAdapter for HttpKanban {
    fn supports_retry(&self) -> bool {
        false
    }
    fn create(&mut self, spec: &TaskSpec) -> Result<RemoteTask, AdapterError> {
        if spec.idempotency_key.len() != 32
            || !spec
                .idempotency_key
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        {
            return Err(failed("Factory idempotency key must be 32 lowercase hex"));
        }
        let digest = format!("{:x}", Sha256::digest(spec.idempotency_key.as_bytes()));
        let remote = format!("t_{}", &digest[..16]);
        let body = json!({"kind":spec.kind,"payload_cbor_hex":spec.payload_cbor_hex,
            "correlation_id":spec.correlation_id,"task_id":spec.task_id})
        .to_string();
        // Exact row lookup before create handles response loss beyond the service's
        // bounded applied-op cache. Never treat a reused/archived identity as fresh.
        let existing = self.evidence("tasks", &remote)?;
        let row = match existing {
            Some(row) => row,
            None => {
                self.op(json!({"op":"create","task":remote,
                    "client_op_id":format!("factory:create:{}",spec.idempotency_key),
                    "fields":{"title":format!("Factory {} {}",spec.kind,spec.task_id),"body":body,
                    "assignee":self.assignee,"created_by":"coglet-factory",
                    "idempotency_key":spec.idempotency_key,"workspace_kind":"scratch","review_required":0}}))?;
                self.evidence("tasks", &remote)?
                    .ok_or(AdapterError::Uncertain)?
            }
        };
        if row["idempotency_key"] != spec.idempotency_key
            || row["body"] != body
            || row["assignee"] != self.assignee
            || row["status"] == "archived"
        {
            return Err(AdapterError::Conflict(
                "remote identity/payload mismatch or archived".into(),
            ));
        }
        Ok(RemoteTask {
            remote_id: remote,
            idempotency_key: spec.idempotency_key.clone(),
        })
    }

    fn claim(
        &mut self,
        remote: &str,
        owner: &str,
        until: u64,
        attempt: u64,
    ) -> Result<Lease, AdapterError> {
        let lease = self.lease("grant", remote, attempt, until)?;
        let effective_until = local_deadline(&lease, until)?;
        Ok(Lease {
            remote_id: remote.into(),
            owner: owner.into(),
            until: effective_until,
            attempt,
            token: lease["run_uid"]
                .as_str()
                .ok_or(AdapterError::Uncertain)?
                .into(),
        })
    }

    fn restore_lease(
        &mut self,
        remote: &str,
        owner: &str,
        until: u64,
        attempt: u64,
        token: &str,
    ) -> Result<Lease, AdapterError> {
        if Self::run(remote, attempt)? != token {
            return Err(AdapterError::Conflict("stale run".into()));
        }
        let lease = self.lease("renew", remote, attempt, until)?;
        Ok(Lease {
            remote_id: remote.into(),
            owner: owner.into(),
            until: local_deadline(&lease, until)?,
            attempt,
            token: token.into(),
        })
    }
    fn heartbeat(
        &mut self,
        remote: &str,
        until: u64,
        attempt: u64,
        token: &str,
    ) -> Result<u64, AdapterError> {
        if Self::run(remote, attempt)? != token {
            return Err(AdapterError::Conflict("stale run".into()));
        }
        let lease = self.lease("renew", remote, attempt, until)?;
        local_deadline(&lease, until)
    }

    fn retry(&mut self, _remote: &str) -> Result<(), AdapterError> {
        Err(failed(
            "unsupported: Board has no public retry operation; release/unblock are not equivalent",
        ))
    }
    fn cancel(&mut self, _remote: &str, _attempt: u64, _token: &str) -> Result<(), AdapterError> {
        Err(failed(
            "unsupported: Board has no public cancel operation; archive/delete are not cancellation",
        ))
    }

    fn progress(
        &mut self,
        remote: &str,
        note: &str,
        attempt: u64,
        token: &str,
    ) -> Result<(), AdapterError> {
        if Self::run(remote, attempt)? != token {
            return Err(AdapterError::Conflict("stale run".into()));
        }
        // Comment itself is not lease-fenced in Hermes; independently verify the
        // live current run. No claim that this read and comment are atomic.
        let row = self
            .evidence("tasks", remote)?
            .ok_or_else(|| failed("missing task"))?;
        if row["current_run_uid"] != token
            || row["lease_holder"] != self.gateway
            || row["status"] != "running"
        {
            return Err(AdapterError::Conflict("not current holder".into()));
        }
        let hash = format!("{:x}", Sha256::digest(note.as_bytes()));
        self.op(
            json!({"op":"comment","task":remote,"run_uid":token,"author":"coglet-factory",
            "body":note,"uid":format!("factory:progress:{remote}:{attempt}:{hash}"),
            "client_op_id":format!("factory:progress:{remote}:{attempt}:{hash}")}),
        )?;
        Ok(())
    }

    fn poll(
        &mut self,
        remote: &str,
        attempt: u64,
        token: &str,
    ) -> Result<Option<WorkerReport>, AdapterError> {
        if Self::run(remote, attempt)? != token {
            return Err(AdapterError::Conflict("stale run".into()));
        }
        let Some(run) = self.evidence("task_runs", token)? else {
            return Ok(None);
        };
        if run["task_id"] != remote {
            return Err(AdapterError::Conflict("wrong task run".into()));
        }
        if run["ended_at"].is_null() {
            return Ok(None);
        }
        let note = run["summary"]
            .as_str()
            .or_else(|| run["error"].as_str())
            .unwrap_or("")
            .to_owned();
        let outcome = match (run["status"].as_str(), run["outcome"].as_str()) {
            (Some("done"), Some("completed")) => Outcome::Succeeded { note },
            (Some("crashed" | "failed"), Some("crashed" | "failed")) => Outcome::Failed { note },
            _ => return Err(failed("unsupported terminal run outcome")),
        };
        Ok(Some(WorkerReport {
            attempt,
            lease: token.into(),
            outcome,
        }))
    }
}
fn verdict(value: &Value) -> Result<(), AdapterError> {
    match value["status"].as_str() {
        Some("applied" | "merged") => Ok(()),
        Some("rejected" | "deferred") => Err(AdapterError::Conflict(
            value["reason"].as_str().unwrap_or("Board refused").into(),
        )),
        _ => Err(AdapterError::Uncertain),
    }
}
fn identifier(id: &str) -> Result<(), AdapterError> {
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"._:-".contains(&c))
    {
        return Err(failed("invalid wire identifier"));
    }
    Ok(())
}
fn local_deadline(lease: &Value, until: u64) -> Result<u64, AdapterError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| failed("clock unavailable"))?
        .as_millis();
    let expires = u128::from(lease["expires"].as_u64().ok_or(AdapterError::Uncertain)?);
    let remaining = expires.saturating_sub(now).min(60_000) as u64;
    if remaining == 0 {
        return Err(AdapterError::Conflict("expired lease".into()));
    }
    Ok(until.saturating_sub(60_000).saturating_add(remaining))
}
fn failed(note: &str) -> AdapterError {
    AdapterError::Failed(note.into())
}
