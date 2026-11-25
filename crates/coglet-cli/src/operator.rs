//! ADR-0018 trusted-local client, not a new router wire method.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{path::Path, time::Duration};

const MAX_FRAME: usize = 1_048_576;

/// All caller-selected identity is explicit so a lost response can be retried
/// with the SAME operation ID and intent. Never automatically allocate a retry.
pub fn request(op: &str, id: &str, timeout_ms: u64, params: Value) -> Result<Value> {
    ensure!(
        ["deploy", "push", "status", "stop"].contains(&op),
        "unsupported operator operation"
    );
    super::checked("correlation-id", &json!(id))?;
    ensure!(
        (1..=300000).contains(&timeout_ms),
        "timeout-ms must be 1..300000"
    );
    super::checked("node-id", &params["device"])?;
    super::checked("coglet-id", &params["target"])?;
    let fields: &[&str] = if op == "deploy" || op == "push" {
        super::checked("hub-oid", &params["source_commit"])?;
        for name in ["build_profile", "delegation_id"] {
            ensure!(
                params[name].as_str().is_some_and(|s| !s.is_empty()
                    && s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')),
                "invalid provisioning reference {name}"
            );
        }
        &[
            "device",
            "target",
            "source_commit",
            "build_profile",
            "delegation_id",
        ]
    } else {
        &["device", "target"]
    };
    closed(&params, fields)?;
    Ok(
        json!({"version":"local-1","operation_id":id,"op":op,"timeout_ms":timeout_ms,"params":params}),
    )
}

fn closed(value: &Value, fields: &[&str]) -> Result<()> {
    ensure!(
        value
            .as_object()
            .is_some_and(|m| m.len() == fields.len() && fields.iter().all(|k| m.contains_key(*k))),
        "invalid closed operator object"
    );
    Ok(())
}

/// Validate all framing/shape/identity before interpreting an observation.
fn response(bytes: &[u8], request: &Value) -> Result<Value> {
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let value = Strict::deserialize(&mut decoder)?.0;
    decoder.end()?;
    closed(
        &value,
        &[
            "version",
            "operation_id",
            "op",
            "receipt",
            "result",
            "error",
        ],
    )?;
    for name in ["version", "operation_id", "op"] {
        ensure!(
            value[name] == request[name],
            "operator response {name} mismatch"
        );
    }
    if value["receipt"].is_null() {
        super::checked("error", &value["error"])?;
        ensure!(value["result"].is_null(), "rejected response has result");
    } else {
        ensure!(value["error"].is_null(), "receipt response has outer error");
        super::checked("hub-mutation-receipt", &value["receipt"])?;
        ensure!(
            value["receipt"]["operation_id"] == request["operation_id"],
            "receipt operation ID mismatch"
        );
        if value["receipt"]["state"] == "applied" && request["op"] == "status" {
            super::checked("hub-status", &value["result"])?;
        } else {
            ensure!(value["result"].is_null(), "unexpected operator result");
        }
    }
    Ok(value)
}

/// Exit success is reserved for a confirmed complete operation. Queued is a
/// durable admission, NOT deployment success; failed/cancelled are also nonzero.
pub fn applied(value: &Value) -> bool {
    value["receipt"]["state"] == "applied"
}

#[cfg(unix)]
pub async fn call(socket: &Path, request: &Value) -> Result<Value> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::UnixStream,
    };
    // Only the owning service identity can use this interface (no root exception).
    // SAFETY: geteuid has no arguments or pointers.
    let uid = unsafe { libc::geteuid() };
    let root = socket
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let dir = std::fs::symlink_metadata(root)?;
    ensure!(
        dir.is_dir() && dir.uid() == uid && dir.mode() & 0o777 == 0o700,
        "operator state directory must be service-owned mode 0700, not a symlink"
    );
    let meta = std::fs::symlink_metadata(socket)?;
    ensure!(
        meta.file_type().is_socket() && meta.uid() == uid && meta.mode() & 0o777 == 0o600,
        "operator socket must be service-owned mode 0600, not a symlink"
    );
    let bytes = serde_json::to_vec(request)?;
    ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_FRAME,
        "operator request exceeds frame bound"
    );
    let timeout = request["timeout_ms"].as_u64().context("operator timeout")?;
    let exchange = async {
        let mut stream = UnixStream::connect(socket).await?;
        ensure!(
            stream.peer_cred()?.uid() == uid,
            "operator server UID mismatch"
        );
        let current = std::fs::symlink_metadata(socket)?;
        ensure!(
            current.dev() == meta.dev() && current.ino() == meta.ino(),
            "operator socket changed during connect"
        );
        stream.write_u32(bytes.len() as u32).await?;
        stream.write_all(&bytes).await?;
        let len = stream.read_u32().await? as usize;
        ensure!(
            (1..=MAX_FRAME).contains(&len),
            "invalid operator response frame length"
        );
        let mut bytes = vec![0; len];
        stream.read_exact(&mut bytes).await?;
        response(&bytes, request)
    };
    tokio::time::timeout(Duration::from_millis(timeout + 1000), exchange).await
        .context("operator response deadline; outcome unknown: retry identical intent and operation ID")?
        .context("operator exchange failed; if admitted, outcome unknown: retry identical intent and operation ID")
}

