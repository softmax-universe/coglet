//! Native private database substrate for ADR-0013.
//!
//! This crate is not yet wired into runtime ABI 0.2. The only production commit
//! path is the milestone B coordinator (`Unit::complete`), which prepares a
//! decision, commits the host marker and publishes the exact journal; recovery is
//! `Database::recover`. Runtime/engine admission remains closed.
use fs2::FileExt;
use rusqlite::{Connection, OpenFlags, params_from_iter, types::ValueRef};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

mod admission;
mod catalog;
mod coordinator;
mod dispatch;
mod maintenance;
mod outbox;
mod protocol;
pub use admission::{Admission, Pins};
pub use catalog::{CellType, Column, TypedStatement};
pub use coordinator::{
    CORRUPTION, Decision, HALTED, HELD, Journal, Recovered, StepId, recovery_abort_record,
};
pub use maintenance::{
    ArchivedDecision, CodeRange, CompatibilityFloor, Migration, OperatorApproval, RecoveryBundle,
    RestoreReport, RestoreVerifier, Snapshot,
};
pub use outbox::{Enqueued, Intent, OutboxRecord, OutboxState, WorkQueueRow};
pub use protocol::{CallResult, HostBudget};

pub type Result<T> = std::result::Result<T, String>;

fn sqlite_failure(error: rusqlite::Error) -> String {
    use rusqlite::ErrorCode;
    match error.sqlite_error_code() {
        Some(ErrorCode::ConstraintViolation) => "sqlite constraint".into(),
        Some(ErrorCode::OperationInterrupted) => "sqlite interrupted".into(),
        _ => "sqlite unavailable".into(),
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Read,
    Write,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    Null,
    Integer(i64),
    Unsigned(u64),
    Text(String),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grants {
    pub read: bool,
    pub write: bool,
}

/// Trusted deployment configuration, never supplied by guest calls.
pub struct Statement {
    id: String,
    sql: String,
    mode: Mode,
    parameters: usize,
    expected_affected: Option<u64>,
    binding: Option<catalog::Binding>,
}
impl Statement {
    pub fn new(id: &str, sql: &str, mode: Mode, parameters: usize) -> Self {
        Self {
            id: id.into(),
            sql: sql.into(),
            mode,
            parameters,
            expected_affected: None,
            binding: None,
        }
    }
    /// A compare-and-swap statement must affect exactly one row. A mismatch is
    /// a conflict that poisons the entire unit, not an ignorable partial update.
    pub fn expect_affected(mut self, count: u64) -> Self {
        self.expected_affected = Some(count);
        self
    }
}
pub struct Catalog {
    statements: BTreeMap<String, Statement>,
}
impl Catalog {
    fn approve(statements: Vec<Statement>) -> Result<Self> {
        let mut entries = BTreeMap::new();
        for entry in statements {
            // Intentionally narrow portable subset. Catalogs are trusted pinned
            // deployment config, not a second guest SQL interface. An SQLite
            // authorizer additionally checks actual table/function accesses.
            let sql = entry.sql.to_ascii_uppercase();
            let permitted_start = match entry.mode {
                Mode::Read => sql.starts_with("SELECT ") && sql.contains(" ORDER BY "),
                Mode::Write => ["INSERT INTO ", "UPDATE ", "DELETE FROM "]
                    .iter()
                    .any(|s| sql.starts_with(s)),
            };
            let forbidden_syntax = [';', '\'', '"', '`', '[', ']']
                .iter()
                .any(|c| sql.contains(*c))
                || sql.contains("--")
                || sql.contains("/*")
                || sql.contains("RETURNING")
                || sql.contains("REPLACE")
                || sql.contains("CURRENT_");
            // Calls are forbidden, including clock/random/extensions. Bare
            // INSERT column and VALUES lists are the only admitted parentheses.
            let tokens = sql.replace('(', " ( ").replace(')', " ) ");
            let words: Vec<_> = tokens.split_whitespace().collect();
            let function = words.iter().enumerate().any(|(i, word)| {
                *word == "("
                    && (entry.mode == Mode::Read
                        || i == 0
                        || !["OBJECTS", "RELATIONS", "EVENTS", "WORK_QUEUE", "VALUES"]
                            .contains(&words[i - 1]))
            });
            if !permitted_start || forbidden_syntax || function {
                return Err("unsafe catalog SQL".into());
            }
            if entry.id.is_empty()
                || entry.id.len() > 64
                || !entry.id.bytes().enumerate().all(|(i, c)| {
                    c.is_ascii_lowercase() || (i > 0 && (c.is_ascii_digit() || c == b'_'))
                })
                || entry.parameters > 32
            {
                return Err("invalid statement catalog entry".into());
            }
            if entries.insert(entry.id.clone(), entry).is_some() {
                return Err("duplicate statement ID".into());
            }
        }
        Ok(Self {
            statements: entries,
        })
    }
}
#[derive(Debug)]
pub struct Page {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Cell>>,
    pub affected: u64,
    pub next_cursor: Option<String>,
}
impl Page {
    /// Exact CBOR size without producing intermediate reply buffers. This keeps
    /// host-byte accounting tied to the one final result materialization.
    fn encoded_len(&self, revision: u64) -> usize {
        fn head(n: u64) -> usize {
            match n {
                0..=23 => 1,
                24..=255 => 2,
                256..=65535 => 3,
                65536..=4294967295 => 5,
                _ => 9,
            }
        }
        fn text(s: &str) -> usize {
            head(s.len() as u64) + s.len()
        }
        fn cell(c: &Cell) -> usize {
            match c {
                Cell::Null => 1,
                Cell::Integer(n) => head(if *n < 0 {
                    (-1i128 - *n as i128) as u64
                } else {
                    *n as u64
                }),
                Cell::Unsigned(n) => head(*n),
                Cell::Text(s) => text(s),
            }
        }
        1 + text("status")
            + text("page")
            + text("columns")
            + head(self.columns.len() as u64)
            + self.columns.iter().map(|s| text(s)).sum::<usize>()
            + text("rows")
            + head(self.rows.len() as u64)
            + self
                .rows
                .iter()
                .map(|r| head(r.len() as u64) + r.iter().map(cell).sum::<usize>())
                .sum::<usize>()
            + text("affected")
            + head(self.affected)
            + text("revision")
            + head(revision)
            + text("next_cursor")
            + self.next_cursor.as_ref().map_or(1, |s| text(s))
    }
    /// Canonical wire reply. Caller supplies the committed admission revision.
    pub fn encode(&self, revision: u64) -> Result<Vec<u8>> {
        use coglet_types::Value as V;
        let text = |s: &str| V::Text(s.into());
        coglet_types::encode_profile(&V::Map(vec![
            (text("status"), text("page")),
            (
                text("columns"),
                V::Array(self.columns.iter().map(|s| text(s)).collect()),
            ),
            (
                text("rows"),
                V::Array(
                    self.rows
                        .iter()
                        .map(|row| {
                            V::Array(
                                row.iter()
                                    .map(|c| match c {
                                        Cell::Null => V::Null,
                                        Cell::Integer(i) => V::Integer((*i).into()),
                                        Cell::Unsigned(i) => V::Integer((*i).into()),
                                        Cell::Text(s) => text(s),
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                ),
            ),
            (text("affected"), V::Integer(self.affected.into())),
            (text("revision"), V::Integer(revision.into())),
            (
                text("next_cursor"),
                self.next_cursor.as_ref().map_or(V::Null, |s| text(s)),
            ),
        ]))
    }
}

pub struct Database {
    connection: Connection,
    path: PathBuf,
    _lease: File,
    owner: String,
}
impl Database {
    /// Caller supplies a trusted host-only root and authenticated owner identity.
    /// The guest never receives this method, a path, or a connection.
    pub fn open(root: &Path, owner: &str) -> Result<Self> {
        if owner.is_empty() || owner.len() > 128 {
            return Err("invalid owner".into());
        }
        let directory = root.join(hex::encode(Sha256::digest(owner.as_bytes())));
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        match builder.create(&directory) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
        let metadata = fs::symlink_metadata(&directory).map_err(|e| e.to_string())?;
        if !metadata.file_type().is_dir() {
            return Err("private directory is not a directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let trusted = fs::metadata(root).map_err(|e| e.to_string())?;
            if metadata.uid() != trusted.uid() || metadata.mode() & 0o077 != 0 {
                return Err("private directory ownership or permissions".into());
            }
        }
        let lock_path = directory.join("owner.lock");
        match fs::symlink_metadata(&lock_path) {
            Ok(m) if !m.file_type().is_file() => return Err("invalid owner lock".into()),
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let lease = options
            .open(directory.join("owner.lock"))
            .map_err(|e| e.to_string())?;
        // A restarting owner can briefly overlap a dying holder, and a concurrent
        // fork can hold an inherited descriptor until it execs. Retry briefly.
        let started = Instant::now();
        while lease.try_lock_exclusive().is_err() {
            if started.elapsed() > Duration::from_millis(500) {
                return Err("owner already open".to_string());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let path = directory.join("private.sqlite3");
        let connection = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600))
                .map_err(|e| e.to_string())?;
        }
        connection
            .busy_timeout(Duration::ZERO)
            .map_err(|e| e.to_string())?;
        connection.execute_batch("PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; PRAGMA trusted_schema=OFF;").map_err(|e| e.to_string())?;
        let settings = connection.query_row("SELECT (SELECT journal_mode FROM pragma_journal_mode), (SELECT synchronous FROM pragma_synchronous), (SELECT foreign_keys FROM pragma_foreign_keys), (SELECT trusted_schema FROM pragma_trusted_schema)", [], |r| Ok((r.get::<_, String>(0)?, r.get::<_, u32>(1)?, r.get::<_, u32>(2)?, r.get::<_, u32>(3)?))).map_err(sqlite_failure)?;
        if settings != ("wal".into(), 2, 1, 0) {
            return Err("database durability configuration refused".into());
        }
        admission::initialize(&connection)?;
        Ok(Self {
            connection,
            path,
            _lease: lease,
            owner: owner.into(),
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    /// Authenticated host-selected owner, never selected by a guest request.
    pub fn owner(&self) -> &str {
        &self.owner
    }
    #[cfg(test)]
    fn begin<'a>(
        &'a mut self,
        catalog: &'a Catalog,
        grants: Grants,
        deadline: Instant,
    ) -> Result<Unit<'a>> {
        if Instant::now() >= deadline {
            return Err("deadline exceeded".into());
        }
        if !self.connection.is_autocommit() {
            return Err("unreconciled database unit".into());
        }
        self.connection
            .progress_handler(1000, Some(move || Instant::now() >= deadline))
            .map_err(|e| e.to_string())?;
        if let Err(error) = self.connection.execute_batch("BEGIN IMMEDIATE") {
            let _ = self.connection.progress_handler(0, None::<fn() -> bool>);
            return Err(error.to_string());
        }
        Ok(Unit {
            db: self,
            catalog,
            grants,
            deadline,
            poisoned: false,
            calls: 0,
            intent_grants: None,
            finished: false,
            pins: None,
            revision: 0,
            attempt: 0,
            wrote: false,
            paging: false,
            transaction: true,
        })
    }
    #[cfg(test)]
    fn provision_for_test(&mut self, sql: &str) -> Result<()> {
        self.connection
            .execute_batch(sql)
            .map_err(|e| e.to_string())
    }
}

/// A short, single-step unit. Drop always rolls back unless the host has completed
/// the recovery protocol (not exposed in this initial substrate).
pub struct Unit<'a> {
    db: &'a mut Database,
    catalog: &'a Catalog,
    grants: Grants,
    deadline: Instant,
    poisoned: bool,
    calls: usize,
    intent_grants: Option<std::collections::BTreeSet<String>>,
    finished: bool,
    pins: Option<Pins>,
    revision: u64,
    attempt: u64,
    wrote: bool,
    paging: bool,
    transaction: bool,
}
impl Unit<'_> {
    pub fn statement(
        &mut self,
        id: &str,
        mode: Mode,
        params: &[Cell],
        max_rows: usize,
        max_bytes: usize,
    ) -> Result<Page> {
        self.statement_page(id, mode, params, max_rows, max_bytes, None)
    }
    pub fn statement_page(
        &mut self,
        id: &str,
        mode: Mode,
        params: &[Cell],
        max_rows: usize,
        max_bytes: usize,
        page_cursor: Option<&str>,
    ) -> Result<Page> {
        self.calls = self.calls.saturating_add(1);
        let result = self.execute(id, mode, params, max_rows, max_bytes, page_cursor);
        let result = if result.is_ok() && Instant::now() >= self.deadline {
            Err("resource exhausted".into())
        } else {
            result
        };
        if result.is_err() {
            self.poisoned = true;
            self.rollback_failed_unit();
        }
        if let Err(error) = self.db.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        ) {
            self.poisoned = true;
            return Err(error.to_string());
        }
        result
    }
    fn execute(
        &mut self,
        id: &str,
        mode: Mode,
        params: &[Cell],
        max_rows: usize,
        max_bytes: usize,
        page_cursor: Option<&str>,
    ) -> Result<Page> {
        if !(1..=64).contains(&max_rows)
            || !(256..=65536).contains(&max_bytes)
            || params
                .iter()
                .any(|p| matches!(p, Cell::Text(s) if s.len() > 4096))
            || params.len() > 32
            || page_cursor.is_some_and(|c| c.is_empty() || c.len() > 512 || mode != Mode::Read)
        {
            return Err("invalid bounds".into());
        }
        if (mode == Mode::Read && !self.grants.read) || (mode == Mode::Write && !self.grants.write)
        {
            return Err("permission denied".into());
        }
        if self.poisoned {
            return Err("poisoned unit".into());
        }
        if (mode == Mode::Write && self.paging) || (page_cursor.is_some() && self.wrote) {
            return Err("paging conflict".into());
        }
        let continuation = self.cursor_position(id, params, page_cursor)?;
        if self.calls > 32 || Instant::now() >= self.deadline {
            return Err("resource exhausted".into());
        }
        let spec = self.catalog.statements.get(id).ok_or("unknown statement")?;
        if spec.mode != mode || spec.parameters != params.len() {
            return Err("statement binding mismatch".into());
        }
        if mode == Mode::Write && self.pins.is_some() && self.revision == u64::MAX {
            return Err("revision overflow".into());
        }
        if spec.binding.as_ref().is_some_and(|binding| {
            !binding
                .parameters
                .iter()
                .zip(params)
                .all(|(kind, value)| kind.accepts(value))
        }) {
            return Err("parameter type mismatch".into());
        }
        let values: Vec<_> = params
            .iter()
            .map(|c| match c {
                Cell::Null => rusqlite::types::Value::Null,
                Cell::Integer(i) => (*i).into(),
                Cell::Unsigned(i) => rusqlite::types::Value::Blob(i.to_be_bytes().to_vec()),
                Cell::Text(s) => s.clone().into(),
            })
            .collect();
        self.start_transaction()?;
        if let Some(binding) = &spec.binding {
            binding.validate_order(&self.db.connection)?;
        }
        let permitted_tables = spec.binding.as_ref().map(|binding| binding.tables.clone());
        self.db
            .connection
            .authorizer(Some(move |ctx: rusqlite::hooks::AuthContext<'_>| {
                use rusqlite::hooks::{AuthAction as A, Authorization as Decision};
                let application = |table: &str| {
                    ["objects", "relations", "events", "work_queue"].contains(&table)
                        && permitted_tables
                            .as_ref()
                            .is_none_or(|tables| tables.iter().any(|t| t == table))
                };
                if ctx.database_name.is_some_and(|name| name != "main") || ctx.accessor.is_some() {
                    return Decision::Deny;
                }
                let allowed = match ctx.action {
                    A::Select => true,
                    A::Read { table_name, .. } => application(table_name),
                    A::Insert { table_name }
                    | A::Delete { table_name }
                    | A::Update { table_name, .. } => {
                        mode == Mode::Write
                            && application(table_name)
                            && permitted_tables.as_ref().is_none_or(|tables| {
                                tables.first().is_some_and(|t| t == table_name)
                            })
                    }
                    _ => false,
                };
                if allowed {
                    Decision::Allow
                } else {
                    Decision::Deny
                }
            }))
            .map_err(|e| e.to_string())?;
        let mut statement = self
            .db
            .connection
            .prepare(&spec.sql)
            .map_err(|e| e.to_string())?;
        if statement.parameter_count() != spec.parameters
            || statement.readonly() != (mode == Mode::Read)
        {
            return Err("catalog SQL binding mismatch".into());
        }
        if mode == Mode::Write {
            let affected = statement
                .execute(params_from_iter(values))
                .map_err(sqlite_failure)? as u64;
            self.wrote = true;
            if spec
                .expected_affected
                .is_some_and(|count| count != affected)
            {
                return Err("optimistic version conflict".into());
            }
            return Ok(Page {
                columns: vec![],
                rows: vec![],
                affected,
                next_cursor: None,
            });
        }
        let columns = statement
            .column_names()
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>();
        if spec
            .binding
            .as_ref()
            .is_some_and(|binding| binding.columns.iter().map(|c| &c.name).ne(columns.iter()))
        {
            return Err("result column binding mismatch".into());
        }
        if columns.len() > 16
            || columns.iter().any(|c| c.is_empty() || c.len() > 4096)
            || columns
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                != columns.len()
        {
            return Err("invalid result columns".into());
        }
        let encoding_revision = if self.pins.is_some() {
            self.revision
        } else {
            u64::MAX
        };
        if (Page {
            columns: columns.clone(),
            rows: vec![],
            affected: 0,
            next_cursor: None,
        })
        .encoded_len(encoding_revision)
            > max_bytes
        {
            return Err("resource exhausted".into());
        }
        let mut cursor = statement
            .query(params_from_iter(values))
            .map_err(|e| e.to_string())?;
        let mut rows = vec![];
        let mut position = 0u64;
        let mut next_position = None;
        while let Some(row) = cursor.next().map_err(sqlite_failure)? {
            if Instant::now() >= self.deadline {
                return Err("resource exhausted".into());
            }
            let mut cells = vec![];
            for index in 0..columns.len() {
                cells.push(match row.get_ref(index).map_err(|e| e.to_string())? {
                    ValueRef::Null => Cell::Null,
                    ValueRef::Integer(i) => Cell::Integer(i),
                    ValueRef::Blob(b)
                        if b.len() == 8
                            && spec.binding.as_ref().is_some_and(|s| {
                                matches!(
                                    s.columns[index].kind,
                                    CellType::Unsigned | CellType::NullableUnsigned
                                )
                            }) =>
                    {
                        Cell::Unsigned(u64::from_be_bytes(b.try_into().unwrap()))
                    }
                    ValueRef::Text(s) if s.len() <= 4096 => {
                        Cell::Text(std::str::from_utf8(s).map_err(|e| e.to_string())?.into())
                    }
                    _ => return Err("unrepresentable cell".into()),
                });
            }
            if spec.binding.as_ref().is_some_and(|binding| {
                binding
                    .columns
                    .iter()
                    .zip(&cells)
                    .any(|(column, cell)| !column.kind.accepts(cell))
            }) {
                return Err("result type mismatch".into());
            }
            if position < continuation.0 {
                position += 1;
                if position == continuation.0
                    && continuation
                        .1
                        .as_ref()
                        .is_some_and(|key| key != &admission::row_key(&cells))
                {
                    return Err("cursor conflict".into());
                }
                continue;
            }
            if rows.len() == max_rows {
                next_position = Some(position);
                break;
            }
            rows.push(cells);
            position += 1;
            // Reserve the largest possible revision encoding until runtime
            // admission supplies the pinned revision. Never truncate a row.
            let bounded = Page {
                columns: columns.clone(),
                rows: rows.clone(),
                affected: 0,
                next_cursor: None,
            };
            if bounded.encoded_len(encoding_revision) > max_bytes {
                rows.pop();
                if rows.is_empty() {
                    return Err("resource exhausted".into());
                }
                next_position = Some(position - 1);
                break;
            }
        }
        drop(cursor);
        drop(statement);
        if position < continuation.0 {
            return Err("cursor conflict".into());
        }
        // Host cursor metadata is outside the application's authorizer scope.
        self.db
            .connection
            .authorizer(
                None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
            )
            .map_err(|e| e.to_string())?;
        let next_cursor = if let Some(mut position) = next_position {
            if self.pins.is_none() {
                return Err("resource exhausted".into());
            }
            if self.wrote {
                return Err("paging conflict".into());
            }
            while (Page {
                columns: columns.clone(),
                rows: rows.clone(),
                affected: 0,
                next_cursor: Some("0".repeat(64)),
            })
            .encoded_len(self.revision)
                > max_bytes
            {
                rows.pop();
                position -= 1;
                if rows.is_empty() {
                    return Err("resource exhausted".into());
                }
            }
            self.mint_cursor(id, params, position, rows.last().unwrap())?
        } else {
            None
        };
        self.paging |= page_cursor.is_some() || next_cursor.is_some();
        let page = Page {
            columns,
            rows,
            affected: 0,
            next_cursor,
        };
        if page.encoded_len(self.revision) > max_bytes {
            return Err("resource exhausted".into());
        }
        Ok(page)
    }
    #[cfg(test)]
    fn commit_for_test(mut self) -> Result<()> {
        if self.poisoned {
            return Err("poisoned unit".into());
        }
        if self.wrote && self.pins.is_some() {
            let next = self.resulting_revision()?;
            self.db
                .connection
                .execute(
                    "UPDATE _coglet_state SET revision=?1 WHERE singleton=1",
                    [next.to_be_bytes().to_vec()],
                )
                .map_err(|e| e.to_string())?;
        }
        if !self.transaction {
            self.finished = true;
            return Ok(());
        }
        self.db
            .connection
            .execute_batch("COMMIT")
            .map_err(|e| e.to_string())?;
        self.finished = true;
        Ok(())
    }
}
impl Drop for Unit<'_> {
    fn drop(&mut self) {
        let _ = self.db.connection.progress_handler(0, None::<fn() -> bool>);
        let _ = self.db.connection.authorizer(
            None::<fn(rusqlite::hooks::AuthContext<'_>) -> rusqlite::hooks::Authorization>,
        );
        if !self.finished {
            let _ = self.db.connection.execute_batch("ROLLBACK");
        }
    }
}
#[cfg(test)]
mod coordinator_tests;
#[cfg(test)]
mod native_tests;
#[cfg(test)]
mod precedence_tests;
#[cfg(test)]
mod tests;
