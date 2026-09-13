-- Schema-2 collector state is separate from the legacy v1 replacement graph.
CREATE TABLE IF NOT EXISTS cider_nodes (
    node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
    state_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS cider_receipts (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    session_id TEXT NOT NULL,
    sequence TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    ack_json TEXT NOT NULL,
    received_at INTEGER NOT NULL,
    PRIMARY KEY(node_id, session_id, sequence)
);
CREATE INDEX IF NOT EXISTS cider_receipts_age ON cider_receipts(received_at);
CREATE TABLE IF NOT EXISTS cider_records (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    fingerprint TEXT NOT NULL,
    projected INTEGER NOT NULL DEFAULT 0,
    received_at INTEGER NOT NULL,
    PRIMARY KEY(node_id, entity_type, entity_id)
);
CREATE INDEX IF NOT EXISTS cider_records_age ON cider_records(received_at);
CREATE TABLE IF NOT EXISTS cider_series (
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    series_key TEXT NOT NULL,
    observation_json TEXT NOT NULL,
    updated_at INTEGER NOT NULL,
    PRIMARY KEY(node_id, series_key)
);
CREATE INDEX IF NOT EXISTS cider_series_age ON cider_series(updated_at);
PRAGMA user_version = 2;
