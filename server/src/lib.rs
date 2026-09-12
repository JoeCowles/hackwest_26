pub mod api;
pub mod config;
pub mod error;
pub mod model;
pub mod read_api;
pub mod store;
pub mod workers;

pub const HEARTBEAT_SECONDS: u64 = 5;
pub const MAX_BODY_BYTES: usize = 1_048_576;
pub const MAX_SAMPLES: usize = 2048;
pub const MAX_EVENTS: usize = 128;
pub const MAX_OBJECTS: usize = 2048;

#[cfg(test)]
mod tests;
