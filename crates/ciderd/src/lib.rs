//! Storage telemetry domain types, collectors, and supervised daemon runtime.
pub mod clock;
pub mod collectors;
pub mod command;
pub mod config;
pub mod device_snapshot;
pub mod heartbeat;
pub mod hardware;
pub mod identity;
pub mod model;
pub mod cider;
pub mod platform;
pub mod runtime;
mod scheduler;
pub mod state;
pub mod topology;

pub mod quota;
