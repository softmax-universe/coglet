//! Deployment catalog data, never SQL accepted from a guest call.
#[derive(Clone, Copy)]
pub struct Statement {
    pub id: &'static str,
    pub sql: &'static str,
    pub mode: &'static str,
    pub expected_affected: Option<u64>,
}
pub const STATEMENTS: &[Statement] = &[
    Statement {
        id: "objects_query",
        sql: "SELECT id, type, status, data_json, version, created_at, updated_at FROM objects WHERE type=?1 AND status=?2 ORDER BY id",
        mode: "read",
        expected_affected: None,
    },
    Statement {
        id: "relations_get",
        sql: "SELECT id, source_id, target_id, type, data_json, version FROM relations WHERE source_id=?1 AND type=?2 ORDER BY id",
        mode: "read",
        expected_affected: None,
    },
    Statement {
        id: "objects_get",
        sql: "SELECT id, type, status, data_json, version, created_at, updated_at FROM objects WHERE id=?1 ORDER BY id",
        mode: "read",
        expected_affected: None,
    },
    Statement {
        id: "objects_update",
        sql: "UPDATE objects SET status=?1, data_json=?2, updated_at=?3, version=version+1 WHERE id=?4 AND version=?5",
        mode: "write",
        expected_affected: Some(1),
    },
    Statement {
        id: "events_append",
        sql: "INSERT INTO events(id, object_id, event_type, payload_json, created_at, version) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        mode: "write",
        expected_affected: Some(1),
    },
    Statement {
        id: "objects_create",
        sql: "INSERT INTO objects(id, type, status, data_json, version, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        mode: "write",
        expected_affected: Some(1),
    },
];
