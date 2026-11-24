//! Host-owned typed deployment catalog. Never constructed from guest requests.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellType {
    Null,
    Integer,
    Unsigned,
    Text,
    NullableInteger,
    NullableUnsigned,
    NullableText,
}
impl CellType {
    pub(crate) fn accepts(self, cell: &Cell) -> bool {
        matches!(
            (self, cell),
            (
                Self::Null | Self::NullableInteger | Self::NullableText | Self::NullableUnsigned,
                Cell::Null
            ) | (Self::Integer | Self::NullableInteger, Cell::Integer(_))
                | (Self::Text | Self::NullableText, Cell::Text(_))
                | (Self::Unsigned | Self::NullableUnsigned, Cell::Unsigned(_))
        )
    }
}
#[derive(Clone, Debug)]
pub struct Column {
    pub name: String,
    pub kind: CellType,
}
pub struct TypedStatement {
    pub statement: Statement,
    pub parameters: Vec<CellType>,
    pub columns: Vec<Column>,
    /// Read allowlist; first entry is the only mutation target for writes.
    /// Later entries declare FK reads, never mutation authority.
    pub tables: Vec<String>,
    /// Exact ascending ORDER BY columns, including a schema-unique tie-break key.
    pub order: Vec<String>,
}
pub(crate) struct Binding {
    pub parameters: Vec<CellType>,
    pub columns: Vec<Column>,
    pub tables: Vec<String>,
    pub order: Vec<String>,
}
impl Catalog {
    /// SHA256 of deterministic CBOR deployment metadata, sorted by statement ID.
    /// This encoding is a host catalog format, not a new guest message schema.
    pub fn digest(&self) -> Result<String> {
        use coglet_types::Value as V;
        let text = |s: &str| V::Text(s.into());
        let mut entries = Vec::new();
        for statement in self.statements.values() {
            let binding = statement
                .binding
                .as_ref()
                .ok_or("untyped catalog cannot be pinned")?;
            let kind = |k: CellType| {
                text(match k {
                    CellType::Null => "null",
                    CellType::Integer => "int64",
                    CellType::Unsigned => "uint64",
                    CellType::Text => "text",
                    CellType::NullableInteger => "nullable_int64",
                    CellType::NullableUnsigned => "nullable_uint64",
                    CellType::NullableText => "nullable_text",
                })
            };
            entries.push(V::Map(vec![
                (text("id"), text(&statement.id)),
                (text("sql"), text(&statement.sql)),
                (
                    text("mode"),
                    text(if statement.mode == Mode::Read {
                        "read"
                    } else {
                        "write"
                    }),
                ),
                (
                    text("parameters"),
                    V::Array(binding.parameters.iter().map(|k| kind(*k)).collect()),
                ),
                (
                    text("columns"),
                    V::Array(
                        binding
                            .columns
                            .iter()
                            .map(|c| V::Array(vec![text(&c.name), kind(c.kind)]))
                            .collect(),
                    ),
                ),
                (
                    text("tables"),
                    V::Array(binding.tables.iter().map(|t| text(t)).collect()),
                ),
                (
                    text("order"),
                    V::Array(binding.order.iter().map(|t| text(t)).collect()),
                ),
                (
                    text("expected_affected"),
                    statement
                        .expected_affected
                        .map_or(V::Null, |n| V::Integer(n.into())),
                ),
            ]));
        }
        let bytes = coglet_types::encode_profile(&V::Map(vec![
            (text("version"), text("0.2")),
            (text("statements"), V::Array(entries)),
        ]))?;
        Ok(hex::encode(Sha256::digest(bytes)))
    }

    pub fn approve_typed(statements: Vec<TypedStatement>) -> Result<Self> {
        let mut bound = Vec::new();
        for typed in statements {
            let mut statement = typed.statement;
            if typed.parameters.len() != statement.parameters {
                return Err("catalog parameter count mismatch".into());
            }
            if typed.tables.is_empty()
                || typed.tables.len() > 4
                || typed.tables.iter().any(|t| {
                    !["objects", "relations", "events", "work_queue"].contains(&t.as_str())
                })
                || (statement.mode == Mode::Read && typed.tables.len() != 1)
                || typed.columns.len() > 16
                || typed.columns.iter().any(|c| !identifier(&c.name))
                || typed
                    .columns
                    .iter()
                    .map(|c| &c.name)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != typed.columns.len()
                || typed.order.iter().any(|s| !identifier(s))
            {
                return Err("invalid typed catalog metadata".into());
            }
            if statement.mode == Mode::Read {
                validate_read_shape(&statement.sql)?;
                let upper = statement.sql.to_ascii_uppercase();
                let (_, order) = upper
                    .split_once(" ORDER BY ")
                    .ok_or("missing total order")?;
                let expected = typed
                    .order
                    .iter()
                    .map(|s| s.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join(",");
                if typed.columns.is_empty()
                    || typed.order.is_empty()
                    || order.replace(' ', "") != expected
                    || [
                        " JOIN ",
                        " UNION ",
                        " GROUP ",
                        " DISTINCT ",
                        " INTERSECT ",
                        " EXCEPT ",
                    ]
                    .iter()
                    .any(|s| upper.contains(s))
                {
                    return Err("catalog total order mismatch".into());
                }
            } else if !typed.columns.is_empty() || !typed.order.is_empty() {
                return Err("write catalog must have no result metadata".into());
            }
            statement.binding = Some(Binding {
                parameters: typed.parameters,
                columns: typed.columns,
                tables: typed.tables,
                order: typed.order,
            });
            bound.push(statement);
        }
        Self::approve(bound)
    }
}

fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .enumerate()
            .all(|(i, c)| c.is_ascii_lowercase() || c == b'_' || (i > 0 && c.is_ascii_digit()))
}

