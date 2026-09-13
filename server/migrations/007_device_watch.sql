CREATE TABLE device_watch_nodes (
 node_id TEXT PRIMARY KEY REFERENCES nodes(node_id),
 state_json TEXT NOT NULL CHECK(length(CAST(state_json AS BLOB))<=8192),
 updated_at INTEGER NOT NULL
);
CREATE TABLE device_watch_devices (
 watch_id TEXT PRIMARY KEY,
 node_id TEXT NOT NULL REFERENCES nodes(node_id),
 state_json TEXT NOT NULL CHECK(length(CAST(state_json AS BLOB))<=16384),
 updated_at INTEGER NOT NULL
);
CREATE INDEX device_watch_node ON device_watch_devices(node_id);
PRAGMA user_version = 7;
