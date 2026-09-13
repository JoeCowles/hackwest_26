-- Independent reliability policy state, committed with the native receipt.
CREATE TABLE reliability_sources (
    source_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    object_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    collector TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    support_state TEXT NOT NULL CHECK(support_state IN ('supported','capacity_limited')),
    state_json TEXT NOT NULL CHECK(length(CAST(state_json AS BLOB)) <= 524288),
    summary_json TEXT NOT NULL,
    boot_id TEXT NOT NULL,
    agent_generation TEXT NOT NULL,
    agent_session_id TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
CREATE INDEX reliability_sources_node ON reliability_sources(node_id,active);
CREATE INDEX reliability_sources_admission ON reliability_sources(active,support_state);
CREATE TABLE reliability_findings (
    finding_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES reliability_sources(source_id),
    node_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('open','resolved','interrupted')),
    first_seen_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER,
    finding_json TEXT NOT NULL
);
CREATE INDEX reliability_findings_node_time ON reliability_findings(node_id,first_seen_at,finding_id);
CREATE INDEX reliability_findings_retention ON reliability_findings(status,ended_at);
PRAGMA user_version = 4;
