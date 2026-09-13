use ciderd::collectors::parse_iokit;
use ciderd::device_snapshot::{DeviceSnapshot, UsbDevice};
use serde_json::{Value, json};

fn encoded(value: Value) -> Vec<u8> {
    let value: plist::Value = serde_json::from_value(value).unwrap();
    let mut bytes = Vec::new();
    plist::to_writer_binary(&mut bytes, &value).unwrap();
    bytes
}

fn row() -> Value {
    json!({
        "registry_id":"17",
        "media_mapping_state":"ok",
        "whole_media_candidates":[{"bsd_name":"disk7","registry_entry_id":"18","whole":true}],
        "statistics":{"Bytes (Read)":"18446744073709551617"},
        "usb":{"state":"usb","registry_id":"19","speed_id":"4",
            "vendor_id":"1234","product_id":"5678","serial":"PRIVATE-USB-SERIAL"}
    })
}

#[test]
fn native_usb_envelope_preserves_exact_driver_counter() {
    let bytes = encoded(json!({"version":2,"usb_complete":true,"drivers":[row()]}));
    let result = parse_iokit(&bytes, "node", "boot");
    assert!(
        result.is_ok(),
        "new native envelope must retain usable I/O: {result:?}"
    );
    let result = result.unwrap();
    let metric = result
        .samples
        .iter()
        .flat_map(|s| &s.metrics)
        .find(|m| m.name == "storage.device.read_bytes_total")
        .unwrap();
    assert_eq!(metric.value.as_ref().unwrap(), "18446744073709551617");
    assert!(!format!("{result:?}").contains("PRIVATE-USB-SERIAL"));
}

#[test]
fn complete_empty_usb_scan_has_a_host_acquisition() {
    let bytes = encoded(json!({"version":2,"usb_complete":true,"drivers":[]}));
    let result = parse_iokit(&bytes, "node", "boot");
    assert!(result.is_ok(), "empty enumeration is valid source evidence");
    let result = result.unwrap();
    assert!(
        result
            .samples
            .iter()
            .any(|s| s.resource_id == "node/host" && s.collector == "iokit.block")
    );
    assert_eq!(snapshot(&[]).devices.len(), 0);
    assert!(snapshot(&[]).complete);
}

fn snapshot(rows: &[Value]) -> DeviceSnapshot {
    snapshot_context(rows, "node", "boot", true)
}

fn snapshot_context(rows: &[Value], node: &str, boot: &str, complete: bool) -> DeviceSnapshot {
    let result = parse_iokit(
        &encoded(json!({"version":2,"usb_complete":complete,"drivers":rows})),
        node,
        boot,
    )
    .unwrap();
    let source = result
        .samples
        .iter()
        .find(|s| s.resource_id == format!("{node}/host"))
        .unwrap();
    serde_json::from_value(source.extensions.as_ref().unwrap()["usb_device_snapshot"].clone())
        .unwrap()
}

#[test]
fn usb_host_speed_enum_decodes_operational_bitrate_without_legacy_numbering() {
    for (code, expected) in [
        ("1", "12000000"),
        ("2", "1500000"),
        ("3", "480000000"),
        ("4", "5000000000"),
        ("5", "10000000000"),
        ("6", "20000000000"),
    ] {
        let mut source = row();
        source["usb"]["speed_id"] = json!(code);
        let parsed = snapshot(&[source]);
        assert_eq!(parsed.devices.len(), 1, "USB identity must be retained");
        assert!(parsed.complete);
        assert_eq!(parsed.devices[0].speed_state, "available");
        assert_eq!(parsed.devices[0].negotiated_bps.as_deref(), Some(expected));
        assert_eq!(parsed.devices[0].bsd_name.as_deref(), Some("disk7"));
    }
}

#[test]
fn speed_unavailable_keeps_presence_and_never_emits_zero() {
    for (value, state) in [
        (None, "unknown"),
        (Some(json!("0")), "unknown"),
        (Some(json!("7")), "unsupported"),
        (Some(json!(-1)), "unknown"),
        (Some(json!(1.5)), "unknown"),
        (Some(json!("18446744073709551616")), "unknown"),
    ] {
        let mut source = row();
        source["usb"].as_object_mut().unwrap().remove("speed_id");
        if let Some(value) = value {
            source["usb"]["speed_id"] = value;
        }
        let parsed = snapshot(&[source]);
        assert!(parsed.complete);
        assert_eq!(parsed.devices.len(), 1);
        assert_eq!(parsed.devices[0].speed_state, state);
        assert!(parsed.devices[0].negotiated_bps.is_none());
    }
}

