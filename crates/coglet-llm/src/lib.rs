//! Outbox-worker LLM gateway. Never call from inside guest `handle`.
//!
//! The authenticated adapter supplies identity/grants and only committed requests.
//! SQLite FULL transactions journal request, reservation, terminal reply and spend.
//! A claimed request is never automatically resent, including after process death.
use coglet_types::{Value as Cbor, encode_typed};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};

pub mod http;
mod policies;

pub type Result<T> = std::result::Result<T, String>;
fn db(e: rusqlite::Error) -> String {
    e.to_string()
}

// Convert explicitly: serde_json's workspace-unified arbitrary_precision feature
// serializes Number as a private map through generic serde-to-CBOR bridges.
fn profile(value: &Value, depth: usize) -> Result<Cbor> {
    if depth > coglet_types::MAX_DEPTH {
        return Err("nesting too deep".into());
    }
    Ok(match value {
        Value::Null => Cbor::Null,
        Value::Bool(b) => Cbor::Bool(*b),
        Value::String(s) => Cbor::Text(s.clone()),
        Value::Number(n) => {
            if let Some(n) = n.as_u64() {
                Cbor::Integer(n.into())
            } else if let Some(n) = n.as_i64() {
                Cbor::Integer(n.into())
            } else {
                return Err("non-profile integer/float".into());
            }
        }
        Value::Array(a) => Cbor::Array(
            a.iter()
                .map(|v| profile(v, depth + 1))
                .collect::<Result<_>>()?,
        ),
        Value::Object(m) => Cbor::Map(
            m.iter()
                .map(|(k, v)| Ok((Cbor::Text(k.clone()), profile(v, depth + 1)?)))
                .collect::<Result<_>>()?,
        ),
    })
}
fn encode_json(rule: &str, value: &Value) -> Result<Vec<u8>> {
    coglet_types::encode(rule, &profile(value, 0)?)
}
fn json_profile(value: Cbor) -> Result<Value> {
    Ok(match value {
        Cbor::Null => Value::Null,
        Cbor::Bool(b) => Value::Bool(b),
        Cbor::Text(s) => Value::String(s),
        Cbor::Integer(n) => {
            let n = i128::from(n);
            if n >= 0 {
                json!(u64::try_from(n).map_err(|e| e.to_string())?)
            } else {
                json!(i64::try_from(n).map_err(|e| e.to_string())?)
            }
        }
        Cbor::Array(a) => Value::Array(a.into_iter().map(json_profile).collect::<Result<_>>()?),
        Cbor::Map(m) => Value::Object(
            m.into_iter()
                .map(|(k, v)| {
                    let Cbor::Text(k) = k else {
                        return Err("non-text map key".into());
                    };
                    Ok((k, json_profile(v)?))
                })
                .collect::<Result<_>>()?,
        ),
        _ => return Err("non-profile CBOR".into()),
    })
}
fn decode_json(rule: &str, raw: &[u8]) -> Result<Value> {
    json_profile(coglet_types::decode(rule, raw)?)
}

/// Trusted route configuration, NOT fields taken from provider input.
#[derive(Clone, Debug)]
pub struct Route {
    pub provider: String,
    /// Integer micro-units of a single deployment-defined currency.
    /// The adapter must enforce this upper bound at the provider boundary.
    pub ceiling: i64,
}

/// Adapter-authenticated context. Construction is the embedding host's trust boundary.
pub struct Admission<'a> {
    pub coglet: &'a str,
    pub llm_complete_granted: bool,
}

#[derive(Clone, Debug)]
pub struct Call {
    pub coglet: String,
    pub request: Value,
    pub provider: String,
    pub ceiling: i64,
}

/// A definitive provider receipt. Timeout/cancellation with known billing is a
/// failed ABI reply; an ambiguous network error must instead return Err.
pub struct Completion {
    pub reply: Value,
    pub cost: i64,
}

pub trait Provider {
    /// Err means uncertain delivery: leave the full reservation held for reconciliation.
    fn complete(&mut self, call: &Call) -> Result<Completion>;
}

#[derive(Debug, PartialEq, Eq)]
pub struct Ledger {
    pub budget: i64,
    pub spent: i64,
    pub reserved: i64,
}

pub struct Gateway {
    conn: Connection,
    routes: BTreeMap<String, Route>,
}

