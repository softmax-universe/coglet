//! Trusted-local administration only; never registered as a hub wire method.
#[cfg(unix)]
pub mod server;
#[cfg(unix)]
pub mod service;
#[cfg(unix)]
pub mod socket;
use crate::{Error, Result, Store, audit};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Populated by OS peer credentials, never from a request's JSON.
#[derive(Clone, Copy, Debug)]
pub struct PeerIdentity {
    pub uid: u32,
    pub gid: Option<u32>,
    pub pid: Option<u32>,
}

/// Validated local intent. Timeout is a wait budget, not part of identity.
#[derive(Clone)]
pub struct Request(Value);
impl Request {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        if bytes.is_empty() || bytes.len() > 1_048_576 {
            return Err(Error::Invalid);
        }
        let mut decoder = serde_json::Deserializer::from_slice(bytes);
        let value = <StrictValue as serde::Deserialize>::deserialize(&mut decoder)?.0;
        decoder.end()?;
        closed(
            &value,
            &["version", "operation_id", "op", "timeout_ms", "params"],
        )?;
        if value["version"] != "local-1"
            || !valid("correlation-id", &value["operation_id"])
            || !value["timeout_ms"]
                .as_u64()
                .is_some_and(|n| (1..=300000).contains(&n))
        {
            return Err(Error::Invalid);
        }
        validate_params(
            value["op"].as_str().ok_or(Error::Invalid)?,
            &value["params"],
        )?;
        Ok(Self(value))
    }
    pub fn value(&self) -> &Value {
        &self.0
    }
    fn intent(&self) -> Result<Vec<u8>> {
        let mut intent = self.0.clone();
        intent
            .as_object_mut()
            .ok_or(Error::Invalid)?
            .remove("timeout_ms");
        coglet_proto::wire::encode(&intent).map_err(|_| Error::Invalid)
    }
}

fn validate_params(op: &str, p: &Value) -> Result<()> {
    let fields: &[&str] = match op {
        "deploy" | "push" => &[
            "device",
            "target",
            "source_commit",
            "build_profile",
            "delegation_id",
        ],
        "stop" | "status" => &["device", "target"],
        "grant.provision" => &["peer", "method", "target", "generation", "expires_unix_s"],
        "grant.revoke" => &["peer", "method", "target", "generation"],
        "enrollment.window" => &["duration_ms"],
        "enrollment.invite" => &[
            "window_id",
            "expected_device",
            "secret_sha256",
            "ttl_ms",
            "grants",
        ],
        _ => return Err(Error::Invalid),
    };
    closed(p, fields)?;
    for field in fields {
        let ok = match *field {
            "device" | "peer" | "expected_device" => valid("node-id", &p[field]),
            "target" if op.starts_with("grant.") => valid("hub-target", &p[field]),
            "target" => valid("coglet-id", &p[field]),
            "source_commit" => valid("hub-oid", &p[field]),
            "window_id" => valid("correlation-id", &p[field]),
            "secret_sha256" => valid("hub-hash", &p[field]),
            "method" => valid("hub-method", &p[field]),
            "generation" | "expires_unix_s" => p[field].as_u64().is_some(),
            "duration_ms" => p[field].as_u64().is_some_and(|n| n <= 300000),
            "ttl_ms" => p[field].as_u64().is_some_and(|n| (1..=300000).contains(&n)),
            "build_profile" | "delegation_id" => p[field].as_str().is_some_and(|s| {
                !s.is_empty()
                    && s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            }),
            "grants" => {
                let grants = p[field].as_array().ok_or(Error::Invalid)?;
                if grants.len() > 64 {
                    return Err(Error::Invalid);
                }
                for (i, grant) in grants.iter().enumerate() {
                    validate_params("grant.provision", grant)?;
                    if grant["peer"] != p["expected_device"] || grants[..i].contains(grant) {
                        return Err(Error::Invalid);
                    }
                }
                true
            }
            _ => false,
        };
        if !ok {
            return Err(Error::Invalid);
        }
    }
    Ok(())
}

