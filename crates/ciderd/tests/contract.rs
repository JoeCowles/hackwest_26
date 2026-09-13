use ciderd::{
    config::Config,
    model::{parse_json, Acknowledgement, Decimal, Heartbeat, Metric},
};
use serde_json::{json, Value};

fn fixture() -> Value {
    serde_json::from_str(include_str!("../contract/example-heartbeat.json")).unwrap()
}
fn validate(value: Value) -> anyhow::Result<()> {
    serde_json::from_value::<Heartbeat>(value)?.validate()
}

#[test]
fn integers_above_binary64_precision_round_trip_exactly() {
    let reading: Decimal = serde_json::from_str("\"12345678901234567890\"").unwrap();
    assert_eq!(reading.get(), 12345678901234567890);
    assert_eq!(
        serde_json::to_string(&reading).unwrap(),
        "\"12345678901234567890\""
    );
    for invalid in [
        "\"01\"",
        "\"-1\"",
        "\"1e3\"",
        "123",
        "\"340282366920938463463374607431768211456\"",
    ] {
        assert!(
            serde_json::from_str::<Decimal>(invalid).is_err(),
            "{invalid}"
        );
    }
}

#[test]
fn supplied_wire_fixtures_preserve_fields() {
    for input in [
        include_str!("../contract/example-heartbeat.json"),
        include_str!("../contract/example-degraded-heartbeat.json"),
        include_str!("../contract/example-startup-heartbeat.json"),
        include_str!("../contract/example-repeated-heartbeat.json"),
    ] {
        let raw: serde_json::Value = serde_json::from_str(input).unwrap();
        let heartbeat: Heartbeat = serde_json::from_str(input).unwrap();
        heartbeat.validate().unwrap();
        assert_eq!(serde_json::to_value(heartbeat).unwrap(), raw);
    }
}

#[test]
fn acknowledgement_requires_inventory_confirmation_and_required_nullable_field() {
    let heartbeat: Heartbeat = serde_json::from_value(fixture()).unwrap();
    let original: Value =
        serde_json::from_str(include_str!("../contract/example-heartbeat-ack.json")).unwrap();
    let ack: Acknowledgement = serde_json::from_value(original.clone()).unwrap();
    ack.validate_for(&heartbeat).unwrap();
    assert_eq!(serde_json::to_value(ack).unwrap(), original);
    for revision in [json!("0"), Value::Null] {
        let mut raw = original.clone();
        raw["inventory_revision"] = revision;
        let ack: Acknowledgement = serde_json::from_value(raw).unwrap();
        assert!(ack.validate_for(&heartbeat).is_err());
    }
    let mut missing = original.clone();
    missing
        .as_object_mut()
        .unwrap()
        .remove("inventory_revision");
    assert!(serde_json::from_value::<Acknowledgement>(missing).is_err());
    let mut request = original;
    request["request_inventory"] = json!(true);
    request["inventory_revision"] = Value::Null;
    serde_json::from_value::<Acknowledgement>(request)
        .unwrap()
        .validate_for(&heartbeat)
        .unwrap();
}

#[test]
fn metric_availability_controls_measurement_field_presence() {
    let mut available = fixture();
    available["collections"][0]["metrics"][0]
        .as_object_mut()
        .unwrap()
        .remove("freshness");
    assert!(validate(available).is_err());
    for field in ["value", "value_type", "freshness"] {
        let mut heartbeat = fixture();
        let metric = heartbeat["collections"][0]["metrics"][0]
            .as_object_mut()
            .unwrap();
        metric.insert("availability".into(), json!("timeout"));
        for other in ["value", "value_type", "freshness"] {
            if other != field {
                metric.remove(other);
            }
        }
        assert!(
            validate(heartbeat).is_err(),
            "unavailable metric retained {field}"
        );
    }
}

