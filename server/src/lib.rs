pub mod api;
pub mod attention;
pub mod notifications;
pub mod history;
pub mod diagnostics;
pub mod quota;
pub mod cider_api;
// Share only the collector's pure wire model and validator, not its native workers.
#[path = "../../crates/ciderd/src/model.rs"]
pub mod cider_wire;
#[path = "../../crates/ciderd/src/device_snapshot.rs"]
pub mod device_snapshot;
pub mod device_watch;
pub mod config;
pub mod detection;
pub mod detection_store;
pub mod disk_view;
pub mod error;
pub mod hardware;
pub mod model;
pub mod read_api;
pub mod reliability;
pub mod reliability_store;
pub mod reliability_view;
pub mod security_rules;
pub mod store;
pub mod workers;

pub const HEARTBEAT_SECONDS: u64 = 5;
pub const MAX_BODY_BYTES: usize = 1_048_576;
pub const MAX_SAMPLES: usize = 2048;
pub const MAX_EVENTS: usize = 128;
pub const MAX_OBJECTS: usize = 2048;

#[cfg(test)]
mod tests;
