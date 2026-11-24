//! Explicitly opt-in ABI 0.2 client. Replies are provisional until step commit.
//! This module does not enable DB admission in a 0.1 runtime or grant authority.
use crate::{Context, Host, HostError};
use coglet_types::{
    DatabaseDbEnqueue, DatabaseDbEnqueued, DatabaseDbFailed, DatabaseDbPage, DatabaseDbStatement,
    Value,
};
use serde::Serialize;

pub const MAX_MESSAGE: usize = 65_536;
pub const OUTPUT_CAPACITY: usize = MAX_MESSAGE + 4;

/// Synchronous, copied input/output boundary, separate from the frozen 0.1 Host.
pub trait DbHost {
    fn db(&mut self, request: &[u8], output: &mut [u8]) -> i32;
}
#[derive(Debug)]
pub enum DbError {
    Host(HostError),
    Failed(coglet_types::CommonError),
    Protocol(String),
}
type Result<T> = std::result::Result<T, DbError>;
fn protocol(message: impl Into<String>) -> DbError {
    DbError::Protocol(message.into())
}

pub struct Client<'a, H: DbHost> {
    host: &'a mut H,
}
impl<'a, H: DbHost> Client<'a, H> {
    pub fn new(host: &'a mut H) -> Self {
        Self { host }
    }
    fn exchange<T: Serialize>(&mut self, request: &T) -> Result<(Value, usize)> {
        let input =
            coglet_types::encode_typed("database::db-request", request).map_err(protocol)?;
        if input.len() > MAX_MESSAGE {
            return Err(protocol("request too large"));
        }
        let mut output = vec![0u8; OUTPUT_CAPACITY];
        match self.host.db(&input, &mut output) {
            0 => (),
            n => {
                return Err(DbError::Host(match n {
                    1 => HostError::InvalidArgument,
                    2 => HostError::PermissionDenied,
                    3 => HostError::ResourceExhausted,
                    4 => HostError::Conflict,
                    n => HostError::Reserved(n),
                }));
            }
        }
        let len = u32::from_be_bytes(output[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_MESSAGE {
            return Err(protocol("invalid reply length"));
        }
        let value = coglet_types::decode_profile(&output[4..4 + len]).map_err(protocol)?;
        coglet_types::validate("database::db-reply", &value).map_err(protocol)?;
        if field(&value, "status") == Some(&Value::Text("failed".into())) {
            let failed: DatabaseDbFailed =
                value.deserialized().map_err(|e| protocol(e.to_string()))?;
            return Err(DbError::Failed(failed.error));
        }
        Ok((value, len))
    }
    pub fn statement(&mut self, request: &DatabaseDbStatement) -> Result<DatabaseDbPage> {
        for cell in &request.params {
            check_cell(cell)?;
        }
        if let Some(cursor) = &request.cursor {
            check_cursor(cursor)?;
        }
        if request.mode == "write" && request.cursor.is_some() {
            return Err(protocol("write cursor"));
        }
        let (value, len) = self.exchange(request)?;
        let page: DatabaseDbPage = value.deserialized().map_err(|e| protocol(e.to_string()))?;
        if page.status != "page"
            || len > request.max_bytes as usize
            || page.rows.len() > request.max_rows as usize
            || page.columns.iter().any(|c| c.is_empty() || c.len() > 4096)
            || page
                .columns
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != page.columns.len()
            || page.rows.iter().any(|row| row.len() != page.columns.len())
        {
            return Err(protocol("invalid page bounds"));
        }
        for row in &page.rows {
            for cell in row {
                check_cell(cell)?;
            }
        }
        match &page.next_cursor {
            Value::Null => (),
            Value::Text(cursor) if !page.rows.is_empty() => check_cursor(cursor)?,
            _ => return Err(protocol("invalid page cursor")),
        }
        if request.mode == "write" {
            if !page.columns.is_empty() || !page.rows.is_empty() || page.next_cursor != Value::Null
            {
                return Err(protocol("write returned rows"));
            }
        } else if page.affected != 0 {
            return Err(protocol("read affected rows"));
        }
        Ok(page)
    }
    pub fn enqueue(&mut self, request: &DatabaseDbEnqueue) -> Result<DatabaseDbEnqueued> {
        let (value, _) = self.exchange(request)?;
        let reply: DatabaseDbEnqueued =
            value.deserialized().map_err(|e| protocol(e.to_string()))?;
        if reply.status != "enqueued" || (!reply.duplicate && reply.intent_id != request.intent_id)
        {
            return Err(protocol("unexpected enqueue reply"));
        }
        Ok(reply)
    }
}
fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value
        .as_map()?
        .iter()
        .find_map(|(key, value)| (key.as_text() == Some(name)).then_some(value))
}
fn check_cell(value: &Value) -> Result<()> {
    match value {
        Value::Null => Ok(()),
        Value::Integer(i)
            if i128::from(*i) >= i64::MIN as i128 && i128::from(*i) <= u64::MAX as i128 =>
        {
            Ok(())
        }
        Value::Text(s) if s.len() <= 4096 => Ok(()),
        _ => Err(protocol("unrepresentable cell")),
    }
}
fn check_cursor(cursor: &str) -> Result<()> {
    if cursor.is_empty() || cursor.len() > 512 {
        Err(protocol("invalid cursor bytes"))
    } else {
        Ok(())
    }
}
impl<H: Host + DbHost> Context<'_, H> {
    pub fn database(&mut self) -> Client<'_, H> {
        Client::new(self.host)
    }
}
#[cfg(target_arch = "wasm32")]
impl DbHost for crate::__private::Imports {
    fn db(&mut self, request: &[u8], output: &mut [u8]) -> i32 {
        #[link(wasm_import_module = "coglet_v0")]
        unsafe extern "C" {
            fn db(ptr: u32, len: u32, out_ptr: u32, out_cap: u32) -> i32;
        }
        let (Ok(len), Ok(cap)) = (u32::try_from(request.len()), u32::try_from(output.len())) else {
            return 1;
        };
        // SAFETY: disjoint live slice allocations remain valid through this synchronous call.
        unsafe {
            db(
                request.as_ptr() as u32,
                len,
                output.as_mut_ptr() as u32,
                cap,
            )
        }
    }
}
