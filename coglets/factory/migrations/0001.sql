CREATE TABLE objects (
    id TEXT PRIMARY KEY,
    type TEXT NOT NULL,
    status TEXT NOT NULL,
    data_json TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0),
    created_at BIGINT NOT NULL,
    updated_at BIGINT NOT NULL
);
CREATE INDEX objects_type_status_id ON objects(type, status, id);
CREATE TABLE relations (
    id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES objects(id),
    target_id TEXT NOT NULL REFERENCES objects(id),
    type TEXT NOT NULL,
    data_json TEXT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0)
);
CREATE INDEX relations_source_type_id ON relations(source_id, type, id);
CREATE INDEX relations_target_type_id ON relations(target_id, type, id);
CREATE TABLE events (
    id TEXT PRIMARY KEY,
    object_id TEXT REFERENCES objects(id),
    event_type TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at BIGINT NOT NULL,
    version BIGINT NOT NULL CHECK (version > 0)
);
CREATE INDEX events_object_created_id ON events(object_id, created_at, id);
CREATE TABLE work_queue (
    task_id TEXT PRIMARY KEY REFERENCES objects(id),
    state TEXT NOT NULL,
    priority BIGINT NOT NULL,
    lease_owner TEXT,
    lease_until BIGINT,
    attempt BIGINT NOT NULL CHECK (attempt >= 0),
    version BIGINT NOT NULL CHECK (version > 0)
);
CREATE INDEX work_queue_state_priority_task ON work_queue(state, priority, task_id);
CREATE TABLE outbox (
    intent_id TEXT PRIMARY KEY,
    idempotency_key TEXT NOT NULL UNIQUE,
    kind TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    correlation_id TEXT NOT NULL,
    state TEXT NOT NULL CHECK (state IN ('pending','dispatching','uncertain','succeeded','failed','cancelled')),
    attempt BIGINT NOT NULL CHECK (attempt >= 0),
    version BIGINT NOT NULL CHECK (version > 0)
);
CREATE INDEX outbox_state_intent ON outbox(state, intent_id);
CREATE TABLE schema_migrations (
    version BIGINT PRIMARY KEY,
    applied_at BIGINT NOT NULL,
    migration_hash TEXT NOT NULL
);
