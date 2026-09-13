-- Fixed-threshold filesystem and NFS rules run in the server heartbeat transaction.
CREATE TABLE security_rule_sources (
    source_id TEXT PRIMARY KEY,
    node_id TEXT NOT NULL REFERENCES nodes(node_id),
    object_id TEXT NOT NULL,
    resource_id TEXT NOT NULL,
    collector TEXT NOT NULL,
    scope TEXT NOT NULL,
    active INTEGER NOT NULL CHECK(active IN (0,1)),
    state_json TEXT NOT NULL CHECK(length(CAST(state_json AS BLOB)) <= 262144),
    summary_json TEXT NOT NULL CHECK(length(CAST(summary_json AS BLOB)) <= 131072),
    updated_at INTEGER NOT NULL
);
CREATE INDEX security_rule_sources_node ON security_rule_sources(node_id,active);

CREATE TABLE security_rule_findings (
    finding_id TEXT PRIMARY KEY,
    source_id TEXT NOT NULL REFERENCES security_rule_sources(source_id),
    node_id TEXT NOT NULL,
    object_id TEXT NOT NULL,
    rule_id TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('open','resolved','interrupted')),
    severity TEXT NOT NULL CHECK(severity IN ('warning','critical')),
    first_seen_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL,
    ended_at INTEGER,
    finding_json TEXT NOT NULL CHECK(length(CAST(finding_json AS BLOB)) <= 131072)
);
CREATE INDEX security_rule_findings_node_time ON security_rule_findings(node_id,first_seen_at,finding_id);
CREATE INDEX security_rule_findings_status ON security_rule_findings(status,updated_at);

PRAGMA user_version = 6;
