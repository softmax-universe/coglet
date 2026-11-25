//! Buffer checks, reservations and typed replies for the future ABI 0.2 host.
//! Runtime/engine admission remains closed; callers supply the aggregate budget.
use super::*;
use coglet_engine::{GuestMemory, StepBudget};
use coglet_types::Value as V;

// A copy-only view. Engine faults are retained verbatim; no native guest pointer
// or entire linear-memory image escapes the import.
struct Memory<'a> {
    guest: &'a mut dyn GuestMemory,
    copied: u64,
    fault: Option<coglet_engine::Error>,
}
impl Memory<'_> {
    fn read(&mut self, ptr: u32, len: u32) -> Option<Vec<u8>> {
        match self.guest.read(ptr, len) {
            Ok(bytes) => {
                self.copied += bytes.len() as u64;
                Some(bytes)
            }
            Err(error) => {
                self.fault = Some(error);
                None
            }
        }
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
        match self.guest.write(ptr, bytes) {
            Ok(()) => {
                self.copied += bytes.len() as u64;
                true
            }
            Err(error) => {
                self.fault = Some(error);
                false
            }
        }
    }
}
struct SliceMemory<'a>(&'a mut [u8]);
impl GuestMemory for SliceMemory<'_> {
    fn memory_len(&self) -> u64 {
        self.0.len() as u64
    }
    fn read(&self, ptr: u32, len: u32) -> coglet_engine::Result<Vec<u8>> {
        Ok(self.0[coglet_engine::guest_range(ptr, len, self.memory_len())?].to_vec())
    }
    fn write(&mut self, ptr: u32, bytes: &[u8]) -> coglet_engine::Result<()> {
        let range = coglet_engine::guest_range(ptr, bytes.len() as u32, self.memory_len())?;
        self.0[range].copy_from_slice(bytes);
        Ok(())
    }
}
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
        self.call_memory(
            &mut Memory {
                guest: &mut SliceMemory(memory),
                copied: 0,
                fault: None,
            },
            [ptr, len, out_ptr, out_cap],
            budget,
        )
    }
    /// Copy-only engine bridge. Reserve the maximum reply against the shared
    /// aggregate allowance before SQL; engines settle memory copies after the
    /// callback, so charge only additional result production here. Returning a
    /// reply never commits: admission and durable completion remain explicit.
    pub fn call_engine(
        &mut self,
        guest: &mut dyn GuestMemory,
        args: [i32; 4],
        budget: &mut StepBudget,
    ) -> coglet_engine::Result<CallResult> {
        if let Err(error) = budget.check_deadline() {
            self.reject(3);
            return Err(error);
        }
        let remaining = budget.host_bytes_remaining();
        let mut allowance = HostBudget { remaining };
        let mut memory = Memory {
            guest,
            copied: 0,
            fault: None,
        };
        let result = self.call_memory(&mut memory, args, &mut allowance);
        let production = remaining - allowance.remaining - memory.copied;
        if let Err(error) = budget.charge_host_bytes(production) {
            self.poisoned = true;
            self.rollback_failed_unit();
            return Err(error);
        }
        if let Some(error) = memory.fault {
            return Err(error);
        }
        Ok(result)
    }
    fn call_memory(
        &mut self,
        memory: &mut Memory<'_>,
        args: [i32; 4],
        budget: &mut HostBudget,
    ) -> CallResult {
        let [ptr, len, out_ptr, out_cap] = args;
        let (p, n, o, c) = (
            ptr as u32 as u64,
            len as u32 as u64,
            out_ptr as u32 as u64,
            out_cap as u32 as u64,
        );
        if coglet_engine::disjoint_guest_ranges(
            p as u32,
            n as u32,
            o as u32,
            c as u32,
            memory.guest.memory_len(),
        )
        .is_err()
        {
            self.calls = self.calls.saturating_add(1);
            self.poisoned = true;
            self.rollback_failed_unit();
            return CallResult::InvalidPointer;
        }
        if n > 65536 {
            return self.reject(1);
        }
        // Charge the input copy even if profile/shape/grant validation rejects it.
        if budget.remaining < n {
            return self.reject(3);
        }
        let Some(input) = memory.read(p as u32, n as u32) else {
            return self.reject(3);
        };
        budget.remaining -= n;
        // Preserve the bounded copied argument for rejected-call journaling.
        // Capacity still rejects before any SQL or output reservation.
        if c < 65540 {
            return self.reject(1);
        }
        let value = match coglet_types::decode_profile(&input).and_then(|v| {
            coglet_types::validate(self.profile.rule("database::db-request"), &v)?;
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
        budget.remaining += OUTPUT_RESERVATION - (discarded_bytes + response.len() as u64);
        if !memory.write(o as u32, &(response.len() as u32).to_be_bytes()) {
            self.poisoned = true;
            self.rollback_failed_unit();
            return CallResult::Return {
                result: 3,
                response: None,
            };
        }
        budget.remaining -= 4;
        if !memory.write((o + 4) as u32, &response) {
            self.poisoned = true;
            self.rollback_failed_unit();
            return CallResult::Return {
                result: 3,
                response: None,
            };
        }
        budget.remaining -= response.len() as u64;
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
