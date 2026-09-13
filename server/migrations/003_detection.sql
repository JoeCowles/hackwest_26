-- Detection state participates in the same transaction as immutable native receipts.
CREATE TABLE detection_sources (
    source_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    object_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    direction TEXT NOT NULL CHECK(direction IN ('read','write')),
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    support_state TEXT NOT NULL CHECK(support_state IN ('supported','capacity_limited')),
    state_json TEXT NOT NULL CHECK(length(CAST(state_json AS BLOB)) <= 524288),
    summary_json TEXT NOT NULL,
    support_reason TEXT,
    boot_id TEXT NOT NULL,
    agent_generation TEXT NOT NULL,
    agent_session_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX detection_sources_node ON detection_sources(node_id,active);
CREATE INDEX detection_sources_admission ON detection_sources(active,support_state);
CREATE TABLE detection_findings (
    finding_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES detection_sources(source_id),
    node_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('open','resolved','interrupted')),
    first_seen_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER,
    finding_json TEXT NOT NULL
);
CREATE INDEX detection_findings_node_time ON detection_findings(node_id,first_seen_at,finding_id);
CREATE INDEX detection_findings_retention ON detection_findings(status,ended_at);
CREATE TABLE detection_rebaseline_audit (
    id INTEGER PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES detection_sources(source_id),
    actor_hash TEXT NOT NULL,
    reason TEXT NOT NULL,
    previous_revision TEXT NOT NULL,
    new_revision TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
PRAGMA user_version = 3;
