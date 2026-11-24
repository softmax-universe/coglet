//! Factory repository; every method's result is provisional until host step commit.
//! There is deliberately no guest commit, DDL, SQL-text or migration capability.
pub mod catalog;
use coglet_guest_rust::database::{Client, DbError, DbHost};
use coglet_types::{DatabaseDbPage, DatabaseDbStatement, Value};
pub const SCHEMA_SQL: &str = include_str!("../migrations/0001.sql");

/// Minimal walking-skeleton read probe. No executive, LLM, scheduling or dispatch.
/// Other events have no behavior here; runtime grants still authorize each call.
#[coglet_guest_rust::coglet]
pub fn wake<H: coglet_guest_rust::Host + DbHost>(
    context: &mut coglet_guest_rust::Context<'_, H>,
    events: &[coglet_guest_rust::Event],
) -> std::result::Result<(), coglet_guest_rust::HostError> {
    for event in events {
        if event.kind != "factory.inspect" {
            continue;
        }
        let id = event
            .payload
            .get("id")
            .and_then(Value::as_text)
            .ok_or(coglet_guest_rust::HostError::InvalidArgument)?;
        let object = Repository {
            client: context.database(),
        }
        .get_object(id)
        .map_err(|error| match error {
            DbError::Host(error) => error,
            _ => coglet_guest_rust::HostError::InvalidArgument,
        })?;
        context.log(&coglet_guest_rust::LogEntry {
            level: "info".into(),
            message: "factory.inspect".into(),
            fields: [
                ("object_id".into(), text(id)),
                (
                    "object_status".into(),
                    object.map(|o| text(&o.status)).unwrap_or(Value::Null),
                ),
            ]
            .into(),
        })?;
    }
    Ok(())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Object {
    pub id: String,
    pub object_type: String,
    pub status: String,
    pub data_json: String,
    pub version: u64,
    pub created_at: u64,
    pub updated_at: u64,
}
#[derive(Clone, Debug)]
pub struct SemanticEvent {
    pub id: String,
    pub object_id: Option<String>,
    pub event_type: String,
    pub payload_json: String,
    pub created_at: u64,
}
type Result<T> = std::result::Result<T, DbError>;
/// Application-generated ULID using recorded inputs, never ambient time/random.
/// Callers must persist entropy/counter uniqueness across attempts and trace resets.
/// ABI outbox intent/key IDs remain the frozen 32-hex action/correlation IDs;
/// do not send this 26-character representation where those wire types are required.
pub fn ulid(timestamp_ms: u64, entropy: [u8; 10]) -> Result<String> {
    if timestamp_ms >= 1u64 << 48 {
        return Err(invalid());
    }
    let mut bytes = [0u8; 16];
    bytes[..6].copy_from_slice(&timestamp_ms.to_be_bytes()[2..]);
    bytes[6..].copy_from_slice(&entropy);
    let mut n = u128::from_be_bytes(bytes);
    let alphabet = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let mut output = [b'0'; 26];
    for byte in output.iter_mut().rev() {
        *byte = alphabet[(n & 31) as usize];
        n >>= 5;
    }
    Ok(String::from_utf8(output.to_vec()).expect("ULID alphabet is ASCII"))
}
fn invalid() -> DbError {
    DbError::Protocol("invalid Factory row or value".into())
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn uint(n: u64) -> Result<Value> {
    if n > i64::MAX as u64 {
        return Err(invalid());
    }
    Ok(Value::Integer(n.into()))
}
fn row_object(row: &[Value]) -> Result<Object> {
    if row.len() != 7 {
        return Err(invalid());
    }
    let t = |i: usize| row[i].as_text().map(str::to_owned).ok_or_else(invalid);
    let n = |i: usize| {
        row[i]
            .as_integer()
            .and_then(|n| u64::try_from(i128::from(n)).ok())
            .ok_or_else(invalid)
    };
    Ok(Object {
        id: t(0)?,
        object_type: t(1)?,
        status: t(2)?,
        data_json: t(3)?,
        version: n(4)?,
        created_at: n(5)?,
        updated_at: n(6)?,
    })
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relation {
    pub id: String,
    pub source_id: String,
    pub target_id: String,
    pub relation_type: String,
    pub data_json: String,
    pub version: u64,
}
pub struct QueryPage<T> {
    pub items: Vec<T>,
    pub revision: u64,
    pub next_cursor: Option<String>,
}
fn query_page<T>(
    page: DatabaseDbPage,
    decode: impl Fn(&[Value]) -> Result<T>,
) -> Result<QueryPage<T>> {
    Ok(QueryPage {
        items: page.rows.iter().map(|r| decode(r)).collect::<Result<_>>()?,
        revision: page.revision,
        next_cursor: match page.next_cursor {
            Value::Null => None,
            Value::Text(s) => Some(s),
            _ => return Err(invalid()),
        },
    })
}
fn row_relation(row: &[Value]) -> Result<Relation> {
    if row.len() != 6 {
        return Err(invalid());
    }
    let t = |i: usize| row[i].as_text().map(str::to_owned).ok_or_else(invalid);
    Ok(Relation {
        id: t(0)?,
        source_id: t(1)?,
        target_id: t(2)?,
        relation_type: t(3)?,
        data_json: t(4)?,
        version: row[5]
            .as_integer()
            .and_then(|n| u64::try_from(i128::from(n)).ok())
            .ok_or_else(invalid)?,
    })
}
pub struct Repository<'a, H: DbHost> {
    client: Client<'a, H>,
}
impl<'a, H: DbHost> Repository<'a, H> {
    pub fn new(host: &'a mut H) -> Self {
        Self {
            client: Client::new(host),
        }
    }
    fn call(&mut self, id: &str, params: Vec<Value>) -> Result<DatabaseDbPage> {
        let definition = catalog::STATEMENTS
            .iter()
            .find(|s| s.id == id)
            .ok_or_else(invalid)?;
        let page = self.client.statement(&DatabaseDbStatement {
            operation: "statement".into(),
            statement: id.into(),
            mode: definition.mode.into(),
            params,
            max_rows: 64,
            max_bytes: 65536,
            cursor: None,
        })?;
        if definition
            .expected_affected
            .is_some_and(|expected| expected != page.affected)
        {
            return Err(DbError::Host(coglet_guest_rust::HostError::Conflict));
        }
        Ok(page)
    }
    pub fn query_objects(
        &mut self,
        object_type: &str,
        status: &str,
        max_rows: u64,
        max_bytes: u64,
        cursor: Option<String>,
    ) -> Result<QueryPage<Object>> {
        let page = self.client.statement(&DatabaseDbStatement {
            operation: "statement".into(),
            statement: "objects_query".into(),
            mode: "read".into(),
            params: vec![text(object_type), text(status)],
            max_rows,
            max_bytes,
            cursor,
        })?;
        query_page(page, row_object)
    }
    pub fn get_relations(
        &mut self,
        source_id: &str,
        relation_type: &str,
        max_rows: u64,
        max_bytes: u64,
        cursor: Option<String>,
    ) -> Result<QueryPage<Relation>> {
        let page = self.client.statement(&DatabaseDbStatement {
            operation: "statement".into(),
            statement: "relations_get".into(),
            mode: "read".into(),
            params: vec![text(source_id), text(relation_type)],
            max_rows,
            max_bytes,
            cursor,
        })?;
        query_page(page, row_relation)
    }
    /// Dedicated enqueue only. The host owns immutable-key dedup, authorization,
    /// poison and commit/publication fencing; this reply remains provisional.
    pub fn enqueue_outbox(
        &mut self,
        request: &coglet_types::DatabaseDbEnqueue,
    ) -> Result<coglet_types::DatabaseDbEnqueued> {
        self.client.enqueue(request)
    }
    pub fn get_object(&mut self, id: &str) -> Result<Option<Object>> {
        let page = self.call("objects_get", vec![text(id)])?;
        if page.rows.len() > 1 || page.next_cursor != Value::Null {
            return Err(invalid());
        }
        page.rows.first().map(|r| row_object(r)).transpose()
    }
    /// Compare-and-swap on the supplied version; host conflict poisons the step.
    pub fn update_object(&mut self, object: &Object) -> Result<()> {
        if object.version >= i64::MAX as u64 {
            return Err(invalid());
        }
        self.call(
            "objects_update",
            vec![
                text(&object.status),
                text(&object.data_json),
                uint(object.updated_at)?,
                text(&object.id),
                uint(object.version)?,
            ],
        )?;
        Ok(())
    }
    pub fn append_event(&mut self, event: &SemanticEvent) -> Result<()> {
        self.call(
            "events_append",
            vec![
                text(&event.id),
                event.object_id.as_deref().map(text).unwrap_or(Value::Null),
                text(&event.event_type),
                text(&event.payload_json),
                uint(event.created_at)?,
                uint(1)?,
            ],
        )?;
        Ok(())
    }
    pub fn create_object(&mut self, object: &Object) -> Result<()> {
        self.call(
            "objects_create",
            vec![
                text(&object.id),
                text(&object.object_type),
                text(&object.status),
                text(&object.data_json),
                uint(object.version)?,
                uint(object.created_at)?,
                uint(object.updated_at)?,
            ],
        )?;
        Ok(())
    }
}