#[test]
fn optional_nonnullable_fields_reject_explicit_null() {
    for pointer in [
        "/collections/0/metrics/0/freshness",
        "/collections/0/metrics/0/value",
        "/collections/0/error",
        "/resources/0/capabilities",
        "/collector_states/0/last_attempt_id",
        "/extensions",
    ] {
        let mut heartbeat = fixture();
        let (parent, field) = pointer.rsplit_once('/').unwrap();
        heartbeat
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert(field.into(), Value::Null);
        assert!(validate(heartbeat).is_err(), "accepted null at {pointer}");
    }
    let mut heartbeat = fixture();
    heartbeat["collections"][0]["metrics"][0]["effective_at"] = Value::Null;
    validate(heartbeat).unwrap();
}

#[test]
fn metric_dimensions_are_complete_and_source_allowlisted() {
    let source = fixture()["collections"][4]["metrics"][0].clone();
    for attributes in [
        json!({}),
        json!({"family":"v3_procedure"}),
        json!({"family":"v3_procedure","operation":"invented"}),
        json!({"family":"v3_procedure","operation":true}),
        json!({"family":"v3_procedure","operation":"layoutget"}),
        json!({"family":"v4_callback","operation":"cb_compound"}),
    ] {
        let mut value = source.clone();
        value["attributes"] = attributes;
        assert!(serde_json::from_value::<Metric>(value)
            .unwrap()
            .validate()
            .is_err());
    }
    for (family, operation) in [
        ("v3_procedure", "READ"),
        ("v3_procedure", "rdirplus"),
        ("v4_rpc", "compound"),
        ("v4_operation", "open_conf"),
        ("v4_operation", "exchangeid"),
        ("v4_operation", "layoutget"),
        ("v4_callback", "cb_getattr"),
    ] {
        let mut value = source.clone();
        value["attributes"] = json!({"family":family,"operation":operation});
        serde_json::from_value::<Metric>(value)
            .unwrap()
            .validate()
            .unwrap();
    }
    let mut cache = Metric::integer(
        "storage.nfs.client.cache_hits_total",
        1,
        "live",
        Some("epoch"),
    )
    .unwrap();
    for family in [
        "accs", "attr", "biod", "bior", "biorl", "biow", "dire", "lkup",
    ] {
        cache.attributes.insert("cache".into(), json!(family));
        cache.validate().unwrap();
    }
    cache.attributes.insert("cache".into(), json!("invented"));
    assert!(cache.validate().is_err());
    let mut diagnostic =
        Metric::reading("storage.probe.success", json!(true), "live", None).unwrap();
    diagnostic.attributes =
        ciderd::model::attrs(json!({"operation":"arbitrary", "stack":"native"}));
    assert!(diagnostic.validate().is_err());
}

#[test]
fn integer_gauges_preserve_signed_and_wide_positive_values() {
    for value in [
        "-1",
        "-170141183460469231731687303715884105728",
        "340282366920938463463374607431768211455",
    ] {
        let metric = Metric::reading(
            "storage.filesystem.available_bytes",
            json!(value),
            "cached",
            None,
        )
        .unwrap();
        metric.validate().unwrap();
        assert_eq!(serde_json::to_value(metric).unwrap()["value"], json!(value));
    }
    for value in [
        "-0",
        "-01",
        "+1",
        "1e3",
        "-170141183460469231731687303715884105729",
        "340282366920938463463374607431768211456",
    ] {
        assert!(Metric::reading(
            "storage.filesystem.available_bytes",
            json!(value),
            "cached",
            None
        )
        .is_err());
    }
    assert!(Metric::reading(
        "storage.device.read_bytes_total",
        json!("-1"),
        "live",
        Some("epoch")
    )
    .is_err());
}

#[test]
fn sequence_and_generation_stop_at_u64_maximum() {
    for field in ["sequence", "agent_generation"] {
        let mut heartbeat = fixture();
        heartbeat[field] = json!("18446744073709551615");
        validate(heartbeat.clone()).unwrap();
        heartbeat[field] = json!("18446744073709551616");
        assert!(validate(heartbeat).is_err(), "accepted overflowing {field}");
    }
}

