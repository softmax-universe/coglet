//! Production trusted-local provisioning. No remote or implicit actor authority.
use super::{
    PeerIdentity, Request,
    server::{Handler, Reply},
};
use crate::{Error, Kernel, Result};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{future::Future, io, pin::Pin};

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GrantPolicy {
    pub peer: String,
    pub method: String,
    pub target: Option<String>,
    pub max_expires_unix_s: u64,
}
/// Trusted startup configuration, never populated from socket input. Authority
/// is non-ticket only; no wildcard operations, methods, targets or peers.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provisioning {
    pub uid: u32,
    pub actor: String,
    pub operations: Vec<String>,
    pub max_window_ms: u64,
    pub grants: Vec<GrantPolicy>,
}
impl Provisioning {
    pub fn validate(&self, service_uid: u32) -> Result<()> {
        if self.uid != service_uid
            || !super::valid("source-id", &json!(self.actor))
            || self.max_window_ms > 300000
            || self.operations.len() > 9
            || self.grants.len() > 256
            || self.operations.iter().enumerate().any(|(i, op)| {
                !matches!(
                    op.as_str(),
                    "grant.provision"
                        | "grant.revoke"
                        | "enrollment.window"
                        | "enrollment.invite"
                        | "deploy"
                        | "push"
                        | "stop"
                        | "status"
                ) || self.operations[..i].contains(op)
            })
            || self.grants.iter().any(|g| {
                g.peer.parse::<iroh::EndpointId>().is_err()
                    || !super::valid("hub-method", &json!(g.method))
                    || !super::valid("hub-target", &json!(g.target))
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
    pub(crate) fn permits(&self, peer: PeerIdentity, request: &Request) -> bool {
        let r = request.value();
        if peer.uid != self.uid || !self.operations.iter().any(|op| r["op"] == *op) {
            return false;
        }
        let p = &r["params"];
        match r["op"].as_str() {
            Some("grant.provision" | "grant.revoke") => self.permits_grant(p),
            Some("enrollment.window") => p["duration_ms"]
                .as_u64()
                .is_some_and(|n| n <= self.max_window_ms),
            Some("enrollment.invite") => {
                self.grants.iter().any(|g| p["expected_device"] == g.peer)
                    && p["grants"]
                        .as_array()
                        .is_some_and(|gs| gs.iter().all(|g| self.permits_grant(g)))
            }
            _ => false, // A qualified saga owns downstream dispatch, never this configuration handler.
        }
    }
    fn permits_grant(&self, p: &Value) -> bool {
        self.grants.iter().any(|g| {
            p["peer"] == g.peer
                && p["method"] == g.method
                && p["target"] == json!(g.target)
                && p.get("expires_unix_s")
                    .is_none_or(|v| v.as_u64().is_some_and(|n| n <= g.max_expires_unix_s))
        })
    }
}
pub struct Service {
    kernel: Kernel,
    provisioning: Option<Provisioning>,
}
impl Service {
    pub fn new(kernel: Kernel, service_uid: u32, provisioning: Provisioning) -> Result<Self> {
        provisioning.validate(service_uid)?;
        Ok(Self {
            kernel,
            provisioning: Some(provisioning),
        })
    }
    /// Listen but refuse admission until explicit local mapping is supplied.
    pub fn unprovisioned(kernel: Kernel) -> Self {
        Self {
            kernel,
            provisioning: None,
        }
    }
}
pub(crate) fn common_error(error: &Error) -> Value {
    use coglet_proto::authority::AuthorityError;
    let code = match error {
        Error::Invalid | Error::Authority(AuthorityError::InvalidArgument) => "invalid_argument",
        Error::Conflict(_) => "conflict",
        Error::Full | Error::Authority(AuthorityError::ResourceExhausted) => "resource_exhausted",
        Error::Authority(AuthorityError::Unauthenticated) => "unauthenticated",
        _ => "permission_denied",
    };
    json!({"code":code,"message":code,"retryable":false,"details":{}})
}
impl Handler for Service {
    fn admit(
        &self,
        peer: PeerIdentity,
        request: Request,
    ) -> Pin<Box<dyn Future<Output = io::Result<Reply>> + Send + '_>> {
        Box::pin(async move {
            let provisioning = self
                .provisioning
                .as_ref()
                .filter(|p| p.permits(peer, &request));
            let Some(provisioning) = provisioning else {
                return Ok(Reply::Rejected(
                    json!({"version":"local-1","operation_id":request.value()["operation_id"],"op":request.value()["op"],"receipt":null,"result":null,"error":common_error(&Error::Denied)}),
                ));
            };
            let id = request.value()["operation_id"].clone();
            let op = request.value()["op"].clone();
            match self
                .kernel
                .operator_configuration(peer, request, provisioning.clone())
                .await
            {
                Ok(response) => Ok(Reply::Admitted(tokio::sync::watch::channel(response).1)),
                Err(error) => Ok(Reply::Rejected(
                    json!({"version":"local-1","operation_id":id,"op":op,"receipt":null,"result":null,"error":common_error(&error)}),
                )),
            }
        })
    }
}