fn valid(rule: &str, value: &Value) -> bool {
    coglet_proto::wire::encode(value).is_ok_and(|bytes| {
        coglet_types::decode_profile(&bytes).is_ok_and(|v| coglet_types::validate(rule, &v).is_ok())
    })
}
fn closed(value: &Value, keys: &[&str]) -> Result<()> {
    if value
        .as_object()
        .is_some_and(|v| v.len() == keys.len() && keys.iter().all(|k| v.contains_key(*k)))
    {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
// serde_json::Value by itself accepts duplicate keys. Deserialize every nested
// object through this visitor before performing contract validation.
struct StrictValue(Value);
impl<'de> serde::Deserialize<'de> for StrictValue {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = StrictValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(Value::Null))
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_u64<E: serde::de::Error>(self, v: u64) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_i64<E: serde::de::Error>(self, v: i64) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_f64<E: serde::de::Error>(self, v: f64) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<StrictValue, E> {
                Ok(StrictValue(json!(v)))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<StrictValue, A::Error> {
                let mut items = Vec::new();
                while let Some(v) = a.next_element::<StrictValue>()? {
                    items.push(v.0);
                }
                Ok(StrictValue(Value::Array(items)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<StrictValue, A::Error> {
                let mut object = serde_json::Map::new();
                while let Some(key) = a.next_key::<String>()? {
                    if object.contains_key(&key) {
                        return Err(serde::de::Error::custom("duplicate object key"));
                    }
                    object.insert(key, a.next_value::<StrictValue>()?.0);
                }
                Ok(StrictValue(Value::Object(object)))
            }
        }
        d.deserialize_any(Visitor)
    }
}

pub(crate) fn initialize(db: &rusqlite::Connection) -> Result<()> {
    db.execute_batch("CREATE TABLE IF NOT EXISTS operator_operations(
        uid INTEGER NOT NULL, id TEXT NOT NULL, intent BLOB NOT NULL,
        digest TEXT NOT NULL, response TEXT NOT NULL, PRIMARY KEY(uid,id));
        CREATE TABLE IF NOT EXISTS operator_sequence(singleton INTEGER PRIMARY KEY CHECK(singleton=1), value TEXT NOT NULL);
        INSERT OR IGNORE INTO operator_sequence VALUES(1,'0');")?;
    Ok(())
}
impl Store {
    /// Commit a terminal observation and an optional local configuration effect
    /// together. The callback may write ONLY through this transaction: never
    /// perform I/O or mutate the separately stored shared Authority here.
    pub fn operator_finish<F>(
        &mut self,
        uid: u32,
        request: &Request,
        error: Option<Value>,
        effect: F,
    ) -> Result<Value>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<Value>,
    {
        self.operator_terminal(
            uid,
            request,
            if error.is_some() { "failed" } else { "applied" },
            error,
            effect,
        )
    }
    pub(crate) fn operator_terminal<F>(
        &mut self,
        uid: u32,
        request: &Request,
        state: &str,
        error: Option<Value>,
        effect: F,
    ) -> Result<Value>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> Result<Value>,
    {
        if !matches!(state, "applied" | "failed" | "cancelled")
            || (state == "failed") != error.is_some()
        {
            return Err(Error::Invalid);
        }
        let id = request.0["operation_id"].as_str().ok_or(Error::Invalid)?;
        let intent = request.intent()?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (old, response): (Vec<u8>, String) = tx.query_row(
            "SELECT intent,response FROM operator_operations WHERE uid=?1 AND id=?2",
            params![uid, id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        if old != intent {
            return Err(Error::Conflict(0));
        }
        let mut response: Value = serde_json::from_str(&response)?;
        if response["receipt"]["state"] != "queued" {
            return Ok(response);
        }
        if error.as_ref().is_some_and(|e| !valid("error", e)) {
            return Err(Error::Invalid);
        }
        let step = if state == "applied" {
            let old: String = tx.query_row(
                "SELECT value FROM operator_sequence WHERE singleton=1",
                [],
                |r| r.get(0),
            )?;
            let next = crate::number(old)?.checked_add(1).ok_or(Error::Full)?;
            tx.execute(
                "UPDATE operator_sequence SET value=?1 WHERE singleton=1",
                [next.to_string()],
            )?;
            Some(next)
        } else {
            None
        };
        let result = effect(&tx)?;
        let result_valid = if state != "applied" {
            result.is_null()
        } else {
            match request.0["op"].as_str() {
                Some("status") => valid("hub-status", &result),
                Some("enrollment.window" | "enrollment.invite") => {
                    result == request.0["operation_id"]
                }
                _ => result.is_null(),
            }
        };
        if !result_valid {
            return Err(Error::Invalid);
        }
        response["receipt"] = json!({"operation_id":id,"state":state,"step":step,"error":error});
        response["result"] = result;
        audit(
            &tx,
            json!({"method":"operator.finish","uid":uid,"operation_id":id,"op":request.0["op"],"state":response["receipt"]["state"],"step":step}),
        )?;
        tx.execute(
            "UPDATE operator_operations SET response=?1 WHERE uid=?2 AND id=?3",
            params![serde_json::to_string(&response)?, uid, id],
        )?;
        tx.commit()?;
        Ok(response)
    }

    /// Retain admission and OS-authenticated audit in the same FULL/WAL commit.
    /// This low-level method is not an authorization API: the socket driver must
    /// authenticate the peer and check its explicit local actor mapping first.
    pub fn operator_admit(&mut self, peer: PeerIdentity, request: &Request) -> Result<Value> {
        self.operator_admit_actor(peer, request, None)
    }
    pub(crate) fn operator_admit_actor(
        &mut self,
        peer: PeerIdentity,
        request: &Request,
        actor: Option<&str>,
    ) -> Result<Value> {
        let intent = request.intent()?;
        let id = request.0["operation_id"].as_str().ok_or(Error::Invalid)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some((old, response)) = tx
            .query_row(
                "SELECT intent,response FROM operator_operations WHERE uid=?1 AND id=?2",
                params![peer.uid, id],
                |r| Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?
        {
            if let Some(actor) = actor {
                let saved: Option<String>=tx.query_row("SELECT json_extract(progress,'$.actor') FROM operator_sagas WHERE uid=?1 AND id=?2",params![peer.uid,id],|r|r.get(0)).optional()?;
                if saved.as_deref() != Some(actor) {
                    return Err(Error::Denied);
                }
            }
            return if old == intent {
                Ok(serde_json::from_str(&response)?)
            } else {
                Err(Error::Conflict(0))
            };
        }
        // Permanent records are never evicted. Reserve enough admin sequence
        // space for every queued operation before admitting another effect.
        let (count, bytes, pending): (u32, u32, u32) = tx.query_row(
            "SELECT count(*),coalesce(sum(length(intent)+length(response)),0),coalesce(sum(CASE WHEN json_extract(response,'$.receipt.state')='queued' THEN 1 ELSE 0 END),0) FROM operator_operations",
            [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
        let sequence: String = tx.query_row(
            "SELECT value FROM operator_sequence WHERE singleton=1",
            [],
            |r| r.get(0),
        )?;
        if count >= 4096
            || u64::from(bytes)
                .saturating_add(intent.len() as u64)
                .saturating_add(1024)
                > 64 * 1024 * 1024
            || crate::number(sequence)?
                .checked_add(u64::from(pending))
                .and_then(|v| v.checked_add(1))
                .is_none()
        {
            return Err(Error::Full);
        }
        let digest = hex::encode(Sha256::digest(&intent));
        let response = json!({"version":"local-1","operation_id":id,"op":request.0["op"],
            "receipt":{"operation_id":id,"state":"queued","step":null,"error":null},
            "result":null,"error":null});
        tx.execute(
            "INSERT INTO operator_operations VALUES(?1,?2,?3,?4,?5)",
            params![
                peer.uid,
                id,
                intent,
                digest,
                serde_json::to_string(&response)?
            ],
        )?;
        if let Some(actor) = actor {
            tx.execute(
                "INSERT INTO operator_sagas VALUES(?1,?2,?3)",
                params![
                    peer.uid,
                    id,
                    serde_json::to_string(
                        &json!({"actor":actor,"children":[],"outcomes":[],"attempted":[]})
                    )?
                ],
            )?;
        }
        audit(
            &tx,
            json!({"method":"operator.admit","uid":peer.uid,"gid":peer.gid,"pid":peer.pid,
            "operation_id":id,"op":request.0["op"],"intent_sha256":digest}),
        )?;
        tx.commit()?;
        Ok(response)
    }
}