#[test]
fn heartbeat_rejects_inconsistent_inventory_clock_and_collector_states() {
    let mut hidden_inventory = fixture();
    hidden_inventory["inventory"]["included"] = json!(false);
    assert!(validate(hidden_inventory).is_err());
    let mut duplicate = fixture();
    let state = duplicate["collector_states"][0].clone();
    duplicate["collector_states"]
        .as_array_mut()
        .unwrap()
        .push(state);
    assert!(validate(duplicate).is_err());
    let mut future = fixture();
    future["monotonic_ns"] = json!("0");
    assert!(validate(future).is_err());
    let mut quarantine = fixture();
    quarantine["collector_states"][0]["phase"] = json!("timed_out_pending_exit");
    assert!(validate(quarantine.clone()).is_err());
    quarantine["collector_states"][0]
        .as_object_mut()
        .unwrap()
        .remove("last_attempt_id");
    assert!(validate(quarantine).is_err());
    let mut previous_clock = fixture();
    previous_clock["clock_id"] = json!("new-segment");
    previous_clock["monotonic_ns"] = json!("0");
    validate(previous_clock).unwrap();
}

#[test]
fn nested_records_enforce_schema_types_lengths_and_ranges() {
    for (pointer, value) in [
        ("/resources/0/resource_type", json!("invented")),
        ("/resources/0/resource_id", json!("")),
        ("/resources/0/observed_at", json!("yesterday")),
        ("/resources/0/identity_confidence", json!("certain")),
        ("/relationships/0/relation", json!("invented")),
        (
            "/relationships/0/from_resource_id",
            json!("unknown-resource"),
        ),
        ("/agent/delivery_mode", json!("historical")),
        ("/agent/heartbeat_interval_seconds", json!(0)),
        ("/agent/quarantined_workers", json!(4097)),
        ("/agent/version", json!("x".repeat(65))),
        ("/collector_states/0/poll_interval_seconds", json!(0)),
        ("/collector_states/0/stale_after_seconds", json!(604801)),
        ("/collections/0/adapter_version", json!("x".repeat(65))),
        ("/collections/0/resource_id", json!("unknown-resource")),
    ] {
        let mut heartbeat = fixture();
        *heartbeat.pointer_mut(pointer).unwrap() = value;
        assert!(validate(heartbeat).is_err(), "accepted invalid {pointer}");
    }
    for (field, value) in [
        ("worker_state", json!("invented")),
        ("raw_artifact_sha256", json!("G".repeat(64))),
        (
            "error",
            json!({"domain":"io", "code":"signal", "message":"failed", "signal":0}),
        ),
    ] {
        let mut heartbeat = fixture();
        heartbeat["collections"][0][field] = value;
        assert!(validate(heartbeat).is_err(), "accepted invalid {field}");
    }
    let mut heartbeat = fixture();
    heartbeat["resources"][0]["capabilities"] = json!({"smart":{"support":"invented"}});
    assert!(validate(heartbeat).is_err());
    let mut heartbeat = fixture();
    heartbeat["collections"][0]["metrics"][0]["effective_at"] = json!({"timestamp":"invalid"});
    assert!(validate(heartbeat).is_err());
    let mut heartbeat = fixture();
    heartbeat["collections"][0]["metrics"][0]["reason"] = json!("x".repeat(4097));
    assert!(validate(heartbeat).is_err());
    let mut heartbeat = fixture();
    heartbeat["collections"][0]["metrics"][0]["extensions"] =
        Value::Object((0..17).map(|i| (i.to_string(), json!(0))).collect());
    assert!(validate(heartbeat).is_err());
}

