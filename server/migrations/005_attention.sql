CREATE TABLE attention_episodes (
 id TEXT PRIMARY KEY, source_key TEXT NOT NULL, kind TEXT NOT NULL,
 node_id TEXT NOT NULL, object_id TEXT, status TEXT NOT NULL,
 severity TEXT NOT NULL, observation_state TEXT NOT NULL, summary TEXT NOT NULL,
 evidence_json TEXT NOT NULL, revision TEXT NOT NULL, first_seen_at INTEGER NOT NULL,
 updated_at INTEGER NOT NULL, resolved_at INTEGER, ack_json TEXT, notification_intent_json TEXT
);
CREATE UNIQUE INDEX attention_one_open ON attention_episodes(source_key) WHERE status='open';
CREATE INDEX attention_order ON attention_episodes(status,updated_at,id);
CREATE TABLE notification_settings (
 id INTEGER PRIMARY KEY CHECK(id=1), enabled INTEGER NOT NULL DEFAULT 0,
 sender TEXT NOT NULL DEFAULT '', recipient TEXT NOT NULL DEFAULT '',
 revision TEXT NOT NULL, updated_at INTEGER NOT NULL DEFAULT 0, actor_hash TEXT,
 credential_state TEXT NOT NULL DEFAULT 'not_loaded', worker_seen_at INTEGER,
 admission_limited INTEGER NOT NULL DEFAULT 0,
 last_reconciled_at INTEGER, reconcile_attempted_at INTEGER, reconcile_error TEXT, last_send_claim_at INTEGER
);
INSERT INTO notification_settings(id,revision) VALUES(1,'initial');
CREATE TABLE notification_outbox (
 id TEXT PRIMARY KEY, episode_id TEXT NOT NULL REFERENCES attention_episodes(id),
 transition_key TEXT NOT NULL UNIQUE, state TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0,
 next_attempt_at INTEGER NOT NULL, created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL,
 message_sid TEXT, provider_status TEXT, reason TEXT, settings_revision TEXT
);
CREATE INDEX notification_due ON notification_outbox(state,next_attempt_at);
PRAGMA user_version = 5;
