use ciderd::{
    collectors::*,
    platform::{parse_worker_request, WorkerRequest},
};
use serde_json::{json, Value};

const NODE: &str = "test-node";
const BOOT: &str = "test-boot";
fn metric<'a>(result: &'a Collected, name: &str) -> &'a ciderd::model::Metric {
    result
        .samples
        .iter()
        .flat_map(|s| &s.metrics)
        .find(|m| m.name == name)
        .unwrap()
}

fn ata_smart(rows: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "smartctl": {"version": [7, 5], "exit_status": 0},
        "serial_number": "PRIVATE-ATA-SERIAL",
        "model_name": "Example ATA Device",
        "device": {"protocol": "ATA"},
        "ata_smart_attributes": {"revision": 16, "table": rows}
    }))
    .unwrap()
}

#[test]
fn smart_ata_sector_attributes_emit_exact_nonmonotonic_gauges() {
    let result = parse_smart(
        include_bytes!("fixtures/smart-ata.json"),
        0,
        "device",
        "epoch",
    )
    .unwrap();
    for (name, value) in [
        ("storage.ata.reallocated_sectors", "9007199254740993"),
        ("storage.ata.current_pending_sectors", "2"),
        ("storage.ata.offline_uncorrectable_sectors", "3"),
    ] {
        let reading = metric(&result, name);
        assert_eq!(reading.kind, "gauge");
        assert_eq!(reading.unit, "sectors");
        assert_eq!(reading.value, Some(json!(value)));
        assert_eq!(reading.counter_epoch, None);
        assert_eq!(
            reading.extensions.as_ref().unwrap()["device_identity_confidence"],
            "reported_serial"
        );
        assert_eq!(
            reading.extensions.as_ref().unwrap()["device_identity"]
                .as_str()
                .unwrap()
                .len(),
            36
        );
    }
}

#[test]
fn smart_ata_absent_vendor_packed_and_duplicate_rows_never_become_counts() {
    let absent = parse_smart(&ata_smart(json!([])), 0, "device", "epoch").unwrap();
    assert!(!absent.samples[0]
        .metrics
        .iter()
        .any(|reading| reading.name.starts_with("storage.ata.")));

    let packed = parse_smart(
        &ata_smart(json!([
            {
                "id": 197,
                "name": "Current_Pending_Sector",
                "value": 100,
                "worst": 100,
                "thresh": 0,
                "raw": {"value": 4294967298_u64, "string": "2/1"}
            },
            {
                "id": 198,
                "name": "Vendor_Offline_Value",
                "value": 100,
                "worst": 100,
                "thresh": 0,
                "raw": {"value": 7, "string": "7"}
            }
        ])),
        0,
        "device",
        "epoch",
    )
    .unwrap();
    assert!(!packed.samples[0]
        .metrics
        .iter()
        .any(|reading| reading.name.starts_with("storage.ata.")));

    let duplicate = ata_smart(json!([
        {
            "id": 5,
            "name": "Reallocated_Sector_Ct",
            "value": 100,
            "worst": 100,
            "thresh": 10,
            "raw": {"value": 1, "string": "1"}
        },
        {
            "id": 5,
            "name": "Reallocated_Sector_Ct",
            "value": 100,
            "worst": 100,
            "thresh": 10,
            "raw": {"value": 2, "string": "2"}
        }
    ]));
    let duplicate = parse_smart(&duplicate, 0, "device", "epoch").unwrap();
    assert_eq!(duplicate.samples[0].status, "partial");
    assert!(!duplicate.samples[0]
        .metrics
        .iter()
        .any(|reading| reading.name.starts_with("storage.ata.")));
}

#[test]
fn ambiguous_ata_table_does_not_suppress_independent_overall_health() {
    let mut value: Value = serde_json::from_slice(&ata_smart(json!([
        {
            "id": 197,
            "name": "Current_Pending_Sector",
            "value": 100,
            "worst": 100,
            "thresh": 0,
            "raw": {"value": 2, "string": "2"}
        },
        {
            "id": 197,
            "name": "Current_Pending_Sector",
            "value": 100,
            "worst": 100,
            "thresh": 0,
            "raw": {"value": 3, "string": "3"}
        }
    ])))
    .unwrap();
    value["smart_status"] = json!({"passed": false});

    let result = parse_smart(&serde_json::to_vec(&value).unwrap(), 0, "device", "epoch").unwrap();
    assert_eq!(result.samples[0].status, "partial");
    assert_eq!(
        metric(&result, "storage.media.smart_passed").value,
        Some(false.into())
    );
    assert!(!result.samples[0]
        .metrics
        .iter()
        .any(|reading| reading.name.starts_with("storage.ata.")));
}