#[test]
fn malformed_events_and_tombstones_do_not_pass_as_inventory() {
    let mut heartbeat = fixture();
    let resource_id = heartbeat["resources"][0]["resource_id"].clone();
    let event = json!({"event_id":"event", "resource_id":resource_id, "observed_at":"2026-09-12T00:00:00Z", "source":"agent", "category":"collector", "severity":"warning", "message":"failure"});
    let tombstone = json!({"entity_type":"resource", "entity_id":"removed", "revision":"1", "observed_at":"2026-09-12T00:00:00Z", "reason":"gone"});
    heartbeat["events"] = json!([event]);
    heartbeat["tombstones"] = json!([tombstone]);
    validate(heartbeat.clone()).unwrap();
    for (pointer, value) in [
        ("/events/0/severity", json!("fatal")),
        ("/events/0/category", json!("invented")),
        ("/events/0/message", json!("x".repeat(4097))),
        ("/tombstones/0/entity_type", json!("collection")),
        ("/tombstones/0/observed_at", json!("invalid")),
    ] {
        let mut malformed = heartbeat.clone();
        *malformed.pointer_mut(pointer).unwrap() = value;
        assert!(validate(malformed).is_err(), "accepted invalid {pointer}");
    }
}

#[test]
fn ambiguous_keys_and_nonfinite_extension_numbers_are_rejected() {
    assert!(parse_json::<Value>(br#"{"counter":{"value":"1","value":"2"}}"#).is_err());
    let mut heartbeat = fixture();
    heartbeat["extensions"] = serde_json::from_str(r#"{"value":1e400}"#).unwrap();
    assert!(validate(heartbeat).is_err());
}

#[test]
fn strict_attribute_constructor_rejects_incomplete_series() {
    let name = "storage.nfs.client.operations_total";
    let temporary = Metric::integer(name, 1, "live", Some("epoch")).unwrap();
    assert!(temporary.validate().is_err());
    assert!(Metric::reading_with_attributes(
        name,
        json!("1"),
        "live",
        Some("epoch"),
        Default::default()
    )
    .is_err());
    let attributes = ciderd::model::attrs(json!({"family":"v3_procedure", "operation":"read"}));
    Metric::reading_with_attributes(name, json!("1"), "live", Some("epoch"), attributes)
        .unwrap()
        .validate()
        .unwrap();
}

#[test]
fn omitted_inventory_references_and_scopes_validate_against_cached_resources() {
    let mut heartbeat: Heartbeat = serde_json::from_value(fixture()).unwrap();
    let resources = heartbeat
        .resources
        .iter()
        .cloned()
        .map(|resource| (resource.resource_id.clone(), resource))
        .collect();
    heartbeat.inventory.included = false;
    heartbeat.resources.clear();
    heartbeat.relationships.clear();
    heartbeat.tombstones.clear();
    heartbeat.validate_with_resources(&resources).unwrap();
    let original_resource = heartbeat.collections[0].resource_id.clone();
    heartbeat.collections[0].resource_id = "unknown-resource".into();
    assert!(heartbeat.validate_with_resources(&resources).is_err());
    heartbeat.collections[0].resource_id = original_resource;
    let state_resource = heartbeat.collector_states[0].resource_id.clone();
    heartbeat.collector_states[0].resource_id = "unknown-resource".into();
    assert!(heartbeat.validate_with_resources(&resources).is_err());
    heartbeat.collector_states[0].resource_id = state_resource;
    let mut wrong_scope = resources.clone();
    wrong_scope
        .get_mut(&heartbeat.collections[0].resource_id)
        .unwrap()
        .resource_type = "mount".into();
    assert!(heartbeat.validate_with_resources(&wrong_scope).is_err());
}

#[test]
fn config_rejects_misspellings_and_unsafe_transport() {
    let input = include_str!("../examples/ciderd.toml");
    Config::parse(input).unwrap();
    for invalid in [
        input.replace("follow_redirects = false", "follow_redirects = true"),
        input.replace("https://", "http://"),
        input.replace("request_timeout_seconds", "request_timout_seconds"),
        input.replace("io_seconds = 3", "io_seconds = 0"),
        input.replace("recent_sample_history = 0", "recent_sample_history = 1"),
    ] {
        assert!(Config::parse(&invalid).is_err());
    }
}