#[test]
fn reported_usb_identity_survives_replug_but_driver_counter_epoch_does_not() {
    let before = snapshot(&[row()]);
    assert_eq!(before.devices.len(), 1);
    let mut source = row();
    source["registry_id"] = json!("30");
    source["usb"]["registry_id"] = json!("31");
    source["usb"]["speed_id"] = json!("3");
    source["whole_media_candidates"][0]["bsd_name"] = json!("disk9");
    let after = snapshot_context(&[source.clone()], "node", "new-boot", true);
    assert_eq!(before.devices[0].identity, after.devices[0].identity);
    assert_ne!(
        before.devices[0].driver_resource_id,
        after.devices[0].driver_resource_id
    );
    assert_eq!(after.devices[0].identity_basis, "reported_usb_serial");
    assert_eq!(after.devices[0].identity_scope, "usb_enclosure");
    assert_ne!(
        after.devices[0].identity,
        snapshot_context(&[source.clone()], "other", "new-boot", true).devices[0].identity
    );
    source["usb"]["serial"] = json!("DIFFERENT-PRIVATE-SERIAL");
    assert_ne!(
        after.devices[0].identity,
        snapshot(&[source]).devices[0].identity
    );
    let legacy = parse_iokit(&encoded(json!([row()])), "node", "boot").unwrap();
    let current = parse_iokit(
        &encoded(json!({"version":2,"usb_complete":true,"drivers":[row()]})),
        "node",
        "boot",
    )
    .unwrap();
    assert_eq!(
        legacy.resources[0].resource_id,
        current.resources[0].resource_id
    );
    assert_eq!(
        legacy.resources[0].attributes,
        current.resources[0].attributes
    );
    assert_eq!(legacy.samples[0].metrics, current.samples[0].metrics);
    assert!(legacy.samples.iter().all(|s| s.extensions.is_none()));
}

#[cfg(target_os = "macos")]
#[test]
#[ignore = "passive native acquisition; run explicitly on macOS"]
fn native_iokit_worker_reports_versioned_usb_coverage() {
    let bytes = ciderd::platform::worker(ciderd::platform::WorkerRequest::Iokit)
        .expect("native IOKit acquisition");
    let parsed = parse_iokit(&bytes, "native-check", "native-boot").expect("native parser");
    let host = parsed
        .samples
        .iter()
        .find(|s| s.resource_id == "native-check/host");
    assert!(
        host.is_some(),
        "native source must publish host-scoped USB coverage"
    );
    let snapshot: DeviceSnapshot = serde_json::from_value(
        host.unwrap().extensions.as_ref().unwrap()["usb_device_snapshot"].clone(),
    )
    .unwrap();
    snapshot.validate().unwrap();
    // Do not print native raw properties, tokens or filesystem names.
    println!(
        "native USB snapshot validated: complete={}, device_count={}, reported_enclosure_identities={}, negotiated_bitrates={:?}",
        snapshot.complete,
        snapshot.devices.len(),
        snapshot.devices.iter().filter(|d| d.identity_basis == "reported_usb_serial").count(),
        snapshot.devices.iter().map(|d| d.negotiated_bps.as_deref()).collect::<Vec<_>>()
    );
}

#[test]
fn missing_or_invalid_serial_uses_only_driver_incarnation_identity() {
    for serial in [
        None,
        Some(json!("")),
        Some(json!("   ")),
        Some(json!("0")),
        Some(json!("PRIVATE\u{0}SERIAL")),
        Some(json!("A".repeat(1025))),
    ] {
        let mut source = row();
        source["usb"].as_object_mut().unwrap().remove("serial");
        if let Some(serial) = serial {
            source["usb"]["serial"] = serial;
        }
        let first = snapshot(&[source.clone()]);
        assert!(first.complete);
        assert_eq!(first.devices.len(), 1);
        assert_eq!(first.devices[0].identity_basis, "boot_registry");
        assert_eq!(first.devices[0].identity_scope, "driver_incarnation");
        assert_ne!(
            first.devices[0].identity,
            snapshot_context(&[source], "node", "next", true).devices[0].identity
        );
    }
}

