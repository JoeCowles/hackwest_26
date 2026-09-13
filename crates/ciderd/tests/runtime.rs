use ciderd::{
    clock::Clock,
    config::Config,
    runtime::{initial_heartbeat, snapshot_with_worker},
};
use std::time::Duration;

// Offline snapshots intentionally hold one process-wide lease.
#[cfg(target_os = "macos")]
static SNAPSHOT_TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

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
    let _lease = SNAPSHOT_TEST_LOCK.lock().await;
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
    assert!(
        heartbeat
            .resources
            .iter()
            .any(|r| r.resource_type == "mount")
    );
    assert!(
        heartbeat
            .collections
            .iter()
            .flat_map(|c| &c.metrics)
            .any(|m| m.name == "storage.filesystem.total_bytes")
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn configured_quota_is_observed_without_a_local_nfs_mount() {
    let _lease = SNAPSHOT_TEST_LOCK.lock().await;
    use std::net::UdpSocket;
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let port = socket.local_addr().unwrap().port();
    let peer = std::thread::spawn(move || {
        let mut bytes = [0u8; 2048];
        let (_, addr) = socket.recv_from(&mut bytes).unwrap();
        let xid = u32::from_be_bytes(bytes[..4].try_into().unwrap());
        let response: Vec<u8> = [
            xid, 1, 0, 0, 0, 0, 1, 1024, 1, 2048, 1024, 64, 20, 10, 2, 0, 0,
        ]
        .into_iter()
        .flat_map(u32::to_be_bytes)
        .collect();
        socket.send_to(&response, addr).unwrap();
    });
    let text = format!(
        "{}\n[[nfs_quotas.targets]]\nserver='127.0.0.1'\nexport_path='/fixture'\nuid=501\nport={port}\n",
        include_str!("../examples/ciderd.toml")
    );
    let config = Config::parse(&text).unwrap();
    let heartbeat = snapshot_with_worker(
        config,
        Duration::from_secs(3),
        env!("CARGO_BIN_EXE_ciderd").into(),
    )
    .await
    .unwrap();
    peer.join().unwrap();
    heartbeat.validate().unwrap();
    let quota = heartbeat
        .resources
        .iter()
        .find(|r| r.resource_type == "nfs_user_quota")
        .unwrap();
    let sample = heartbeat
        .collections
        .iter()
        .find(|c| c.resource_id == quota.resource_id && c.collector == "nfs.rquota")
        .unwrap();
    assert_eq!(sample.status, "ok");
    assert!(
        sample
            .metrics
            .iter()
            .any(|m| m.name == "storage.nfs.quota.used_bytes"
                && m.value == Some(serde_json::json!("65536")))
    );
    let host = heartbeat
        .resources
        .iter()
        .find(|r| r.resource_type == "host")
        .unwrap();
    assert_eq!(
        host.attributes["diagnostics_configuration"]["smart_enabled"],
        false
    );
    assert_eq!(
        host.attributes["diagnostics_configuration"]["configured_quota_target_count"],
        1
    );
}
