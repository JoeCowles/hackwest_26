use orchard_server::{
    cider_wire::Metric,
    reliability::{Observation, SourceIdentity, SourceState},
};
use serde_json::json;
fn state() -> SourceState {
    SourceState::new(SourceIdentity {
        source_id: "source".into(),
        node_id: "node".into(),
        object_id: "disk".into(),
        resource_id: "native".into(),
        collector: "iokit".into(),
        scope: "driver".into(),
    })
}
fn obs(t: u64, metrics: Vec<Metric>) -> Observation {
    Observation {
        collection_id: format!("c{t}"),
        boot_id: "boot".into(),
        agent_generation: "1".into(),
        agent_session_id: "session".into(),
        clock_id: "clock".into(),
        source_generation: "1".into(),
        source_version: "1".into(),
        adapter_version: "1".into(),
        finished_monotonic_ns: (t * 1_000_000_000).into(),
        observed_at: format!("2026-09-13T00:{:02}:{:02}Z", t / 60 % 60, t % 60),
        received_at_ms: 1_789_257_600_000 + t as i64 * 1000,
        age_at_receipt_seconds: 0.,
        stale_after_seconds: 30.,
        status: "ok".into(),
        metrics,
    }
}
fn errors(n: u128) -> Metric {
    Metric::integer("storage.device.read_errors_total", n, "live", Some("epoch")).unwrap()
}
#[test]
fn historical_errors_are_not_new_but_exact_increments_are() {
    let mut s = state();
    assert!(s.observe(obs(0, vec![errors(7)])).is_empty());
    let events = s.observe(obs(5, vec![errors(8)]));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].status, "open");
    assert_eq!(events[0].evidence["delta"], json!("1"));
}
#[test]
fn gaps_and_replays_do_not_clear_and_two_distinct_clear_observations_do() {
    let mut s = state();
    s.observe(obs(0, vec![errors(0)]));
    assert_eq!(s.observe(obs(5, vec![errors(1)])).len(), 1);
    assert!(s.observe(obs(10, vec![])).is_empty());
    assert!(s.observe(obs(15, vec![errors(1)])).is_empty());
    assert!(s.observe(obs(15, vec![errors(1)])).is_empty());
    let e = s.observe(obs(20, vec![errors(1)]));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].status, "resolved");
}
#[test]
fn epoch_change_interrupts_instead_of_resolving() {
    let mut s = state();
    s.observe(obs(0, vec![errors(0)]));
    s.observe(obs(5, vec![errors(1)]));
    let mut m = errors(0);
    m.counter_epoch = Some("new".into());
    let e = s.observe(obs(10, vec![m]));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].status, "interrupted");
}
#[test]
fn reported_smart_failure_is_immediate() {
    let mut s = state();
    let m = Metric::reading("storage.media.smart_passed", json!(false), "live", None).unwrap();
    let e = s.observe(obs(0, vec![m]));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].classification, "reported_health_condition");
    assert_eq!(e[0].severity, "critical");
}
fn perf(t: u64, ops: u128, bytes: u128, time: u128) -> Observation {
    obs(
        t,
        vec![
            Metric::integer(
                "storage.device.read_operations_total",
                ops,
                "live",
                Some("epoch"),
            )
            .unwrap(),
            Metric::integer(
                "storage.device.read_bytes_total",
                bytes,
                "live",
                Some("epoch"),
            )
            .unwrap(),
            Metric::integer(
                "storage.device.read_accounted_time_nanoseconds_total",
                time,
                "live",
                Some("epoch"),
            )
            .unwrap(),
        ],
    )
}
fn trained() -> (SourceState, u128) {
    let mut s = state();
    for n in 0..=60 {
        assert!(
            s.observe(perf(
                n * 10,
                n as u128 * 100,
                n as u128 * 409600,
                n as u128 * 100_000_000
            ))
            .is_empty()
        );
    }
    (s, 6_000_000_000)
}
#[test]
fn service_time_requires_120_elevated_seconds_and_60_recovery_seconds() {
    let (mut s, mut time) = trained();
    for n in 61..=72 {
        time += 500_000_000;
        let e = s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time));
        if n < 72 {
            assert!(e.is_empty());
        } else {
            assert_eq!(e.len(), 1);
            assert_eq!(e[0].rule_id, "iokit.read_service_time");
            assert_eq!(
                e[0].evidence["reference_ns_per_operation"],
                json!(1_000_000.)
            );
            assert_eq!(e[0].evidence["elevated_seconds"], json!(120.));
        }
    }
    for n in 73..=78 {
        time += 100_000_000;
        let e = s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time));
        if n < 78 {
            assert!(e.is_empty());
        } else {
            assert_eq!(e.len(), 1);
            assert_eq!(e[0].status, "resolved");
            assert_eq!(e[0].evidence["recovery_seconds"], json!(60.));
        }
    }
}
#[test]
fn different_workload_and_idle_intervals_do_not_open() {
    let (mut s, mut time) = trained();
    for n in 61..=90 {
        time += 500_000_000;
        assert!(
            s.observe(perf(
                n * 10,
                n as u128 * 100,
                60 * 409600 + (n - 60) as u128 * 1_638_400,
                time
            ))
            .is_empty()
        );
    }
    let v = s.summary(1_789_257_600_000 + 900000, true);
    let signal = v["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["rule_id"] == "iokit.read_service_time")
        .unwrap();
    assert_eq!(signal["state"], "warming_up");
    assert!(
        s.observe(perf(910, 9000, 60 * 409600 + 30 * 1_638_400, time))
            .is_empty()
    );
}
#[test]
fn missing_service_time_breaks_pending_and_restart_preserves_baseline() {
    let (mut s, mut time) = trained();
    let bytes = serde_json::to_vec(&s).unwrap();
    s = serde_json::from_slice(&bytes).unwrap();
    for n in 61..=71 {
        time += 500_000_000;
        assert!(
            s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time))
                .is_empty()
        );
    }
    s.observe(obs(720, vec![]));
    time += 500_000_000;
    assert!(s.observe(perf(730, 7300, 73 * 409600, time)).is_empty());
    for n in 74..=84 {
        time += 500_000_000;
        assert!(
            s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time))
                .is_empty()
        );
    }
    time += 500_000_000;
    assert_eq!(s.observe(perf(850, 8500, 85 * 409600, time)).len(), 1);
}
#[test]
fn huge_counters_stay_exact_and_stale_readings_cannot_clear() {
    let mut s = state();
    let big = u128::MAX - 3;
    s.observe(obs(0, vec![errors(big)]));
    let e = s.observe(obs(5, vec![errors(big + 1)]));
    assert_eq!(e[0].evidence["delta"], "1");
    let mut stale = obs(10, vec![errors(big + 1)]);
    stale.age_at_receipt_seconds = 31.;
    assert!(s.observe(stale).is_empty());
    assert!(s.observe(obs(15, vec![errors(big + 1)])).is_empty());
    let e = s.observe(obs(20, vec![errors(big + 1)]));
    assert_eq!(e[0].status, "resolved");
}
fn smart(passed: bool, identity: Option<&str>) -> Metric {
    let mut m = Metric::reading("storage.media.smart_passed", json!(passed), "live", None).unwrap();
    m.extensions = Some(std::collections::BTreeMap::from([
        (
            "device_identity_confidence".into(),
            json!(if identity.is_some() {
                "reported_serial"
            } else {
                "caller_epoch_only"
            }),
        ),
        (
            "smartctl_exit_status".into(),
            json!(if passed { 0 } else { 8 }),
        ),
        ("smartctl_exit_status_class".into(), json!("device_report")),
    ]));
    if let Some(id) = identity {
        m.extensions
            .as_mut()
            .unwrap()
            .insert("device_identity".into(), json!(id));
    }
    m
}
#[test]
fn smart_identity_and_exit_provenance_are_explicit_and_replacements_interrupt() {
    let mut s = state();
    let events = s.observe(obs(0, vec![smart(false, Some("disk-A"))]));
    assert_eq!(
        events[0].evidence["device_identity_confidence"],
        "reported_serial"
    );
    assert_eq!(events[0].evidence["smartctl_exit_status"], 8);
    let e = s.observe(obs(5, vec![smart(true, Some("disk-B"))]));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].status, "interrupted");
    assert_eq!(e[0].object_id, "disk");
}
#[test]
fn conflicting_smart_identities_are_unusable() {
    let mut s = state();
    let mut a = smart(false, Some("A"));
    let mut b = Metric::integer("storage.nvme.critical_warning_bits", 1, "live", None).unwrap();
    b.extensions = a.extensions.clone();
    b.extensions
        .as_mut()
        .unwrap()
        .insert("device_identity".into(), json!("B"));
    a.extensions
        .as_mut()
        .unwrap()
        .insert("device_identity".into(), json!("A"));
    assert!(s.observe(obs(0, vec![a, b])).is_empty());
}
#[test]
fn performance_first_seen_survives_gaps_in_an_open_episode() {
    let (mut s, mut time) = trained();
    let mut opened = String::new();
    for n in 61..=72 {
        time += 500_000_000;
        for e in s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time)) {
            opened = e.first_seen_at;
        }
    }
    s.observe(obs(730, vec![]));
    time += 500_000_000;
    s.observe(perf(740, 7400, 74 * 409600, time));
    time += 500_000_000;
    let e = s.observe(perf(750, 7500, 75 * 409600, time));
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].first_seen_at, opened);
}
#[test]
fn summaries_do_not_contain_training_rings_and_age_each_signal() {
    let (s, _) = trained();
    let summary = s.summary(1_789_257_600_000 + 631000, true);
    let p = summary["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["rule_id"] == "iokit.read_service_time")
        .unwrap();
    assert_eq!(p["observation"]["state"], "stale");
    assert!(
        !serde_json::to_string(&summary)
            .unwrap()
            .contains("\"ring\"")
    );
    assert!(serde_json::to_vec(&s).unwrap().len() < 524288);
}
#[test]
fn nvme_decoding_preserves_unknown_bits_and_current_endurance() {
    let mut s = state();
    let e = s.observe(obs(
        0,
        vec![
            Metric::integer("storage.nvme.critical_warning_bits", 139, "live", None).unwrap(),
            Metric::integer("storage.nvme.endurance_used_percent", 100, "live", None).unwrap(),
        ],
    ));
    assert_eq!(e.len(), 2);
    let warning = e
        .iter()
        .find(|f| f.rule_id == "nvme.critical_warning")
        .unwrap();
    assert_eq!(
        warning.evidence["warnings"],
        json!([
            "spare_depleted",
            "temperature",
            "read_only_media",
            "unknown_warning_bit"
        ])
    );
    assert_eq!(
        e.iter()
            .find(|f| f.rule_id == "nvme.endurance_used")
            .unwrap()
            .classification,
        "replacement_planning"
    );
}
#[test]
fn nonadvancing_acquisitions_do_not_change_persisted_state() {
    let mut s = state();
    s.observe(obs(5, vec![errors(7)]));
    let before = serde_json::to_value(&s).unwrap();
    let mut replay = obs(4, vec![errors(9)]);
    replay.collection_id = "different-id".into();
    assert!(s.observe(replay).is_empty());
    assert_eq!(serde_json::to_value(&s).unwrap(), before);
}
#[test]
fn initial_summary_is_explicit_unknown() {
    assert_eq!(state().summary(0, true)["observation"]["state"], "unknown");
}
#[test]
fn counter_evidence_identifies_exact_interval_and_epoch() {
    let mut s = state();
    s.observe(obs(0, vec![errors(7)]));
    let e = s.observe(obs(5, vec![errors(8)]));
    assert_eq!(e[0].evidence["previous_collection_id"], "c0");
    assert_eq!(e[0].evidence["collection_id"], "c5");
    assert_eq!(e[0].evidence["counter_epoch"], "epoch");
    assert_eq!(e[0].evidence["monotonic_start_ns"], "0");
    assert_eq!(e[0].evidence["monotonic_end_ns"], "5000000000");
}
#[test]
fn partial_smart_acquisition_keeps_usable_health_and_missing_rules_unknown() {
    let mut s = state();
    let mut o = obs(0, vec![smart(false, None)]);
    o.status = "partial".into();
    assert_eq!(s.observe(o).len(), 1);
    let summary = s.summary(1_789_257_600_000, true);
    let signals = summary["signals"].as_array().unwrap();
    assert_eq!(
        signals
            .iter()
            .find(|s| s["rule_id"] == "smart.overall")
            .unwrap()["observation"]["state"],
        "current"
    );
    assert_eq!(
        signals
            .iter()
            .find(|s| s["rule_id"] == "nvme.critical_warning")
            .unwrap()["observation"]["state"],
        "unavailable"
    );
}
#[test]
fn weighted_p95_uses_covered_time_and_not_sample_counts() {
    let mut s = state();
    let (mut t, mut ops, mut bytes, mut time) = (0u64, 0u128, 0u128, 0u128);
    s.observe(perf(t, ops, bytes, time));
    for (dt, ns) in std::iter::repeat_n((10u64, 1_000_000u128), 59)
        .chain(std::iter::repeat_n((1, 2_000_000), 10))
    {
        t += dt;
        ops += dt as u128 * 20;
        bytes += dt as u128 * 20 * 4096;
        time += dt as u128 * 20 * ns;
        assert!(s.observe(perf(t, ops, bytes, time)).is_empty());
    }
    assert_eq!(t, 600);
    for _ in 0..12 {
        t += 10;
        ops += 200;
        bytes += 200 * 4096;
        time += 200 * 5_000_000;
        let e = s.observe(perf(t, ops, bytes, time));
        if t == 720 {
            assert_eq!(e.len(), 1);
            assert_eq!(
                e[0].evidence["reference_ns_per_operation"],
                json!(1_000_000.)
            );
        }
    }
}
#[test]
fn bounded_baselines_expire_by_source_time() {
    let mut s = state();
    for n in 0..=1000 {
        s.observe(perf(
            n * 5,
            n as u128 * 100,
            n as u128 * 409600,
            n as u128 * 100_000_000,
        ));
    }
    let value = serde_json::to_value(&s).unwrap();
    let ring = value["performance"]["read"]["ring"].as_array().unwrap();
    assert!(ring.len() <= 720);
    assert!(ring.iter().all(|x| x["end"].as_f64().unwrap() > 1400.));
    assert!(serde_json::to_vec(&s).unwrap().len() < 524288);
}
#[test]
fn ata_gauges_report_current_defects_and_only_new_reallocations() {
    let mut s = state();
    let gauges = |reallocated, pending, uncorrectable| {
        vec![
            Metric::integer("storage.ata.reallocated_sectors", reallocated, "live", None).unwrap(),
            Metric::integer("storage.ata.current_pending_sectors", pending, "live", None).unwrap(),
            Metric::integer(
                "storage.ata.offline_uncorrectable_sectors",
                uncorrectable,
                "live",
                None,
            )
            .unwrap(),
        ]
    };
    let first = s.observe(obs(0, gauges(20, 2, 1)));
    assert_eq!(first.len(), 2);
    assert!(
        first
            .iter()
            .all(|f| f.rule_id != "ata.reallocated_increase")
    );
    let next = s.observe(obs(5, gauges(21, 0, 0)));
    assert_eq!(next.len(), 1);
    assert_eq!(next[0].rule_id, "ata.reallocated_increase");
    assert_eq!(next[0].evidence["delta"], "1");
    let clear = s.observe(obs(10, gauges(21, 0, 0)));
    assert_eq!(clear.len(), 2);
    assert!(clear.iter().all(|f| f.status == "resolved"));
}
#[test]
fn spare_threshold_and_decline_are_distinct_from_historical_logs() {
    let mut s = state();
    let metrics = |spare| {
        vec![
            Metric::integer("storage.nvme.available_spare_percent", spare, "live", None).unwrap(),
            Metric::integer(
                "storage.nvme.available_spare_threshold_percent",
                10,
                "live",
                None,
            )
            .unwrap(),
            Metric::integer(
                "storage.nvme.error_log_entries_total",
                500,
                "live",
                Some("epoch"),
            )
            .unwrap(),
            Metric::integer(
                "storage.nvme.unsafe_shutdowns_total",
                30,
                "live",
                Some("epoch"),
            )
            .unwrap(),
        ]
    };
    assert!(s.observe(obs(0, metrics(11))).is_empty());
    let e = s.observe(obs(5, metrics(9)));
    assert_eq!(e.len(), 2);
    assert_eq!(
        e.iter()
            .find(|f| f.rule_id == "nvme.spare_decline")
            .unwrap()
            .evidence["delta"],
        "2"
    );
    assert_eq!(
        e.iter()
            .find(|f| f.rule_id == "nvme.spare_below_threshold")
            .unwrap()
            .classification,
        "replacement_planning"
    );
}
#[test]
fn missing_observation_breaks_direct_recovery_streak() {
    let mut s = state();
    s.observe(obs(0, vec![smart(false, None)]));
    assert!(s.observe(obs(5, vec![smart(true, None)])).is_empty());
    assert!(s.observe(obs(10, vec![])).is_empty());
    assert!(s.observe(obs(15, vec![smart(true, None)])).is_empty());
    assert_eq!(
        s.observe(obs(20, vec![smart(true, None)]))[0].status,
        "resolved"
    );
}
#[test]
fn no_timing_and_gaps_break_performance_recovery_streak() {
    let (mut s, mut time) = trained();
    for n in 61..=72 {
        time += 500_000_000;
        s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time));
    }
    for n in 73..=77 {
        time += 100_000_000;
        assert!(
            s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time))
                .is_empty()
        );
    }
    assert!(s.observe(perf(780, 7800, 78 * 409600, time)).is_empty());
    for n in 79..=83 {
        time += 100_000_000;
        assert!(
            s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time))
                .is_empty()
        );
    }
    time += 100_000_000;
    assert_eq!(
        s.observe(perf(840, 8400, 84 * 409600, time))[0].status,
        "resolved"
    );
}
#[test]
fn invalid_age_and_clock_change_never_become_recovery() {
    let mut s = state();
    s.observe(obs(0, vec![smart(false, None)]));
    let mut invalid = obs(5, vec![smart(true, None)]);
    invalid.age_at_receipt_seconds = -1.;
    assert!(s.observe(invalid).is_empty());
    let mut changed = obs(10, vec![smart(true, None)]);
    changed.clock_id = "new-clock".into();
    let e = s.observe(changed);
    assert_eq!(e.len(), 1);
    assert_eq!(e[0].status, "interrupted");
}
#[test]
fn counter_findings_keep_bounded_source_provenance() {
    let mut s = state();
    let m = |value| {
        let mut metric = Metric::integer(
            "storage.nvme.media_errors_total",
            value,
            "live",
            Some("epoch"),
        )
        .unwrap();
        metric.extensions = smart(false, Some("A")).extensions;
        metric.source_field = Some("nvme_smart_health_information_log.media_errors".into());
        metric
    };
    s.observe(obs(0, vec![m(3)]));
    let e = s.observe(obs(5, vec![m(4)]));
    assert_eq!(
        e[0].evidence["source_field"],
        "nvme_smart_health_information_log.media_errors"
    );
    assert_eq!(
        e[0].evidence["device_identity_confidence"],
        "reported_serial"
    );
    assert_eq!(e[0].evidence["smartctl_exit_status"], 8);
}
#[test]
fn unavailable_smart_placeholders_do_not_imply_device_replacement() {
    let mut s = state();
    s.observe(obs(0, vec![smart(false, Some("A"))]));
    let mut missing = smart(true, None);
    missing.availability = "unavailable".into();
    missing.freshness = None;
    missing.value = None;
    missing.extensions = None;
    let mut o = obs(5, vec![missing]);
    o.status = "partial".into();
    assert!(s.observe(o).is_empty());
    assert_eq!(
        s.summary(1_789_257_605_000, true)["findings"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
#[test]
fn findings_expose_scope_and_applied_thresholds() {
    let mut s = state();
    s.source.scope = "nvme_namespace".into();
    let e = s.observe(obs(
        0,
        vec![Metric::integer("storage.nvme.endurance_used_percent", 100, "live", None).unwrap()],
    ));
    assert_eq!(e[0].evidence["scope"], "nvme_namespace");
    assert_eq!(e[0].evidence["threshold"], "100");
    assert_eq!(e[0].evidence["clear_observations_required"], 2);
}
#[test]
fn transfer_bucket_boundaries_use_exact_counter_deltas() {
    let mut s = state();
    s.observe(perf(0, 0, 0, 0));
    let ops = 1u128 << 70;
    s.observe(perf(10, ops, ops * 4096 + 1, ops));
    let summary = s.summary(1_789_257_610_000, true);
    let p = summary["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["rule_id"] == "iokit.read_service_time")
        .unwrap();
    assert_eq!(p["evidence"]["workload_bucket"]["transfer_size_band"], 1);
}
#[test]
fn operation_rate_bucket_boundaries_use_exact_counter_deltas() {
    let mut s = state();
    s.observe(perf(0, 0, 0, 0));
    let ops = (1u128 << 53) * 10 - 1;
    s.observe(perf(10, ops, ops * 4096, ops));
    let summary = s.summary(1_789_257_610_000, true);
    let p = summary["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["rule_id"] == "iokit.read_service_time")
        .unwrap();
    assert_eq!(
        p["evidence"]["workload_bucket"]["operation_rate_power_of_two_band"],
        52
    );
}
#[test]
fn interruption_invalidates_source_currentness_even_if_registry_reactivates() {
    let mut s = state();
    s.observe(obs(0, vec![errors(7)]));
    s.interrupt("source_removed", 1_789_257_601_000);
    s.active = true;
    assert!(s.observe(obs(0, vec![errors(7)])).is_empty());
    assert_eq!(
        s.summary(1_789_257_601_000, true)["observation"]["state"],
        "unavailable"
    );
}
#[test]
fn changed_workload_keeps_open_history_but_cannot_claim_current_degradation() {
    let (mut s, mut time) = trained();
    for n in 61..=72 {
        time += 500_000_000;
        s.observe(perf(n * 10, n as u128 * 100, n as u128 * 409600, time));
    }
    time += 500_000_000;
    assert!(
        s.observe(perf(730, 7300, 72 * 409600 + 100 * 16384, time))
            .is_empty()
    );
    let summary = s.summary(1_789_258_330_000, true);
    let p = summary["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["rule_id"] == "iokit.read_service_time")
        .unwrap();
    assert_eq!(p["state"], "workload_not_comparable");
    assert_eq!(p["observation"]["state"], "unavailable");
    assert_eq!(summary["findings"].as_array().unwrap().len(), 1);
}
#[test]
fn every_observation_shape_has_an_explicit_reason() {
    let mut s = state();
    let before = s.summary(1_789_257_600_000, true);
    assert_eq!(before["observation"]["reason"], "no_observation");
    s.observe(obs(0, vec![errors(7)]));
    let current = s.summary(1_789_257_600_000, true);
    assert_eq!(
        current["observation"].get("reason"),
        Some(&serde_json::Value::Null)
    );
    for signal in current["signals"].as_array().unwrap() {
        assert!(signal["observation"].get("reason").is_some());
    }
    assert_eq!(
        s.summary(1_789_257_631_000, true)["observation"]["reason"],
        "observation_stale"
    );
    s.observe(obs(5, vec![]));
    let missing = s.summary(1_789_257_605_000, true);
    let errors = missing["signals"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["rule_id"] == "iokit.read_errors")
        .unwrap();
    assert_eq!(
        errors["observation"]["reason"],
        "missing_or_ineligible_evidence"
    );
}
#[test]
fn spare_comparison_preserves_each_input_provenance() {
    let mut s = state();
    let m = |name, value, field| {
        let mut m = Metric::integer(name, value, "live", None).unwrap();
        m.source_field = Some(String::from(field));
        m.extensions = smart(false, Some("A")).extensions;
        m
    };
    let e = s.observe(obs(
        0,
        vec![
            m("storage.nvme.available_spare_percent", 9, "available_spare"),
            m(
                "storage.nvme.available_spare_threshold_percent",
                10,
                "available_spare_threshold",
            ),
        ],
    ));
    let finding = e
        .iter()
        .find(|f| f.rule_id == "nvme.spare_below_threshold")
        .unwrap();
    assert_eq!(finding.evidence["available_spare_percent"], "9");
    assert_eq!(finding.evidence["threshold_percent"], "10");
    for (key, field) in [
        ("available_spare_percent", "available_spare"),
        ("threshold_percent", "available_spare_threshold"),
    ] {
        let input = &finding.evidence["inputs"][key];
        assert_eq!(input["source_field"], field);
        assert_eq!(input["smartctl_exit_status"], 8);
        assert_eq!(input["smartctl_exit_status_class"], "device_report");
        assert_eq!(input["device_identity_confidence"], "reported_serial");
        assert!(input.get("device_identity").is_none());
    }
}