#[test]
fn ata_exact_companion_must_not_contradict_an_exact_small_numeric_value() {
    let result = parse_smart(
        &ata_smart(json!([{
            "id": 197,
            "name": "Current_Pending_Sector",
            "value": 100,
            "worst": 100,
            "thresh": 0,
            "raw": {"value": 9, "value_s": "2", "string": "2"}
        }])),
        0,
        "device",
        "epoch",
    )
    .unwrap();
    assert!(!result.samples[0]
        .metrics
        .iter()
        .any(|reading| reading.name == "storage.ata.current_pending_sectors"));
}

#[test]
fn smart_health_exit_bits_keep_exact_counters_and_celsius() {
    let result = parse_smart(
        include_bytes!("fixtures/smart-nvme.json"),
        8,
        "device",
        "epoch",
    )
    .unwrap();
    assert_eq!(result.samples[0].status, "ok");
    assert_eq!(result.samples[0].exit_code, Some(8));
    assert_eq!(
        metric(&result, "storage.nvme.data_units_read_total").value,
        Some(u128::MAX.to_string().into())
    );
    assert_eq!(
        metric(&result, "storage.nvme.data_units_written_total").value,
        Some("18446744073709551617".into())
    );
    assert_eq!(
        metric(&result, "storage.media.temperature_celsius").value,
        Some(json!(-2))
    );
    assert_eq!(
        metric(&result, "storage.nvme.endurance_used_percent").value,
        Some("135".into())
    );
    assert_eq!(
        metric(&result, "storage.media.smart_passed").value,
        Some(false.into())
    );
    assert!(!result.samples[0]
        .metrics
        .iter()
        .any(|m| m.name.contains("power_cycles")));
    for reading in &result.samples[0].metrics {
        let extensions = reading.extensions.as_ref().unwrap();
        assert_eq!(extensions["smartctl_exit_status"], 8);
        assert_eq!(extensions["smartctl_exit_status_class"], "device_report");
        assert_eq!(
            extensions["device_identity_confidence"],
            "caller_epoch_only"
        );
        assert_eq!(extensions["device_identity"].as_str().unwrap().len(), 36);
    }
}

#[test]
fn smart_tool_exit_bits_remain_acquisition_errors_with_parsed_readings() {
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/smart-nvme.json")).unwrap();
    value["smartctl"]["exit_status"] = json!(1);
    let result = parse_smart(&serde_json::to_vec(&value).unwrap(), 1, "device", "epoch").unwrap();
    assert_eq!(result.samples[0].status, "partial");
    assert_eq!(result.samples[0].exit_code, Some(1));
    assert_eq!(
        metric(&result, "storage.media.smart_passed")
            .extensions
            .as_ref()
            .unwrap()["smartctl_exit_status_class"],
        "tool_error"
    );
    assert_eq!(
        metric(&result, "storage.media.smart_passed").value,
        Some(false.into())
    );
}

#[test]
fn smart_locator_context_rejects_a_different_reported_device() {
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/smart-nvme.json")).unwrap();
    value["device"]["name"] = json!("/dev/disk0");
    parse_smart_with_locator(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "epoch",
        "/dev/disk0",
    )
    .unwrap();
    assert!(parse_smart_with_locator(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "epoch",
        "/dev/disk1",
    )
    .is_err());
    value["device"]["name"] = json!("/dev/disk0/../disk1");
    assert!(parse_smart_with_locator(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "epoch",
        "/dev/disk0",
    )
    .is_err());
}

