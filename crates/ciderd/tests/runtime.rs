use ciderd::{
    clock::Clock,
    config::Config,
    runtime::{initial_heartbeat, snapshot_with_worker},
};
use std::time::Duration;

#[test]
fn initial_heartbeat_is_a_live_host_with_no_fabricated_storage_metrics() {
    let heartbeat = initial_heartbeat(
        "node-test",
        1,
        "session-test",
        "boot-test",
        "test-os",
        "test-build",
        "test-target",
        15,
    );
    assert_eq!(heartbeat.resources.len(), 1);
    assert_eq!(heartbeat.resources[0].resource_type, "host");
    assert!(heartbeat.collections.is_empty());
    heartbeat.validate().unwrap();
}

#[test]
fn monotonic_samples_keep_a_stable_clock_and_increasing_bounds() {
    let clock = Clock::new("test-session");
    let sample = clock.start();
    let (id, ns) = clock.read();
    assert_eq!(id, sample.clock_id);
    assert!(ns >= sample.started_monotonic_ns);
    assert!(sample.finished_ns() >= sample.started_monotonic_ns);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn offline_snapshot_collects_real_mounts_and_stops_within_its_budget() {
    let config = Config::parse(include_str!("../examples/ciderd.toml")).unwrap();
    let heartbeat = tokio::time::timeout(
        Duration::from_secs(10),
        snapshot_with_worker(
            config,
            Duration::from_secs(3),
            env!("CARGO_BIN_EXE_ciderd").into(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    heartbeat.validate().unwrap();
    assert!(heartbeat
        .resources
        .iter()
        .any(|r| r.resource_type == "mount"));
    assert!(heartbeat
        .collections
        .iter()
        .flat_map(|c| &c.metrics)
        .any(|m| m.name == "storage.filesystem.total_bytes"));
}