impl Gateway {
    pub fn open(path: impl AsRef<Path>, routes: BTreeMap<String, Route>) -> Result<Self> {
        if routes
            .values()
            .any(|r| r.provider.is_empty() || r.ceiling <= 0)
        {
            return Err("invalid route/ceiling".into());
        }
        let conn = Connection::open(path).map_err(db)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))
            .map_err(db)?;
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
            CREATE TABLE IF NOT EXISTS accounts (
                coglet TEXT PRIMARY KEY, budget INTEGER NOT NULL CHECK(budget>=0),
                spent INTEGER NOT NULL DEFAULT 0 CHECK(spent>=0),
                reserved INTEGER NOT NULL DEFAULT 0 CHECK(reserved>=0));
            CREATE TABLE IF NOT EXISTS requests (
                ordinal INTEGER PRIMARY KEY, coglet TEXT NOT NULL, id TEXT NOT NULL,
                request BLOB NOT NULL, provider TEXT NOT NULL, ceiling INTEGER NOT NULL,
                state TEXT NOT NULL CHECK(state IN ('queued','uncertain','terminal')),
                reply BLOB, cost INTEGER, inbox BLOB, UNIQUE(coglet,id));",
        )
        .map_err(db)?;
        policies::init(&conn)?;
        Ok(Self { conn, routes })
    }

    /// Budget changes are operator policy, never derived from an LLM request.
    pub fn set_budget(&mut self, coglet: &str, budget: i64) -> Result<()> {
        encode_typed("coglet-id", &coglet)?;
        if budget < 0 {
            return Err("negative budget".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let used: Option<(i64, i64)> = tx
            .query_row(
                "SELECT spent,reserved FROM accounts WHERE coglet=?",
                [coglet],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(db)?;
        if let Some((s, r)) = used
            && s.checked_add(r).ok_or("ledger overflow")? > budget
        {
            return Err("budget below committed exposure".into());
        }
        tx.execute("INSERT INTO accounts(coglet,budget) VALUES(?,?) ON CONFLICT(coglet) DO UPDATE SET budget=excluded.budget", params![coglet,budget]).map_err(db)?;
        tx.commit().map_err(db)
    }

    /// Persist one committed outbox request; duplicates conflict even if identical.
    /// Refusals never call a provider. Adapter maps them to recorded failure replies
    /// in its runtime delivery journal; ABI staging is not gateway admission.
    pub fn enqueue(&mut self, auth: Admission<'_>, request: &Value) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "clock unavailable")?
            .as_millis();
        self.enqueue_at(
            auth,
            request,
            u64::try_from(now).map_err(|_| "clock overflow")?,
        )
    }

    /// As enqueue, using a trusted host clock, never a guest-supplied timestamp.
    pub fn enqueue_at(&mut self, auth: Admission<'_>, request: &Value, now_ms: u64) -> Result<()> {
        let now = i64::try_from(now_ms).map_err(|_| "clock overflow")?;
        let raw = encode_json("llm-request", request)?;
        encode_typed("coglet-id", &auth.coglet)?;
        if !auth.llm_complete_granted || request["capability"] != "llm.complete" {
            return Err("permission_denied".into());
        }
        let route = self
            .routes
            .get(request["model"].as_str().ok_or("model")?)
            .ok_or("unsupported route")?;
        let id = request["id"].as_str().ok_or("id")?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let duplicate: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM requests WHERE coglet=? AND id=?)",
                params![auth.coglet, id],
                |r| r.get(0),
            )
            .map_err(db)?;
        if duplicate {
            return Err("conflict".into());
        }
        let (budget, spent, reserved): (i64, i64, i64) = tx
            .query_row(
                "SELECT budget,spent,reserved FROM accounts WHERE coglet=?",
                [auth.coglet],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(db)?;
        let exposure = spent
            .checked_add(reserved)
            .and_then(|x| x.checked_add(route.ceiling))
            .ok_or("ledger overflow")?;
        if exposure > budget {
            return Err("resource_exhausted".into());
        }
        policies::admit(&tx, auth.coglet, id, &route.provider, request, now)?;
        tx.execute(
            "UPDATE accounts SET reserved=reserved+? WHERE coglet=?",
            params![route.ceiling, auth.coglet],
        )
        .map_err(db)?;
        tx.execute("INSERT INTO requests(coglet,id,request,provider,ceiling,state) VALUES(?,?,?,?,?,'queued')", params![auth.coglet,id,raw,route.provider,route.ceiling]).map_err(db)?;
        tx.commit().map_err(db)
    }

    pub fn ledger(&self, coglet: &str) -> Result<Ledger> {
        self.conn
            .query_row(
                "SELECT budget,spent,reserved FROM accounts WHERE coglet=?",
                [coglet],
                |r| {
                    Ok(Ledger {
                        budget: r.get(0)?,
                        spent: r.get(1)?,
                        reserved: r.get(2)?,
                    })
                },
            )
            .map_err(db)
    }

    /// Worker claims in committed insertion order. The claim is durable BEFORE
    /// network I/O. The closure selects the configured provider, not guest input.
    /// The adapter must recheck current grants before calling this method.
    pub fn dispatch_next(
        &mut self,
        select: impl FnMut(&Call) -> Result<Completion>,
    ) -> Result<bool> {
        self.dispatch_next_checked(|_| Ok(()), select)
    }

    /// Recheck current host authorization for both cache hits and real dispatch.
    /// The caller must serialize revocation with its dispatch boundary. Failure
    /// leaves a conservative uncertain claim; it never calls the provider.
    pub fn dispatch_next_checked(
        &mut self,
        mut authorize: impl FnMut(&Call) -> Result<()>,
        mut select: impl FnMut(&Call) -> Result<Completion>,
    ) -> Result<bool> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let row: Option<(String,Vec<u8>,String,i64)> = tx.query_row("SELECT coglet,request,provider,ceiling FROM requests WHERE state='queued' ORDER BY ordinal LIMIT 1", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional().map_err(db)?;
        let Some((coglet, raw, provider, ceiling)) = row else {
            return Ok(false);
        };
        let request: Value = decode_json("llm-request", &raw)?;
        let id = request["id"].as_str().ok_or("id")?.to_owned();
        tx.execute(
            "UPDATE requests SET state='uncertain' WHERE coglet=? AND id=?",
            params![coglet, id],
        )
        .map_err(db)?;
        tx.commit().map_err(db)?;
        let call = Call {
            coglet: coglet.clone(),
            request,
            provider,
            ceiling,
        };
        authorize(&call)?;
        let completion = if let Some(completion) = policies::cached(&self.conn, &coglet, &id)? {
            completion
        } else {
            select(&call)?
        };
        self.reconcile(&coglet, &id, completion)?;
        Ok(true)
    }

    /// Administrative/provider receipt reconciliation. No retry or invented result.
    /// Terminal reply and debit are one immutable transaction. Repeated/conflicting
    /// receipts fail closed, so they cannot double-charge or replace journal bytes.
    pub fn reconcile(&mut self, coglet: &str, id: &str, completion: Completion) -> Result<()> {
        let raw = encode_json("llm-reply", &completion.reply)?;
        if completion.reply["request_id"] != id {
            return Err("request_id mismatch".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let (ceiling, state): (i64, String) = tx
            .query_row(
                "SELECT ceiling,state FROM requests WHERE coglet=? AND id=?",
                params![coglet, id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .map_err(db)?;
        if state != "uncertain" {
            return Err("not awaiting receipt".into());
        }
        if completion.cost < 0 || completion.cost > ceiling {
            return Err("cost outside reservation; reconciliation required".into());
        }
        policies::save_cache(&tx, coglet, id, &completion, &raw)?;
        tx.execute(
            "UPDATE accounts SET reserved=reserved-?,spent=spent+? WHERE coglet=?",
            params![ceiling, completion.cost, coglet],
        )
        .map_err(db)?;
        tx.execute(
            "UPDATE requests SET state='terminal',reply=?,cost=? WHERE coglet=? AND id=?",
            params![raw, completion.cost, coglet, id],
        )
        .map_err(db)?;
        tx.commit().map_err(db)
    }

    /// Offline journal lookup has no provider argument and cannot dispatch.
    pub fn replay(&self, coglet: &str, id: &str) -> Result<Vec<u8>> {
        let raw: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT reply FROM requests WHERE coglet=? AND id=?",
                params![coglet, id],
                |r| r.get(0),
            )
            .map_err(db)?;
        raw.ok_or_else(|| "missing recording".into())
    }

    /// Record exact canonical inbox bytes allocated by the runtime/hub adapter.
    /// Call only after that adapter assigns authenticated sender, index and seq.
    /// The external delivery uses this same immutable line as its retry payload.
    pub fn record_inbox(&mut self, coglet: &str, id: &str, line: &[u8]) -> Result<()> {
        let msg: Value = serde_json::from_slice(line).map_err(|e| e.to_string())?;
        encode_json("inbox-message", &msg)?;
        let mut canonical = serde_json::to_vec(&msg).map_err(|e| e.to_string())?;
        canonical.push(b'\n');
        if canonical != line {
            return Err("noncanonical inbox JSONL".into());
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(db)?;
        let (request, reply, inbox): (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>) = tx
            .query_row(
                "SELECT request,reply,inbox FROM requests WHERE coglet=? AND id=?",
                params![coglet, id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .map_err(db)?;
        let req: Value = decode_json("llm-request", &request)?;
        let reply: Value = decode_json("llm-reply", &reply.ok_or("missing recording")?)?;
        if msg["event"]["kind"] != "llm.reply"
            || msg["event"]["payload"] != reply
            || msg["event"]["correlation_id"] != req["correlation_id"]
            || msg["sender"] != msg["event"]["source"]
        {
            return Err("inbox binding mismatch".into());
        }
        if let Some(old) = inbox {
            return if old == line {
                Ok(())
            } else {
                Err("inbox conflict".into())
            };
        }
        tx.execute(
            "UPDATE requests SET inbox=? WHERE coglet=? AND id=?",
            params![line, coglet, id],
        )
        .map_err(db)?;
        tx.commit().map_err(db)
    }

    pub fn replay_inbox(&self, coglet: &str, id: &str) -> Result<Vec<u8>> {
        let line: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT inbox FROM requests WHERE coglet=? AND id=?",
                params![coglet, id],
                |r| r.get(0),
            )
            .map_err(db)?;
        line.ok_or_else(|| "missing inbox recording".into())
    }
}

/// Helper for definitive provider failures (including timeout/cancellation).
pub fn failure(id: &str, code: &str, message: &str, cost: i64) -> Completion {
    Completion {
        reply: json!({"request_id":id,"status":"failed","error":{"code":code,"message":message,"retryable":false}}),
        cost,
    }
}