#[test]
fn smart_namespace_scope_has_its_own_resource_and_epoch() {
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/smart-nvme.json")).unwrap();
    value["nvme_smart_health_information_log"]["nsid"] = json!(2);
    let result = parse_smart(&serde_json::to_vec(&value).unwrap(), 8, "device", "epoch").unwrap();
    assert_eq!(result.resources.len(), 1);
    assert_eq!(result.resources[0].resource_type, "media");
    assert_eq!(result.resources[0].attributes["namespace_id"], "2");
    assert_ne!(result.samples[0].resource_id, "device");
    assert_eq!(result.relationships[0].from_resource_id, "device");
    assert_ne!(
        metric(&result, "storage.nvme.data_units_read_total")
            .counter_epoch
            .as_deref(),
        Some("epoch")
    );
    value["nvme_smart_health_information_log"]
        .as_object_mut()
        .unwrap()
        .remove("nsid");
    let unknown = parse_smart(&serde_json::to_vec(&value).unwrap(), 8, "device", "epoch").unwrap();
    assert_eq!(unknown.samples[0].status, "partial");
    assert!(unknown.samples[0].metrics.is_empty());
}

#[test]
fn smart_reported_identity_resets_every_counter_without_exposing_serial() {
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/smart-nvme.json")).unwrap();
    value["model_name"] = json!("Example NVMe Model");
    for nsid in [-1, 2] {
        value["nvme_smart_health_information_log"]["nsid"] = json!(nsid);
        value["serial_number"] = json!("PRIVATE-SERIAL-ALPHA");
        let first = parse_smart(
            &serde_json::to_vec(&value).unwrap(),
            8,
            "same-bsd-resource",
            "same-caller-epoch",
        )
        .unwrap();
        let repeated = parse_smart(
            &serde_json::to_vec(&value).unwrap(),
            8,
            "same-bsd-resource",
            "same-caller-epoch",
        )
        .unwrap();
        value["serial_number"] = json!("PRIVATE-SERIAL-BETA");
        let replaced = parse_smart(
            &serde_json::to_vec(&value).unwrap(),
            8,
            "same-bsd-resource",
            "same-caller-epoch",
        )
        .unwrap();
        for reading in first.samples[0]
            .metrics
            .iter()
            .filter(|m| m.kind == "counter")
        {
            assert_eq!(
                reading.counter_epoch,
                metric(&repeated, &reading.name).counter_epoch
            );
            assert_ne!(
                reading.counter_epoch,
                metric(&replaced, &reading.name).counter_epoch
            );
            assert_eq!(reading.counter_epoch.as_ref().unwrap().len(), 36);
        }
        for result in [&first, &repeated, &replaced] {
            let serialized = serde_json::to_string(&result.samples[0].metrics).unwrap();
            assert!(!serialized.contains("PRIVATE-SERIAL"));
            assert!(!serialized.contains("Example NVMe Model"));
            assert!(result.samples[0]
                .metrics
                .iter()
                .all(|m| m.attributes.is_empty()));
        }
        assert_eq!(
            metric(&first, "storage.media.smart_passed").extensions,
            metric(&repeated, "storage.media.smart_passed").extensions
        );
        assert_ne!(
            metric(&first, "storage.media.smart_passed")
                .extensions
                .as_ref()
                .unwrap()["device_identity"],
            metric(&replaced, "storage.media.smart_passed")
                .extensions
                .as_ref()
                .unwrap()["device_identity"]
        );
    }
}

#[test]
fn smart_wwn_precedes_serial_and_missing_identity_keeps_caller_epoch() {
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/smart-nvme.json")).unwrap();
    let absent = parse_smart(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "caller-epoch",
    )
    .unwrap();
    let counter = metric(&absent, "storage.nvme.data_units_read_total");
    assert_eq!(counter.counter_epoch.as_deref(), Some("caller-epoch"));
    assert_eq!(
        counter.extensions.as_ref().unwrap()["counter_identity"],
        "caller_epoch_only"
    );
    assert_eq!(
        counter.extensions.as_ref().unwrap()["device_identity_confidence"],
        "caller_epoch_only"
    );
    assert_eq!(
        counter.extensions.as_ref().unwrap()["device_identity"]
            .as_str()
            .unwrap()
            .len(),
        36
    );
    value["wwn"] = json!({"naa":5,"oui":12345,"id":67890});
    value["serial_number"] = json!("serial-a");
    let first = parse_smart(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "caller-epoch",
    )
    .unwrap();
    value["serial_number"] = json!("serial-b");
    let same_wwn = parse_smart(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "caller-epoch",
    )
    .unwrap();
    assert_eq!(
        metric(&first, "storage.nvme.data_units_read_total").counter_epoch,
        metric(&same_wwn, "storage.nvme.data_units_read_total").counter_epoch
    );
    value["wwn"]["id"] = json!(67891);
    let new_wwn = parse_smart(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "caller-epoch",
    )
    .unwrap();
    assert_ne!(
        metric(&first, "storage.nvme.data_units_read_total").counter_epoch,
        metric(&new_wwn, "storage.nvme.data_units_read_total").counter_epoch
    );
    value.as_object_mut().unwrap().remove("wwn");
    value["serial_number"] = json!("x".repeat(257));
    assert!(parse_smart(
        &serde_json::to_vec(&value).unwrap(),
        8,
        "device",
        "caller-epoch"
    )
    .is_err());
}

