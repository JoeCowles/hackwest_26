CREATE TABLE IF NOT EXISTS enrollment_tokens (
    token_hash TEXT PRIMARY KEY, expires_at INTEGER NOT NULL,
    used_at INTEGER
);
CREATE TABLE IF NOT EXISTS nodes (
    node_id TEXT PRIMARY KEY, name TEXT NOT NULL, agent_json TEXT NOT NULL,
    credential_hash TEXT NOT NULL, enrolled_at INTEGER NOT NULL,
    last_seen_at INTEGER, goodbye_at INTEGER, revoked_at INTEGER,
    boot_id TEXT, boot_observed_at INTEGER NOT NULL DEFAULT 0,
    inventory_generation INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS inventory_generations (
    node_id TEXT NOT NULL REFERENCES nodes(node_id), generation INTEGER NOT NULL,
    fingerprint TEXT NOT NULL, inventory_json TEXT NOT NULL,
    received_at INTEGER NOT NULL, change_id INTEGER NOT NULL,
    PRIMARY KEY(node_id, generation)
);
CREATE TABLE IF NOT EXISTS objects (
    object_id TEXT PRIMARY KEY, node_id TEXT NOT NULL REFERENCES nodes(node_id),
    local_id TEXT NOT NULL, kind TEXT NOT NULL, parents_json TEXT NOT NULL,
    properties_json TEXT NOT NULL, generation INTEGER NOT NULL,
    active INTEGER NOT NULL DEFAULT 1, UNIQUE(node_id, local_id)
);
CREATE INDEX IF NOT EXISTS objects_node ON objects(node_id, active);
CREATE TABLE IF NOT EXISTS batches (
    node_id TEXT NOT NULL REFERENCES nodes(node_id), boot_id TEXT NOT NULL,
    sequence TEXT NOT NULL, fingerprint TEXT NOT NULL, raw_json TEXT,
    received_at INTEGER NOT NULL, sample_count INTEGER NOT NULL,
    event_count INTEGER NOT NULL, change_id INTEGER NOT NULL,
    PRIMARY KEY(node_id, boot_id, sequence)
);
CREATE TABLE IF NOT EXISTS metric_samples (
    sample_id INTEGER PRIMARY KEY AUTOINCREMENT,
    node_id TEXT NOT NULL REFERENCES nodes(node_id), object_id TEXT NOT NULL REFERENCES objects(object_id),
    boot_id TEXT NOT NULL, generation INTEGER NOT NULL,
    name TEXT NOT NULL, kind TEXT NOT NULL, unit TEXT NOT NULL,
    state TEXT NOT NULL, source TEXT NOT NULL, scope TEXT NOT NULL,
    labels_json TEXT NOT NULL, value_json TEXT NOT NULL, numeric_value REAL,
    observed_at INTEGER NOT NULL, received_at INTEGER NOT NULL,
    rate_per_second REAL, derivation_state TEXT NOT NULL,
    UNIQUE(object_id, boot_id, generation, name, source, scope, labels_json, observed_at)
);
CREATE INDEX IF NOT EXISTS samples_series ON metric_samples(object_id, name, observed_at DESC);
CREATE INDEX IF NOT EXISTS samples_retention ON metric_samples(observed_at);
CREATE TABLE IF NOT EXISTS latest_samples (
    object_id TEXT NOT NULL, name TEXT NOT NULL, source TEXT NOT NULL,
    scope TEXT NOT NULL, labels_json TEXT NOT NULL, sample_json TEXT NOT NULL,
    observed_at INTEGER NOT NULL, received_at INTEGER NOT NULL,
    PRIMARY KEY(object_id, name, source, scope, labels_json)
);
CREATE TABLE IF NOT EXISTS events (
    event_id INTEGER PRIMARY KEY AUTOINCREMENT, node_id TEXT NOT NULL,
    object_id TEXT, event_json TEXT NOT NULL,
    occurred_at INTEGER NOT NULL, received_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS events_time ON events(occurred_at);
CREATE TABLE IF NOT EXISTS change_log (
    change_id INTEGER PRIMARY KEY AUTOINCREMENT, event_type TEXT NOT NULL,
    node_id TEXT, payload_json TEXT NOT NULL, committed_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS request_replays (
    credential_hash TEXT NOT NULL, request_id TEXT NOT NULL,
    expires_at INTEGER NOT NULL, PRIMARY KEY(credential_hash, request_id)
);
CREATE TABLE IF NOT EXISTS metric_rollups (
    resolution_seconds INTEGER NOT NULL, bucket_start INTEGER NOT NULL,
    object_id TEXT NOT NULL, boot_id TEXT NOT NULL, generation INTEGER NOT NULL,
    name TEXT NOT NULL, kind TEXT NOT NULL, unit TEXT NOT NULL,
    source TEXT NOT NULL, scope TEXT NOT NULL, labels_json TEXT NOT NULL,
    min REAL, max REAL, mean REAL, sample_count INTEGER NOT NULL,
    valid_sample_count INTEGER NOT NULL, state_counts_json TEXT NOT NULL,
    PRIMARY KEY(resolution_seconds, bucket_start, object_id, boot_id, generation,
                name, source, scope, labels_json)
);
PRAGMA user_version = 1;
