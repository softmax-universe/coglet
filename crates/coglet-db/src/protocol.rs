//! Buffer checks, reservations and typed replies for the future ABI 0.2 host.
//! Runtime/engine admission remains closed; callers supply the aggregate budget.
use super::*;
use coglet_types::Value as V;
// Result production plus copy into guest memory, including the length prefix.
const OUTPUT_RESERVATION: u64 = 2 * 65536 + 4;

pub struct HostBudget {
    pub remaining: u64,
}
#[derive(Debug, PartialEq, Eq)]
pub enum CallResult {
    /// ABI invalid_argument abort. No argument copy, SQL or output occurred.
    InvalidPointer,
    Return {
        result: u32,
        response: Option<Vec<u8>>,
    },
}
fn field<'a>(v: &'a V, key: &str) -> Option<&'a V> {
    v.as_map()?
        .iter()
        .find_map(|(k, v)| (k.as_text() == Some(key)).then_some(v))
}
fn text<'a>(v: &'a V, key: &str) -> Result<&'a str> {
    field(v, key)
        .and_then(V::as_text)
        .ok_or("invalid request".into())
}
fn uint(v: &V, key: &str) -> Result<u64> {
    match field(v, key) {
        Some(V::Integer(n)) => u64::try_from(*n).map_err(|_| "invalid request".into()),
        _ => Err("invalid request".into()),
    }
}
fn failed(error: &str) -> Result<Vec<u8>> {
    let code = match error {
        "cursor conflict"
        | "paging conflict"
        | "optimistic version conflict"
        | "idempotency conflict"
        | "intent ID conflict"
        | "poisoned unit" => "conflict",
        "resource exhausted" | "deadline exceeded" | "revision overflow" | "sqlite interrupted" => {
            "resource_exhausted"
        }
        "permission denied" => "permission_denied",
        "invalid bounds"
        | "invalid request"
        | "invalid cell"
        | "invalid result columns"
        | "unknown statement"
        | "statement binding mismatch"
        | "parameter type mismatch"
        | "catalog SQL binding mismatch"
        | "result column binding mismatch"
        | "result type mismatch"
        | "unrepresentable cell"
        | "sqlite constraint" => "invalid_argument",
        _ => "unavailable",
    };
    let t = |s: &str| V::Text(s.into());
    // Never expose SQLite messages, paths or contents to a guest.
    coglet_types::encode_typed(
        "database::db-failed",
        &V::Map(vec![
            (t("status"), t("failed")),
            (
                t("error"),
                V::Map(vec![
                    (t("code"), t(code)),
                    (t("message"), t(code)),
                    (t("retryable"), V::Bool(false)),
                ]),
            ),
        ]),
    )
}
impl Unit<'_> {
    fn reject(&mut self, result: u32) -> CallResult {
        self.calls = self.calls.saturating_add(1);
        self.poisoned = true;
        self.rollback_failed_unit();
        CallResult::Return {
            result,
            response: None,
        }
    }
    /// Validate widened unsigned wasm32 ranges, disjointness and capacity before
    /// copying. Reserve input + maximum framed reply before SQL. Unused reserved
    /// output is released after materialization; actual copies remain charged.
    pub fn call(
        &mut self,
        memory: &mut [u8],
        ptr: i32,
        len: i32,
        out_ptr: i32,
        out_cap: i32,
        budget: &mut HostBudget,
    ) -> CallResult {
        let (p, n, o, c) = (
            ptr as u32 as u64,
            len as u32 as u64,
            out_ptr as u32 as u64,
            out_cap as u32 as u64,
        );
        if p + n > memory.len() as u64 || o + c > memory.len() as u64 || (p < o + c && o < p + n) {
            self.calls = self.calls.saturating_add(1);
            self.poisoned = true;
            self.rollback_failed_unit();
            return CallResult::InvalidPointer;
        }
        if n > 65536 || c < 65540 {
            return self.reject(1);
        }
        // Charge the input copy even if profile/shape/grant validation rejects it.
        if budget.remaining < n {
            return self.reject(3);
        }
        budget.remaining -= n;
        let input = memory[p as usize..(p + n) as usize].to_vec();
        let value = match coglet_types::decode_profile(&input).and_then(|v| {
            coglet_types::validate("database::db-request", &v)?;
            Ok(v)
        }) {
            Ok(v) => v,
            Err(_) => return self.reject(1),
        };
        let op = text(&value, "operation").unwrap_or("");
        let mode = if text(&value, "mode") == Ok("read") {
            Mode::Read
        } else {
            Mode::Write
        };
        if op == "statement"
            && (field(&value, "params")
                .and_then(V::as_array)
                .is_none_or(|p| p.iter().any(|c| matches!(c,V::Text(s) if s.len()>4096)))
                || field(&value, "cursor")
                    .and_then(V::as_text)
                    .is_some_and(|s| s.len() > 512 || mode != Mode::Read))
        {
            return self.reject(1);
        }
        if (mode == Mode::Read && !self.grants.read)
            || (mode == Mode::Write && !self.grants.write)
            || (op == "enqueue"
                && self
                    .intent_grants
                    .as_ref()
                    .is_none_or(|k| !k.contains(text(&value, "kind").unwrap_or(""))))
        {
            return self.reject(2);
        }
        if self.poisoned {
            return self.reject(4);
        }
        if budget.remaining < OUTPUT_RESERVATION {
            return self.reject(3);
        }
        budget.remaining -= OUTPUT_RESERVATION;
        // Cursor/paging identity precedes fresh call/statement admission. Output
        // reservation is still mandatory even for a typed rejection.
        let preflight = self.preflight(&value);
        if preflight.is_ok() && (self.calls >= 32 || Instant::now() >= self.deadline) {
            budget.remaining += OUTPUT_RESERVATION;
            return self.reject(3);
        }
        let calls_before = self.calls;
        let response = preflight.and_then(|()| self.dispatch(&value)).or_else(|e| {
            self.poisoned = true;
            self.rollback_failed_unit();
            failed(&e)
        });
        if self.calls == calls_before {
            self.calls = self.calls.saturating_add(1);
        }
        let expired = !self.poisoned && Instant::now() >= self.deadline;
        let discarded_bytes = if expired {
            response.as_ref().map_or(0, |r| r.len() as u64)
        } else {
            0
        };
        let response = if expired {
            self.poisoned = true;
            self.rollback_failed_unit();
            failed("resource exhausted")
        } else {
            response
        };
        let response = match response {
            Ok(r) => r,
            Err(_) => {
                budget.remaining += OUTPUT_RESERVATION;
                return self.reject(3);
            }
        };
        if response.len() > 65536 {
            budget.remaining += OUTPUT_RESERVATION;
            return self.reject(3);
        }
        budget.remaining += OUTPUT_RESERVATION - (discarded_bytes + 2 * response.len() as u64 + 4);
        let start = o as usize;
        memory[start..start + 4].copy_from_slice(&(response.len() as u32).to_be_bytes());
        memory[start + 4..start + 4 + response.len()].copy_from_slice(&response);
        CallResult::Return {
            result: 0,
            response: Some(response),
        }
    }
    fn dispatch(&mut self, v: &V) -> Result<Vec<u8>> {
        if text(v, "operation")? == "enqueue" {
            let reply = self.enqueue(&Intent {
                intent_id: text(v, "intent_id")?.into(),
                idempotency_key: text(v, "idempotency_key")?.into(),
                kind: text(v, "kind")?.into(),
                payload: field(v, "payload").ok_or("invalid request")?.clone(),
                correlation_id: text(v, "correlation_id")?.into(),
            })?;
            let t = |s: &str| V::Text(s.into());
            return coglet_types::encode_typed(
                "database::db-enqueued",
                &V::Map(vec![
                    (t("status"), t("enqueued")),
                    (t("intent_id"), t(&reply.intent_id)),
                    (t("duplicate"), V::Bool(reply.duplicate)),
                ]),
            );
        }
        let id = text(v, "statement")?;
        let params = self.parameters(id, v)?;
        let page = self.statement_page(
            id,
            if text(v, "mode")? == "read" {
                Mode::Read
            } else {
                Mode::Write
            },
            &params,
            uint(v, "max_rows")? as usize,
            uint(v, "max_bytes")? as usize,
            field(v, "cursor").and_then(V::as_text),
        )?;
        page.encode(self.revision)
    }
    fn preflight(&self, v: &V) -> Result<()> {
        let op = text(v, "operation")?;
        if self.paging && (op == "enqueue" || text(v, "mode")? == "write") {
            return Err("paging conflict".into());
        }
        if op == "enqueue" {
            self.enqueue_identity(&Intent {
                intent_id: text(v, "intent_id")?.into(),
                idempotency_key: text(v, "idempotency_key")?.into(),
                kind: text(v, "kind")?.into(),
                payload: field(v, "payload").ok_or("invalid request")?.clone(),
                correlation_id: text(v, "correlation_id")?.into(),
            })?;
        }
        if let Some(cursor) = field(v, "cursor").and_then(V::as_text) {
            if self.wrote {
                return Err("paging conflict".into());
            }
            let id = text(v, "statement")?;
            self.cursor_position(id, &self.parameters(id, v)?, Some(cursor))?;
        }
        Ok(())
    }
    fn parameters(&self, id: &str, v: &V) -> Result<Vec<Cell>> {
        let kinds = self
            .catalog
            .statements
            .get(id)
            .and_then(|s| s.binding.as_ref())
            .map(|b| b.parameters.as_slice());
        field(v, "params")
            .and_then(V::as_array)
            .ok_or("invalid request")?
            .iter()
            .enumerate()
            .map(|(i, c)| {
                Ok(match c {
                    V::Null => Cell::Null,
                    V::Text(s) => Cell::Text(s.clone()),
                    V::Integer(n)
                        if kinds.and_then(|k| k.get(i)).is_some_and(|k| {
                            matches!(k, CellType::Unsigned | CellType::NullableUnsigned)
                        }) =>
                    {
                        Cell::Unsigned(u64::try_from(*n).map_err(|_| "parameter type mismatch")?)
                    }
                    V::Integer(n) => {
                        Cell::Integer(i64::try_from(*n).map_err(|_| "unrepresentable cell")?)
                    }
                    _ => return Err("invalid cell".into()),
                })
            })
            .collect::<Result<Vec<_>>>()
    }
}