#[test]
fn collision_or_partial_ancestry_never_certifies_device_absence() {
    let mut second = row();
    second["registry_id"] = json!("27");
    second["usb"]["registry_id"] = json!("29");
    let parsed = snapshot(&[row(), second]);
    assert!(!parsed.complete);
    assert!(
        parsed.devices.is_empty(),
        "colliding identities must not transfer a baseline"
    );
    for usb in [
        json!({"state":"unavailable"}),
        json!({"state":"surprise"}),
        json!(false),
    ] {
        let mut source = row();
        source["usb"] = usb;
        let bytes = encoded(json!({"version":2,"usb_complete":true,"drivers":[source]}));
        let parsed = parse_iokit(&bytes, "node", "boot").unwrap();
        assert_eq!(
            parsed.samples[0].metrics[0].value.as_ref().unwrap(),
            "18446744073709551617"
        );
        let host = parsed.samples.last().unwrap();
        assert_eq!(
            host.extensions.as_ref().unwrap()["usb_device_snapshot"]["complete"],
            false
        );
    }
    let mut non_usb = row();
    non_usb["usb"] = json!({"state":"not_usb"});
    assert!(snapshot(&[non_usb]).complete);
    assert!(!snapshot_context(&[], "node", "boot", false).complete);
}

#[test]
fn malformed_media_evidence_cannot_attach_a_usb_identity_to_a_bsd_disk() {
    let mut source = row();
    source["whole_media_candidates"][0]
        .as_object_mut()
        .unwrap()
        .remove("registry_entry_id");
    let parsed = snapshot(&[source]);
    assert!(!parsed.complete);
    assert_eq!(parsed.devices.len(), 1);
    assert!(parsed.devices[0].bsd_name.is_none());
}

#[test]
fn bounded_usb_snapshot_never_turns_dropped_devices_into_confirmed_absence() {
    let rows: Vec<_> = (1..=129)
        .map(|i| {
            let mut r = row();
            r["registry_id"] = json!(i.to_string());
            r["usb"]["serial"] = json!(format!("SYNTHETIC-USB-{i}"));
            r
        })
        .collect();
    let parsed = snapshot(&rows);
    assert!(!parsed.complete);
    assert_eq!(parsed.devices.len(), 128);
    parsed.validate().unwrap();
}

fn valid_device(index: u128) -> UsbDevice {
    UsbDevice {
        identity: uuid::Uuid::from_u128(index).to_string(),
        identity_basis: "reported_usb_serial".into(),
        identity_scope: "usb_enclosure".into(),
        driver_resource_id: format!("driver-{index}"),
        bsd_name: Some("disk7".into()),
        negotiated_bps: Some("5000000000".into()),
        speed_state: "available".into(),
        reason: None,
    }
}

#[test]
fn snapshot_contract_rejects_ambiguous_and_unbounded_wire_evidence() {
    let good = DeviceSnapshot {
        version: 1,
        complete: true,
        devices: vec![valid_device(1)],
    };
    good.validate().unwrap();
    for mutation in 0..12 {
        let mut bad = good.clone();
        match mutation {
            0 => bad.version = 2,
            1 => bad.devices.push(bad.devices[0].clone()),
            2 => bad.devices[0].identity = "private-serial".into(),
            3 => bad.devices[0].identity_scope = "driver_incarnation".into(),
            4 => bad.devices[0].negotiated_bps = Some("0".into()),
            5 => bad.devices[0].negotiated_bps = Some("05000000000".into()),
            6 => bad.devices[0].negotiated_bps = None,
            7 => bad.devices[0].speed_state = "unknown".into(),
            8 => bad.devices[0].bsd_name = Some("disk1s1".into()),
            9 => bad.devices[0].reason = Some("R".repeat(129)),
            10 => bad.devices = (1..=129).map(valid_device).collect(),
            _ => {
                bad.devices = (1..=128).map(valid_device).collect();
                for d in &mut bad.devices {
                    d.driver_resource_id.push_str(&"x".repeat(500));
                }
            }
        }
        assert!(
            bad.validate().is_err(),
            "invalid snapshot case {mutation} accepted"
        );
    }
    let mut unknown = serde_json::to_value(good).unwrap();
    unknown["raw_serial"] = json!("private");
    assert!(serde_json::from_value::<DeviceSnapshot>(unknown).is_err());
}