// Deliberately small grammar: SELECT bare columns FROM one bare table, with
// optional AND-joined column/comparison/positional-parameter predicates.
// This is not a general SQL parser. Reject forms whose cardinality is unproven.
fn validate_read_shape(sql: &str) -> Result<()> {
    let (body, _) = sql
        .split_once(" ORDER BY ")
        .ok_or("unsupported read shape")?;
    let (select, from) = body
        .strip_prefix("SELECT ")
        .and_then(|s| s.split_once(" FROM "))
        .ok_or("unsupported read shape")?;
    if !select.split(',').all(|s| identifier(s.trim())) {
        return Err("unsupported read shape".into());
    }
    let (table, predicate) = from
        .split_once(" WHERE ")
        .map_or((from, None), |(t, p)| (t, Some(p)));
    if !identifier(table) {
        return Err("unsupported read shape".into());
    }
    if let Some(predicate) = predicate {
        for clause in predicate.split(" AND ") {
            let (column, parameter) = ["<=", ">=", "<>", "=", "<", ">"]
                .iter()
                .find_map(|op| clause.split_once(op))
                .ok_or("unsupported read predicate")?;
            let parameter = parameter
                .trim()
                .strip_prefix('?')
                .ok_or("unsupported read predicate")?;
            if !identifier(column.trim())
                || parameter.is_empty()
                || !parameter.bytes().all(|b| b.is_ascii_digit())
                || parameter
                    .parse::<usize>()
                    .ok()
                    .is_none_or(|n| !(1..=32).contains(&n))
            {
                return Err("unsupported read predicate".into());
            }
        }
    }
    Ok(())
}

impl Binding {
    /// Resolve a genuine nonnullable unique key in the live, pinned schema.
    /// Partial, expression and non-BINARY indexes are deliberately unsupported.
    pub(crate) fn validate_order(&self, connection: &Connection) -> Result<()> {
        if self.order.is_empty() {
            return Ok(());
        }
        let table = &self.tables[0]; // constrained application identifier at approval
        let mut info = connection
            .prepare("SELECT name, type, [notnull], pk FROM pragma_table_info(?1)")
            .map_err(|e| e.to_string())?;
        let columns = info
            .query_map([table], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                    row.get::<_, u32>(3)?,
                ))
            })
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        if self
            .order
            .iter()
            .any(|key| !columns.iter().any(|c| &c.0 == key))
        {
            return Err("unknown order column".into());
        }
        for key in &self.order {
            let (_, collation, _, _, _) = connection
                .column_metadata(Some("main"), table.as_str(), key.as_str())
                .map_err(|e| e.to_string())?;
            if collation.is_none_or(|s| s.to_bytes() != b"BINARY") {
                return Err("order requires BINARY column collation".into());
            }
        }
        // A single INTEGER PRIMARY KEY is a nonnullable rowid alias, even when
        // table_info's notnull bit is false. Other PRIMARY KEY forms need NOT NULL.
        let pk: Vec<_> = columns.iter().filter(|c| c.3 > 0).collect();
        let has_pk_index: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_index_list(?1) WHERE origin='pk')",
                [table],
                |row| row.get(0),
            )
            .map_err(|e| e.to_string())?;
        if pk.len() == 1
            && pk[0].1.eq_ignore_ascii_case("INTEGER")
            && !has_pk_index
            && self.order.contains(&pk[0].0)
        {
            return Ok(());
        }
        let mut indexes = connection
            .prepare("SELECT name FROM pragma_index_list(?1) WHERE [unique]=1 AND partial=0")
            .map_err(|e| e.to_string())?;
        let names = indexes
            .query_map([table], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        for name in names {
            let mut index = connection
                .prepare("SELECT name, coll FROM pragma_index_xinfo(?1) WHERE key=1 ORDER BY seqno")
                .map_err(|e| e.to_string())?;
            let keys = index
                .query_map([name], |r| {
                    Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            if !keys.is_empty()
                && keys.iter().all(|(name, coll)| {
                    name.as_ref().is_some_and(|name| {
                        coll == "BINARY"
                            && self.order.contains(name)
                            && columns.iter().any(|c| &c.0 == name && c.2)
                    })
                })
            {
                return Ok(());
            }
        }
        Err("order lacks nonnullable unique key".into())
    }
}