#[test]
fn smart_missing_fields_standby_and_malformed_numbers_are_distinct() {
    let absent = br#"{"smartctl":{"version":[7,5],"exit_status":0}}"#;
    let result = parse_smart(absent, 0, "device", "epoch").unwrap();
    assert_eq!(result.samples[0].status, "unsupported");
    assert!(result.samples[0].metrics.is_empty());
    let standby = br#"{"smartctl":{"version":[7,5],"exit_status":2},"power_mode":"STANDBY"}"#;
    assert_eq!(
        parse_smart(standby, 2, "device", "epoch").unwrap().samples[0].status,
        "skipped"
    );
    assert!(parse_smart(absent, 2, "device", "epoch").is_err());
    let fractional = br#"{"smartctl":{"version":[7,5]},"nvme_smart_health_information_log":{"nsid":-1,"data_units_read":1.5}}"#;
    assert!(parse_smart(fractional, 0, "device", "epoch").is_err());
    let duplicate =
        br#"{"smartctl":{"version":[7,5]},"smart_status":{"passed":true,"passed":false}}"#;
    assert!(parse_smart(duplicate, 0, "device", "epoch").is_err());
}

#[test]
fn physical_and_apfs_topology_keep_capacity_pools_separate() {
    let physical = parse_inventory(
        include_bytes!("fixtures/diskutil-physical.plist"),
        NODE,
        BOOT,
    )
    .unwrap();
    let devices: Vec<_> = physical
        .resources
        .iter()
        .filter(|r| r.resource_type == "physical_device")
        .collect();
    assert_eq!(devices.len(), 2);
    assert_ne!(devices[0].resource_id, devices[1].resource_id);
    assert!(devices
        .iter()
        .all(|r| r.attributes["smart_eligible"] == true));
    let apfs = parse_apfs(include_bytes!("fixtures/diskutil-apfs.plist"), NODE, BOOT).unwrap();
    assert_eq!(
        apfs.resources
            .iter()
            .filter(|r| r.resource_type == "apfs_container")
            .count(),
        1
    );
    assert_eq!(
        apfs.resources
            .iter()
            .filter(|r| r.resource_type == "filesystem")
            .count(),
        2
    );
    assert_eq!(
        metric(&apfs, "storage.apfs.container_free_bytes").value,
        Some("100000".into())
    );
    assert!(!apfs
        .samples
        .iter()
        .flat_map(|s| &s.metrics)
        .any(|m| m.name == "storage.filesystem.total_bytes"));
    let physical_store = physical
        .resources
        .iter()
        .find(|r| r.attributes.get("bsd_name") == Some(&json!("disk0s2")))
        .unwrap();
    assert!(apfs
        .resources
        .iter()
        .any(|r| r.resource_id == physical_store.resource_id));
    let volume = apfs
        .resources
        .iter()
        .find(|r| r.attributes.get("bsd_name") == Some(&json!("disk2s2")))
        .unwrap();
    let sample = apfs
        .samples
        .iter()
        .find(|s| s.resource_id == volume.resource_id)
        .unwrap();
    assert!(!sample.metrics.iter().any(|m| m.name.contains("quota")));
}

#[test]
fn plist_malformed_shapes_and_duplicate_identity_are_rejected() {
    assert!(parse_inventory(b"{}", NODE, BOOT).is_err());
    assert!(parse_apfs(
        b"<plist><dict><key>Containers</key><string>wrong</string></dict></plist>",
        NODE,
        BOOT
    )
    .is_err());
    let duplicate = b"<plist><dict><key>AllDisksAndPartitions</key><array/><key>AllDisksAndPartitions</key><array/></dict></plist>";
    assert!(parse_inventory(duplicate, NODE, BOOT).is_err());
    let deep = format!(
        "<plist>{}<array/>{}</plist>",
        "<array>".repeat(70),
        "</array>".repeat(70)
    );
    assert!(parse_iokit(deep.as_bytes(), NODE, BOOT).is_err());
}