#[cfg(not(unix))]
pub async fn call(_socket: &Path, _request: &Value) -> Result<Value> {
    anyhow::bail!(
        "operator control requires a local Unix socket; run coglet on the router host, directly or over SSH"
    )
}

// Reject duplicate keys at EVERY depth, not merely in the envelope. JSON Value
// alone keeps only the last key, which could turn an ambiguous receipt to success.
struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = Strict;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique keys")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(self, v: bool) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_str<E: serde::de::Error>(self, v: &str) -> std::result::Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut v = Vec::new();
                while let Some(x) = a.next_element::<Strict>()? {
                    v.push(x.0);
                }
                Ok(Strict(Value::Array(v)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Strict, A::Error> {
                let mut v = serde_json::Map::new();
                while let Some(k) = a.next_key::<String>()? {
                    if v.contains_key(&k) {
                        return Err(serde::de::Error::custom("duplicate operator JSON key"));
                    }
                    v.insert(k, a.next_value::<Strict>()?.0);
                }
                Ok(Strict(Value::Object(v)))
            }
        }
        d.deserialize_any(Visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn operator_response_rejects_ambiguous_or_unbound_receipts() {
        let req = json!({"version":"local-1","operation_id":"11".repeat(16),"op":"deploy"});
        let value = json!({"version":"local-1","operation_id":req["operation_id"],"op":"deploy","receipt":{"operation_id":req["operation_id"],"state":"applied","step":1,"error":null},"result":null,"error":null});
        assert!(applied(
            &response(&serde_json::to_vec(&value).unwrap(), &req).unwrap()
        ));
        for field in ["version", "operation_id", "op"] {
            let mut bad = value.clone();
            bad[field] = json!("wrong");
            assert!(response(&serde_json::to_vec(&bad).unwrap(), &req).is_err());
        }
        let raw = serde_json::to_string(&value).unwrap().replace(
            "\"state\":\"applied\"",
            "\"state\":\"failed\",\"state\":\"applied\"",
        );
        assert!(response(raw.as_bytes(), &req).is_err());
        let mut bad = value.clone();
        bad["receipt"]["extra"] = json!(true);
        assert!(response(&serde_json::to_vec(&bad).unwrap(), &req).is_err());
        let mut bad = value.clone();
        bad["receipt"]["operation_id"] = json!("22".repeat(16));
        assert!(response(&serde_json::to_vec(&bad).unwrap(), &req).is_err());
        assert!(response(format!("{value} null").as_bytes(), &req).is_err());
    }
}