#[test]
fn iokit_absent_properties_do_not_become_zero_and_epoch_changes_with_boot() {
    let one = parse_iokit(include_bytes!("fixtures/iokit.plist"), NODE, BOOT).unwrap();
    assert_eq!(one.samples[0].metrics.len(), 4);
    assert_eq!(
        metric(&one, "storage.device.read_bytes_total").value,
        Some(u64::MAX.to_string().into())
    );
    assert!(!one.samples[0]
        .metrics
        .iter()
        .any(|m| m.name.contains("errors")));
    let two = parse_iokit(include_bytes!("fixtures/iokit.plist"), NODE, "next-boot").unwrap();
    assert_ne!(
        one.samples[0].metrics[0].counter_epoch,
        two.samples[0].metrics[0].counter_epoch
    );
    assert_eq!(one.resources[0].attributes["smart_eligible"], false);
}

#[test]
fn cached_capacity_is_exact_and_mount_identity_tracks_source() {
    let bytes = include_bytes!("fixtures/mounts.json");
    let result = parse_mounts(bytes, NODE, BOOT).unwrap();
    let total = metric(&result, "storage.filesystem.total_bytes");
    assert_eq!(
        total.value,
        Some((9007199254740993u128 * 4096).to_string().into())
    );
    assert_eq!(total.freshness.as_deref(), Some("cached"));
    assert_eq!(result.samples.len(), 1);
    assert_eq!(result.samples[0].collector, "mount.inventory");
    assert!(result.samples[0]
        .metrics
        .iter()
        .any(|m| m.name == "storage.mount.read_only"));
    assert!(!result
        .samples
        .iter()
        .any(|s| s.collector == "filesystem.capacity"));
    assert_eq!(result.resources[0].attributes["fsid"], json!([12, 34]));
    let mut changed: Value = serde_json::from_slice(bytes).unwrap();
    changed["mounts"][0]["fsid"] = json!([56, 78]);
    let next = parse_mounts(&serde_json::to_vec(&changed).unwrap(), NODE, BOOT).unwrap();
    assert_ne!(
        result.resources[0].resource_id,
        next.resources[0].resource_id
    );
    let gone = parse_capacity(br#"{"status":"gone"}"#, "mount").unwrap();
    assert_eq!(gone.samples[0].collector, "filesystem.capacity");
    assert_eq!(gone.samples[0].status, "gone");
    assert!(gone.samples[0].metrics.is_empty());
    assert!(parse_capacity(
        br#"{"status":"ok","capacity":{"block_size":"4096","blocks":"1","blocks_free":"2"}}"#,
        "mount"
    )
    .is_err());
}

#[test]
fn nfs_host_client_families_and_cache_names_are_distinct() {
    let result = parse_nfs(include_bytes!("fixtures/nfsstat-client.json"), NODE, BOOT).unwrap();
    assert_eq!(result.resources[0].resource_type, "nfs_client");
    assert_eq!(result.samples[0].status, "ok");
    let callbacks: Vec<_> = result.samples[0]
        .metrics
        .iter()
        .filter(|m| m.attributes.get("family") == Some(&json!("v4_callback")))
        .collect();
    assert_eq!(callbacks.len(), 2);
    assert!(callbacks
        .iter()
        .all(|m| m.attributes["operation"] != "cb_compound"));
    assert!(result.samples[0]
        .metrics
        .iter()
        .any(|m| m.attributes.get("cache") == Some(&json!("biorl"))));
    let mut value: Value =
        serde_json::from_slice(include_bytes!("fixtures/nfsstat-client.json")).unwrap();
    value["Client Info"]["RPC Info"]
        .as_object_mut()
        .unwrap()
        .remove("Retries");
    let missing = parse_nfs(&serde_json::to_vec(&value).unwrap(), NODE, BOOT).unwrap();
    assert_eq!(missing.samples[0].status, "partial");
    assert!(!missing.samples[0]
        .metrics
        .iter()
        .any(|m| m.name == "storage.nfs.client.rpc_retries_total"));
    value["Client Info"]["NFSv3 RPC Counts"]["Read"] = json!("-1");
    assert!(parse_nfs(&serde_json::to_vec(&value).unwrap(), NODE, BOOT).is_err());
    assert!(parse_nfs(br#"{"Server Info":{}}"#, NODE, BOOT).is_err());
}

#[test]
fn nstatus_has_entry_counts_and_never_invents_recovering() {
    let result = parse_nfs_status(br#"{"status":"ok","not_responding":true,"dead":false,"outstanding_request_entries":"3","oldest_request_age_seconds":"4294967295"}"#,"mount").unwrap();
    assert_eq!(result.samples.len(), 2);
    assert_eq!(
        metric(&result, "storage.nfs.mount.outstanding_request_entries").value,
        Some("3".into())
    );
    assert!(!result
        .samples
        .iter()
        .flat_map(|s| &s.metrics)
        .any(|m| m.name.ends_with("recovering")));
    let denied = parse_nfs_status(br#"{"status":"permission_denied"}"#, "mount").unwrap();
    assert!(denied.samples.iter().all(|s| s.metrics.is_empty()));
}

#[test]
fn snapshots_count_only_complete_current_inventory() {
    let result = parse_snapshots(include_bytes!("fixtures/snapshots.plist"), "volume").unwrap();
    assert_eq!(result.resources.len(), 1);
    assert_eq!(
        result.resources[0].attributes["transaction_id"],
        "9007199254740993"
    );
    assert_eq!(
        metric(&result, "storage.apfs.snapshot_count").value,
        Some("1".into())
    );
    assert!(parse_snapshots(b"<plist><dict/></plist>", "volume").is_err());
}

#[test]
fn worker_request_is_strict_and_bounded() {
    assert!(matches!(
        parse_worker_request(br#"{"operation":"mounts"}"#).unwrap(),
        WorkerRequest::Mounts
    ));
    assert!(parse_worker_request(br#"{"operation":"mounts","path":"/"}"#).is_err());
    assert!(parse_worker_request(br#"{"operation":"mounts","operation":"iokit"}"#).is_err());
    assert!(parse_worker_request(&vec![b' '; 65537]).is_err());
}

#[cfg(target_os = "macos")]
async fn native(request: WorkerRequest) -> Vec<u8> {
    use ciderd::command::{CommandOutcome, CommandRunner, CommandSpec};
    use std::{ffi::OsString, path::PathBuf, time::Duration};
    let runner = CommandRunner::new(1, 4 * 1024 * 1024, 4096, None, "smoke-boot".into())
        .await
        .unwrap();
    let job = runner
        .launch(CommandSpec {
            executable: PathBuf::from(env!("CARGO_BIN_EXE_ciderd")),
            args: vec![OsString::from("worker")],
            stdin: Some(serde_json::to_vec(&request).unwrap()),
            timeout: Duration::from_secs(3),
            scope: "native-smoke".into(),
        })
        .await
        .unwrap()
        .unwrap();
    let output = tokio::time::timeout(Duration::from_secs(4), job.result)
        .await
        .expect("supervisor returned by worker deadline")
        .unwrap();
    assert_eq!(
        output.outcome,
        CommandOutcome::Completed,
        "native worker deadline/output failure"
    );
    assert_eq!(
        output.exit_code,
        Some(0),
        "worker stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output.stdout
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn live_worker_mounts_iokit_capacity_identity_and_nstatus_probe_are_bounded() {
    let system = ciderd::platform::system_info().unwrap();
    assert!(!system.boot_id.is_empty() && !system.os_build.is_empty());
    assert!(ciderd::platform::pid_exists(std::process::id()));
    assert!(!ciderd::platform::process_is_definitely_gone(
        std::process::id()
    ));
    let mounts = parse_mounts(&native(WorkerRequest::Mounts).await, NODE, &system.boot_id).unwrap();
    let root = mounts
        .resources
        .iter()
        .find(|r| r.attributes.get("mount_path") == Some(&json!("/")))
        .unwrap();
    let fsid: [i32; 2] = serde_json::from_value(root.attributes["fsid"].clone()).unwrap();
    let capacity = parse_capacity(
        &native(WorkerRequest::Capacity {
            path: "/".into(),
            fsid,
        })
        .await,
        &root.resource_id,
    )
    .unwrap();
    assert_eq!(capacity.samples[0].status, "ok");
    let gone = parse_capacity(
        &native(WorkerRequest::Capacity {
            path: "/".into(),
            fsid: [i32::MIN, i32::MIN],
        })
        .await,
        &root.resource_id,
    )
    .unwrap();
    assert_eq!(gone.samples[0].status, "gone");
    let iokit = parse_iokit(&native(WorkerRequest::Iokit).await, NODE, &system.boot_id).unwrap();
    assert!(iokit.complete);
    // A deliberately nonexistent fsid probes dispatch and error handling without
    // requiring an NFS mount, network access, or a remote filesystem path lookup.
    let status = parse_nfs_status(
        &native(WorkerRequest::NfsStatus {
            fsid: [i32::MIN, i32::MIN],
        })
        .await,
        &root.resource_id,
    )
    .unwrap();
    assert!(status.samples.iter().all(|s| matches!(
        s.status.as_str(),
        "gone" | "unsupported" | "permission_denied"
    )));
}

#[test]
fn iokit_media_evidence_is_optional_and_does_not_change_counter_epoch() {
    let base = include_bytes!("fixtures/iokit.plist");
    let original = ciderd::collectors::parse_iokit(base, "node", "boot").unwrap();
    let mut plist: plist::Value = plist::from_bytes(base).unwrap();
    let row = plist.as_array_mut().unwrap()[0]
        .as_dictionary_mut()
        .unwrap();
    let mut media = plist::Dictionary::new();
    media.insert("bsd_name".into(), "disk0".into());
    media.insert("registry_entry_id".into(), "4294968000".into());
    media.insert("whole".into(), true.into());
    row.insert(
        "whole_media_candidates".into(),
        plist::Value::Array(vec![plist::Value::Dictionary(media)]),
    );
    row.insert("media_mapping_state".into(), "ok".into());
    let mut bytes = Vec::new();
    plist.to_writer_xml(&mut bytes).unwrap();
    let mapped = ciderd::collectors::parse_iokit(&bytes, "node", "boot").unwrap();
    assert_eq!(
        mapped.resources[0].attributes["whole_media_candidates"][0]["bsd_name"],
        "disk0"
    );
    assert_eq!(mapped.samples[0].metrics, original.samples[0].metrics);
    assert_eq!(
        mapped.resources[0].resource_id,
        original.resources[0].resource_id
    );
    assert_eq!(
        original.resources[0].attributes["media_mapping_state"],
        "unavailable"
    );
}

#[test]
fn mount_identity_request_is_strict_and_local_device_only() {
    let valid =
        br#"{"operation":"mount-identity","path":"/","fsid":[1,2],"source":"/dev/disk3s1s1"}"#;
    assert!(ciderd::platform::parse_worker_request(valid).is_ok());
    for invalid in [
        br#"{"operation":"mount-identity","path":"/net","fsid":[1,2],"source":"server:/export"}"#.as_slice(),
        br#"{"operation":"mount-identity","path":"relative","fsid":[1,2],"source":"/dev/disk0"}"#,
        br#"{"operation":"mount-identity","path":"/","fsid":[1,2],"source":"/dev/disk0","extra":true}"#,
    ] { assert!(ciderd::platform::parse_worker_request(invalid).is_err()); }
}

#[test]
fn mount_identity_parser_rejects_races_and_keeps_authoritative_snapshot_parent() {
    let mount = parse_mounts(include_bytes!("fixtures/mounts.json"), NODE, BOOT)
        .unwrap()
        .resources
        .into_iter()
        .find(|r| r.resource_type == "mount")
        .unwrap();
    let value = json!({"state":"ok","reason":null,"fsid":mount.attributes["fsid"],"source":mount.attributes["source"],
        "volume_uuid":"snapshot","media_bsd_name":"disk3s1s1","media_registry_id":"42",
        "parent_volume_uuid":"volume","parent_media_bsd_name":"disk3s1","parent_media_registry_id":"41"});
    let parsed =
        ciderd::collectors::parse_mount_identity(&serde_json::to_vec(&value).unwrap(), &mount)
            .unwrap();
    assert_eq!(parsed.resources[0].resource_id, mount.resource_id);
    assert_eq!(
        parsed.resources[0].attributes["mount_identity"]["parent_volume_uuid"],
        "volume"
    );
    let mut race = value;
    race["fsid"] = json!([987, 654]);
    assert!(
        ciderd::collectors::parse_mount_identity(&serde_json::to_vec(&race).unwrap(), &mount)
            .is_err()
    );
}
