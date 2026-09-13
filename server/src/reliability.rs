//! Pure, deterministic drive reliability assessment. No native acquisition or I/O.
use crate::cider_wire::{Decimal, Metric};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const POLICY_VERSION: &str = "1";
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub source_id: String,
    pub node_id: String,
    pub object_id: String,
    pub resource_id: String,
    pub collector: String,
    pub scope: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub collection_id: String,
    pub boot_id: String,
    pub agent_generation: String,
    pub agent_session_id: String,
    pub clock_id: String,
    pub source_generation: String,
    pub source_version: String,
    pub adapter_version: String,
    pub finished_monotonic_ns: Decimal,
    pub observed_at: String,
    pub received_at_ms: i64,
    pub age_at_receipt_seconds: f64,
    pub stale_after_seconds: f64,
    pub status: String,
    pub metrics: Vec<Metric>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub finding_id: String,
    pub source_id: String,
    pub node_id: String,
    pub object_id: String,
    pub resource_id: String,
    pub rule_id: String,
    pub dimension: String,
    pub classification: String,
    pub severity: String,
    pub status: String,
    pub summary: String,
    pub policy_version: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub opened_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    pub reason: Option<String>,
    pub evidence: Value,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub version: String,
    pub maximum_source_bytes: usize,
    pub maximum_active_sources: usize,
    pub closed_finding_retention_seconds: u64,
    pub baseline_horizon_seconds: f64,
    pub required_baseline_seconds: f64,
    pub required_baseline_intervals: usize,
    pub maximum_baseline_intervals: usize,
    pub percentile: f64,
    pub high_multiplier: f64,
    pub high_excess_ns_per_operation: f64,
    pub recovery_multiplier: f64,
    pub recovery_excess_ns_per_operation: f64,
    pub opening_seconds: f64,
    pub recovery_seconds: f64,
    pub minimum_interval_seconds: f64,
    pub maximum_interval_seconds: f64,
    pub minimum_operations: u64,
    pub required_clear_observations: u8,
    pub endurance_used_threshold_percent: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            version: POLICY_VERSION.into(),
            maximum_source_bytes: 512 * 1024,
            maximum_active_sources: 8192,
            closed_finding_retention_seconds: 30 * 86400,
            baseline_horizon_seconds: 3600.,
            required_baseline_seconds: 600.,
            required_baseline_intervals: 60,
            maximum_baseline_intervals: 720,
            percentile: 0.95,
            high_multiplier: 3.,
            high_excess_ns_per_operation: 1_000_000.,
            recovery_multiplier: 2.,
            recovery_excess_ns_per_operation: 500_000.,
            opening_seconds: 120.,
            recovery_seconds: 60.,
            minimum_interval_seconds: 1.,
            maximum_interval_seconds: 15.,
            minimum_operations: 20,
            required_clear_observations: 2,
            endurance_used_threshold_percent: 100,
        }
    }
}
use std::collections::BTreeMap;
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Stamp {
    observed_at: String,
    received_at_ms: i64,
    age: f64,
    stale: f64,
    usable: bool,
}
impl Stamp {
    fn from(o: &Observation, usable: bool) -> Self {
        Self {
            observed_at: o.observed_at.clone(),
            received_at_ms: o.received_at_ms,
            age: o.age_at_receipt_seconds,
            stale: o.stale_after_seconds,
            usable,
        }
    }
    fn value(&self, now: i64, online: bool) -> Value {
        let age = self.age + (now - self.received_at_ms).max(0) as f64 / 1000.;
        let state = if !self.usable {
            "unavailable"
        } else if !online || !age.is_finite() || age < 0. || age > self.stale {
            "stale"
        } else {
            "current"
        };
        let reason = if !self.usable {
            Some("missing_or_ineligible_evidence")
        } else if !online {
            Some("owner_or_source_unavailable")
        } else if state == "stale" {
            Some("observation_stale")
        } else {
            None
        };
        json!({"state":state,"reason":reason,"age_seconds":age,"stale_after_seconds":self.stale,"observed_at":self.observed_at,"received_at":date(self.received_at_ms)})
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Counter {
    value: Decimal,
    epoch: Option<String>,
    collection_id: String,
    observed_at: String,
    monotonic_ns: Decimal,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Signal {
    dimension: String,
    state: String,
    severity: String,
    reason: Option<String>,
    evidence: Value,
    stamp: Option<Stamp>,
    finding: Option<Finding>,
    clear_count: u8,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct PerfEndpoint {
    at: Decimal,
    bytes: Decimal,
    ops: Decimal,
    time: Decimal,
    epoch: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Interval {
    end: f64,
    duration: f64,
    bucket: (u8, u8),
    ns_per_op: f64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Performance {
    previous: Option<PerfEndpoint>,
    ring: Vec<Interval>,
    reference: Option<f64>,
    bucket: Option<(u8, u8)>,
    elevated: f64,
    recovery: f64,
    first_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceState {
    pub source: SourceIdentity,
    pub active: bool,
    continuity: Option<Vec<String>>,
    last_collection: Option<String>,
    last_mono: Option<Decimal>,
    device_identity: Option<String>,
    device_identity_confidence: String,
    signals: BTreeMap<String, Signal>,
    counters: BTreeMap<String, Counter>,
    performance: BTreeMap<String, Performance>,
    last_stamp: Option<Stamp>,
}
fn unknown_observation() -> Value {
    json!({"state":"unknown","reason":"no_observation","age_seconds":null,"stale_after_seconds":null,"observed_at":null,"received_at":null})
}
fn date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or_default()
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn integer(m: &Metric) -> Option<u128> {
    let v = m.value.as_ref()?;
    v.as_str()
        .and_then(|s| s.parse().ok())
        .or_else(|| v.as_u64().map(u128::from))
}
fn metric<'a>(o: &'a Observation, name: &str) -> Option<&'a Metric> {
    let mut ms = o.metrics.iter().filter(|m| m.name == name);
    let m = ms.next()?;
    if ms.next().is_some()
        || m.availability != "available"
        || m.freshness.as_deref() != Some("live")
    {
        None
    } else {
        Some(m)
    }
}
// Preserve only the fixed, bounded provenance fields. Raw serial/WWN data never enters findings.
fn provenance(metric: &Metric, evidence: &mut Value) {
    if let Some(field) = &metric.source_field {
        evidence["source_field"] = json!(field);
    }
    for key in [
        "device_identity_confidence",
        "smartctl_exit_status",
        "smartctl_exit_status_class",
    ] {
        if let Some(value) = metric.extensions.as_ref().and_then(|e| e.get(key)) {
            evidence[key] = value.clone();
        }
    }
}
impl SourceState {
    pub fn new(source: SourceIdentity) -> Self {
        Self {
            source,
            active: true,
            continuity: None,
            last_collection: None,
            last_mono: None,
            device_identity: None,
            device_identity_confidence: "unavailable".into(),
            signals: BTreeMap::new(),
            counters: BTreeMap::new(),
            performance: BTreeMap::new(),
            last_stamp: None,
        }
    }
    pub fn interrupt(&mut self, reason: &str, now_ms: i64) -> Vec<Finding> {
        let mut out = vec![];
        for s in self.signals.values_mut() {
            if let Some(mut f) = s.finding.take() {
                f.status = "interrupted".into();
                f.reason = Some(reason.into());
                f.ended_at = Some(date(now_ms));
                f.updated_at = date(now_ms);
                out.push(f);
            }
            s.clear_count = 0;
            s.state = "unknown".into();
            s.reason = Some(reason.into());
            if let Some(st) = s.stamp.as_mut() {
                st.usable = false;
            }
        }
        self.counters.clear();
        self.performance.clear();
        if let Some(stamp) = self.last_stamp.as_mut() {
            stamp.usable = false;
        }
        self.active = false;
        out
    }
    fn evidence_context(&self, o: &Observation, evidence: &mut Value) {
        evidence["collection_id"] = json!(o.collection_id);
        evidence["observed_at"] = json!(o.observed_at);
        evidence["received_at"] = json!(date(o.received_at_ms));
        evidence["scope"] = json!(self.source.scope);
        evidence["device_identity_confidence"] = json!(self.device_identity_confidence);
    }
    fn apply(
        &mut self,
        o: &Observation,
        rule: &str,
        dimension: &str,
        class: &str,
        severity: &str,
        summary: &str,
        condition: Option<bool>,
        mut evidence: Value,
        out: &mut Vec<Finding>,
    ) {
        self.evidence_context(o, &mut evidence);
        if dimension == "media_health" {
            evidence["clear_observations_required"] =
                json!(Policy::default().required_clear_observations);
        }
        let s = self.signals.entry(rule.into()).or_default();
        s.dimension = dimension.into();
        s.severity = severity.into();
        s.stamp = Some(Stamp::from(o, condition.is_some()));
        let Some(bad) = condition else {
            s.clear_count = 0;
            s.state = "unknown".into();
            s.reason = Some("missing_or_ineligible_evidence".into());
            return;
        };
        s.reason = None;
        s.evidence = evidence.clone();
        if bad {
            s.clear_count = 0;
            s.state = "warning".into();
            let f = s.finding.get_or_insert_with(|| Finding {
                finding_id: uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_OID,
                    format!("{}:{rule}:{}", self.source.source_id, o.collection_id).as_bytes(),
                )
                .to_string(),
                source_id: self.source.source_id.clone(),
                node_id: self.source.node_id.clone(),
                object_id: self.source.object_id.clone(),
                resource_id: self.source.resource_id.clone(),
                rule_id: rule.into(),
                dimension: dimension.into(),
                classification: class.into(),
                severity: severity.into(),
                status: "open".into(),
                summary: summary.into(),
                policy_version: POLICY_VERSION.into(),
                first_seen_at: o.observed_at.clone(),
                last_seen_at: o.observed_at.clone(),
                opened_at: o.observed_at.clone(),
                updated_at: date(o.received_at_ms),
                ended_at: None,
                reason: None,
                evidence: Value::Null,
            });
            f.last_seen_at = o.observed_at.clone();
            f.updated_at = date(o.received_at_ms);
            f.evidence = evidence;
            out.push(f.clone());
        } else if let Some(f) = s.finding.as_mut() {
            s.clear_count += 1;
            s.state = "recovering".into();
            if s.clear_count >= Policy::default().required_clear_observations {
                let mut f = f.clone();
                f.status = "resolved".into();
                f.updated_at = date(o.received_at_ms);
                f.ended_at = Some(o.observed_at.clone());
                f.reason = Some("two_distinct_usable_clear_observations".into());
                f.evidence = json!({"last_concern":f.evidence,"clear":evidence});
                out.push(f);
                s.finding = None;
                s.clear_count = 0;
                s.state = "no_current_warning".into();
            }
        } else {
            s.state = "no_current_warning".into();
        }
    }
    fn reset_rule(&mut self, rule: &str, o: &Observation, out: &mut Vec<Finding>) {
        if let Some(s) = self.signals.get_mut(rule) {
            if let Some(mut f) = s.finding.take() {
                f.status = "interrupted".into();
                f.reason = Some("counter_epoch_or_value_reset".into());
                f.ended_at = Some(o.observed_at.clone());
                f.updated_at = date(o.received_at_ms);
                out.push(f);
            }
            s.clear_count = 0;
        }
    }
    pub fn observe(&mut self, o: Observation) -> Vec<Finding> {
        let continuity = vec![
            o.boot_id.clone(),
            o.agent_generation.clone(),
            o.agent_session_id.clone(),
            o.clock_id.clone(),
            o.source_generation.clone(),
            o.source_version.clone(),
            o.adapter_version.clone(),
        ];
        let changed = self.continuity.as_ref().is_some_and(|c| c != &continuity);
        if !changed
            && (self.last_collection.as_deref() == Some(&o.collection_id)
                || self.last_mono.is_some_and(|n| o.finished_monotonic_ns <= n))
        {
            return vec![];
        }
        let mut out = if changed {
            self.interrupt("source_continuity_changed", o.received_at_ms)
        } else {
            vec![]
        };
        let mut fresh = matches!(o.status.as_str(), "ok" | "partial")
            && o.age_at_receipt_seconds.is_finite()
            && o.age_at_receipt_seconds >= 0.
            && o.stale_after_seconds.is_finite()
            && o.stale_after_seconds > 0.
            && o.age_at_receipt_seconds <= o.stale_after_seconds;
        if fresh
            && o.metrics.iter().any(|m| {
                m.availability == "available"
                    && m.freshness.as_deref() == Some("live")
                    && m.value.is_some()
            })
            && (self.source.collector.contains("smart")
                || o.metrics.iter().any(|m| {
                    m.name.starts_with("storage.ata.")
                        || m.name.starts_with("storage.nvme.")
                        || m.name == "storage.media.smart_passed"
                }))
        {
            let identities: std::collections::BTreeSet<_> = o
                .metrics
                .iter()
                .filter(|m| {
                    m.availability == "available"
                        && m.freshness.as_deref() == Some("live")
                        && m.value.is_some()
                })
                .filter_map(|m| m.extensions.as_ref()?.get("device_identity")?.as_str())
                .collect();
            if identities.len() > 1 {
                fresh = false;
            }
            let identity = if identities.len() == 1 {
                identities.first().map(|s| s.to_string())
            } else {
                None
            };
            if fresh && self.last_collection.is_some() && self.device_identity != identity {
                out.extend(self.interrupt("device_identity_changed", o.received_at_ms));
            }
            if fresh {
                self.device_identity = identity;
                self.device_identity_confidence = o
                    .metrics
                    .iter()
                    .filter_map(|m| {
                        m.extensions
                            .as_ref()?
                            .get("device_identity_confidence")?
                            .as_str()
                    })
                    .next()
                    .unwrap_or("caller_epoch_only")
                    .into();
            }
        }
        self.continuity = Some(continuity);
        self.last_collection = Some(o.collection_id.clone());
        self.last_mono = Some(o.finished_monotonic_ns);
        self.last_stamp = Some(Stamp::from(&o, fresh));
        self.active = true;
        let direct = [
            (
                "storage.media.smart_passed",
                "smart.overall",
                "critical",
                "Reported SMART overall health failure",
            ),
            (
                "storage.nvme.critical_warning_bits",
                "nvme.critical_warning",
                "critical",
                "Reported NVMe critical warning",
            ),
            (
                "storage.ata.current_pending_sectors",
                "ata.pending_sectors",
                "warning",
                "Reported pending sectors present",
            ),
            (
                "storage.ata.offline_uncorrectable_sectors",
                "ata.uncorrectable_sectors",
                "warning",
                "Reported offline uncorrectable sectors present",
            ),
            (
                "storage.nvme.endurance_used_percent",
                "nvme.endurance_used",
                "warning",
                "Reported endurance estimate at or above 100 percent",
            ),
        ];
        for (name, rule, severity, text) in direct {
            let m = if fresh { metric(&o, name) } else { None };
            let value = m.and_then(|m| m.value.clone());
            let condition = if name.ends_with("smart_passed") {
                value.as_ref().and_then(Value::as_bool).map(|b| !b)
            } else {
                m.and_then(integer).map(|n| {
                    if name.ends_with("endurance_used_percent") {
                        n >= u128::from(Policy::default().endurance_used_threshold_percent)
                    } else {
                        n > 0
                    }
                })
            };
            let mut ev = json!({"metric":name,"value":value,"device_identity_confidence":self.device_identity_confidence,"scope":self.source.scope});
            if name.ends_with("endurance_used_percent") {
                ev["threshold"] = json!(
                    Policy::default()
                        .endurance_used_threshold_percent
                        .to_string()
                );
                ev["comparison"] = json!("greater_than_or_equal");
            } else if !name.ends_with("smart_passed") {
                ev["threshold"] = json!("0");
                ev["comparison"] = json!("greater_than");
            }
            if let Some(m) = m {
                for key in ["smartctl_exit_status", "smartctl_exit_status_class"] {
                    if let Some(value) = m.extensions.as_ref().and_then(|e| e.get(key)) {
                        ev[key] = value.clone();
                    }
                }
                if let Some(field) = &m.source_field {
                    ev["source_field"] = json!(field);
                }
            }
            if name.ends_with("critical_warning_bits") {
                if let Some(bits) = m.and_then(integer) {
                    let mut labels = vec![];
                    for (bit, label) in [
                        (1, "spare_depleted"),
                        (2, "temperature"),
                        (4, "degraded_reliability"),
                        (8, "read_only_media"),
                        (16, "volatile_memory_backup_failure"),
                        (32, "persistent_memory_region_unreliable"),
                    ] {
                        if bits & bit != 0 {
                            labels.push(label);
                        }
                    }
                    if bits & !63 != 0 {
                        labels.push("unknown_warning_bit");
                    }
                    ev["warnings"] = json!(labels);
                }
            }
            self.apply(
                &o,
                rule,
                "media_health",
                if rule == "nvme.endurance_used" {
                    "replacement_planning"
                } else {
                    "reported_health_condition"
                },
                severity,
                text,
                condition,
                ev,
                &mut out,
            );
        }
        let spare_metric = if fresh {
            metric(&o, "storage.nvme.available_spare_percent")
        } else {
            None
        };
        let threshold_metric = if fresh {
            metric(&o, "storage.nvme.available_spare_threshold_percent")
        } else {
            None
        };
        let spare = spare_metric.and_then(integer);
        let threshold = threshold_metric.and_then(integer);
        let mut inputs = json!({});
        for (key, reading) in [
            ("available_spare_percent", spare_metric),
            ("threshold_percent", threshold_metric),
        ] {
            if let Some(reading) = reading {
                let mut input = json!({"metric":reading.name,"value":reading.value});
                provenance(reading, &mut input);
                inputs[key] = input;
            }
        }
        self.apply(
            &o, "nvme.spare_below_threshold", "media_health", "replacement_planning", "warning",
            "Reported available spare below reported threshold", spare.zip(threshold).map(|(s, t)| s < t),
            json!({"available_spare_percent":spare.map(|x|x.to_string()),"threshold_percent":threshold.map(|x|x.to_string()),"inputs":inputs}), &mut out,
        );
        for (name, rule, class) in [
            (
                "storage.device.read_errors_total",
                "iokit.read_errors",
                "observed_error",
            ),
            (
                "storage.device.write_errors_total",
                "iokit.write_errors",
                "observed_error",
            ),
            (
                "storage.device.read_retries_total",
                "iokit.read_retries",
                "deterioration",
            ),
            (
                "storage.device.write_retries_total",
                "iokit.write_retries",
                "deterioration",
            ),
            (
                "storage.nvme.media_errors_total",
                "nvme.media_errors",
                "observed_error",
            ),
            (
                "storage.ata.reallocated_sectors",
                "ata.reallocated_increase",
                "deterioration",
            ),
            (
                "storage.nvme.available_spare_percent",
                "nvme.spare_decline",
                "deterioration",
            ),
        ] {
            let m = if fresh { metric(&o, name) } else { None };
            let current = m.and_then(integer);
            let mut condition = None;
            let mut ev = json!({"metric":name});
            if let (Some(m), Some(value)) = (m, current) {
                provenance(m, &mut ev);
                let previous = self.counters.insert(
                    name.into(),
                    Counter {
                        value: value.into(),
                        epoch: m.counter_epoch.clone(),
                        collection_id: o.collection_id.clone(),
                        observed_at: o.observed_at.clone(),
                        monotonic_ns: o.finished_monotonic_ns,
                    },
                );
                ev["value"] = json!(value.to_string());
                ev["counter_epoch"] = json!(m.counter_epoch);
                ev["monotonic_end_ns"] = json!(o.finished_monotonic_ns);
                if let Some(p) = previous {
                    ev["previous_collection_id"] = json!(p.collection_id);
                    ev["previous_observed_at"] = json!(p.observed_at);
                    ev["monotonic_start_ns"] = json!(p.monotonic_ns);
                    let decrease = rule == "nvme.spare_decline";
                    if p.epoch != m.counter_epoch || (!decrease && value < p.value.get()) {
                        self.reset_rule(rule, &o, &mut out);
                        ev["comparison"] = json!("reset");
                    } else {
                        let delta = if decrease {
                            p.value.get().saturating_sub(value)
                        } else {
                            value - p.value.get()
                        };
                        condition = Some(delta > 0);
                        ev["previous"] = json!(p.value);
                        ev["delta"] = json!(delta.to_string());
                    }
                } else {
                    ev["comparison"] = json!("preexisting_history");
                }
            }
            self.apply(
                &o,
                rule,
                "media_health",
                class,
                "warning",
                "New adverse storage counter change",
                condition,
                ev.clone(),
                &mut out,
            );
            if condition.is_none() && current.is_some() {
                if let Some(s) = self.signals.get_mut(rule) {
                    s.state = "historical_or_initial_observation".into();
                    s.reason = Some("initial_history_or_counter_reset".into());
                    s.evidence = ev;
                    s.stamp = Some(Stamp::from(&o, true));
                }
            }
        }
        for direction in ["read", "write"] {
            self.observe_performance(&o, direction, fresh, &mut out);
        }
        out
    }
    fn observe_performance(
        &mut self,
        o: &Observation,
        direction: &str,
        fresh: bool,
        out: &mut Vec<Finding>,
    ) {
        let policy = Policy::default();
        let rule = format!("iokit.{direction}_service_time");
        let mut p = self.performance.remove(direction).unwrap_or_default();
        let now = o.finished_monotonic_ns.get() as f64 / 1e9;
        p.ring
            .retain(|i| i.end > now - policy.baseline_horizon_seconds && i.end <= now);
        let endpoint = if fresh {
            (|| {
                let b = metric(o, &format!("storage.device.{direction}_bytes_total"))?;
                let ops = metric(o, &format!("storage.device.{direction}_operations_total"))?;
                let time = metric(
                    o,
                    &format!("storage.device.{direction}_accounted_time_nanoseconds_total"),
                )?;
                let epoch = b.counter_epoch.as_ref()?;
                if ops.counter_epoch.as_ref() != Some(epoch)
                    || time.counter_epoch.as_ref() != Some(epoch)
                {
                    return None;
                }
                Some(PerfEndpoint {
                    at: o.finished_monotonic_ns,
                    bytes: integer(b)?.into(),
                    ops: integer(ops)?.into(),
                    time: integer(time)?.into(),
                    epoch: epoch.clone(),
                })
            })()
        } else {
            None
        };
        let previous = p.previous.take();
        p.previous = endpoint.clone();
        let mut reset = false;
        let interval = previous.zip(endpoint).and_then(|(a, b)| {
            if a.epoch != b.epoch || b.bytes < a.bytes || b.ops < a.ops || b.time < a.time {
                reset = true;
                return None;
            }
            let duration_ns = b.at.get().checked_sub(a.at.get())?;
            let duration = duration_ns as f64 / 1e9;
            let ops = b.ops.get() - a.ops.get();
            let bytes = b.bytes.get() - a.bytes.get();
            let time = b.time.get() - a.time.get();
            if !(policy.minimum_interval_seconds..=policy.maximum_interval_seconds).contains(&duration)
                || ops < u128::from(policy.minimum_operations) || time == 0 {
                return None;
            }
            // Classify the exact ratios before converting a service-time value to f64.
            // An overflowing product exceeds every representable byte delta.
            let fits = |limit| ops.checked_mul(limit).is_none_or(|bound| bytes <= bound);
            let transfer_bucket = if fits(4096) { 0 } else if fits(16384) { 1 } else if fits(65536) { 2 } else { 3 };
            // The 1-15 second interval bound keeps every right-hand product exact.
            // Saturation on the left can only select the final capped band.
            let scaled_ops = ops.saturating_mul(1_000_000_000);
            let rate_bucket = (1u8..=63)
                .take_while(|band| scaled_ops >= duration_ns * (1u128 << *band))
                .last().unwrap_or(0);
            Some((Interval { end: now, duration, bucket: (transfer_bucket, rate_bucket), ns_per_op: time as f64 / ops as f64 },
                json!({"bytes_delta":bytes.to_string(),"operations_delta":ops.to_string(),"accounted_time_delta_ns":time.to_string(),"interval_seconds":duration,"counter_epoch":b.epoch,"monotonic_start_ns":a.at,"monotonic_end_ns":b.at})))
        });
        if reset {
            self.reset_rule(&rule, o, out);
            p.ring.clear();
            p.reference = None;
            p.bucket = None;
        }
        let open = self.signals.get(&rule).is_some_and(|s| s.finding.is_some());
        let Some((interval, mut evidence)) = interval else {
            p.elevated = 0.;
            p.recovery = 0.;
            p.first_at = None;
            if !open {
                p.reference = None;
                p.bucket = None;
            }
            self.apply(
                o,
                &rule,
                "performance",
                "service_time_degradation",
                "warning",
                "Driver-accounted service-time degradation",
                None,
                json!({}),
                out,
            );
            self.performance.insert(direction.into(), p);
            return;
        };
        self.evidence_context(o, &mut evidence);
        let matching: Vec<_> = p
            .ring
            .iter()
            .filter(|i| i.bucket == interval.bucket)
            .collect();
        let covered: f64 = matching
            .iter()
            .map(|i| {
                i.duration
                    .min((i.end - (now - policy.baseline_horizon_seconds)).max(0.))
            })
            .sum();
        let count = matching.len();
        let mut reference = p.reference;
        if reference.is_none()
            && covered >= policy.required_baseline_seconds
            && count >= policy.required_baseline_intervals
        {
            let mut sorted = matching.clone();
            sorted.sort_by(|a, b| a.ns_per_op.total_cmp(&b.ns_per_op));
            let target = covered * policy.percentile;
            let mut accum = 0.;
            for i in sorted {
                accum += i
                    .duration
                    .min((i.end - (now - policy.baseline_horizon_seconds)).max(0.));
                if accum >= target {
                    reference = Some(i.ns_per_op);
                    break;
                }
            }
        }
        let comparable = p.bucket.is_none_or(|b| b == interval.bucket);
        evidence["workload_bucket"] = json!({"transfer_size_band":interval.bucket.0,"operation_rate_power_of_two_band":interval.bucket.1});
        evidence["ns_per_operation"] = json!(interval.ns_per_op);
        evidence["reference_ns_per_operation"] = json!(reference);
        evidence["baseline_covered_seconds"] = json!(covered);
        evidence["baseline_interval_count"] = json!(count);
        evidence["required_baseline_seconds"] = json!(policy.required_baseline_seconds);
        evidence["required_baseline_intervals"] = json!(policy.required_baseline_intervals);
        evidence["opening_seconds_required"] = json!(policy.opening_seconds);
        evidence["recovery_seconds_required"] = json!(policy.recovery_seconds);
        evidence["label"] = json!("driver-accounted service-time degradation");
        evidence["possible_causes"] = json!(["workload", "caching", "controller", "media"]);
        let mut train = false;
        let mut state = "warming_up";
        if !comparable {
            p.elevated = 0.;
            p.recovery = 0.;
            p.first_at = None;
            if !open {
                p.reference = None;
                p.bucket = None;
            }
            state = "workload_not_comparable";
        } else if let Some(b) = reference {
            let high = (policy.high_multiplier * b).max(b + policy.high_excess_ns_per_operation);
            let recovery =
                (policy.recovery_multiplier * b).max(b + policy.recovery_excess_ns_per_operation);
            evidence["high_threshold_ns_per_operation"] = json!(high);
            evidence["recovery_threshold_ns_per_operation"] = json!(recovery);
            if interval.ns_per_op > high {
                p.reference = Some(b);
                p.bucket = Some(interval.bucket);
                p.recovery = 0.;
                p.elevated += interval.duration;
                p.first_at.get_or_insert_with(|| o.observed_at.clone());
                state = "pending";
                evidence["elevated_seconds"] = json!(p.elevated);
                if open || p.elevated >= policy.opening_seconds {
                    self.apply(
                        o,
                        &rule,
                        "performance",
                        "service_time_degradation",
                        "warning",
                        "Driver-accounted service-time degradation",
                        Some(true),
                        evidence.clone(),
                        out,
                    );
                    if let Some(f) = self.signals.get_mut(&rule).and_then(|s| s.finding.as_mut()) {
                        if !open {
                            f.first_seen_at =
                                p.first_at.clone().unwrap_or_else(|| o.observed_at.clone());
                        }
                        if let Some(last) = out.last_mut() {
                            *last = f.clone();
                        }
                    }
                    state = "warning";
                }
            } else if open {
                p.elevated = 0.;
                state = "warning";
                if interval.ns_per_op <= recovery {
                    p.recovery += interval.duration;
                    state = "recovering";
                } else {
                    p.recovery = 0.;
                }
                evidence["recovery_seconds"] = json!(p.recovery);
                if p.recovery >= policy.recovery_seconds {
                    if let Some(mut f) = self.signals.get_mut(&rule).and_then(|s| s.finding.take())
                    {
                        f.status = "resolved".into();
                        f.ended_at = Some(o.observed_at.clone());
                        f.updated_at = date(o.received_at_ms);
                        f.reason = Some("sustained_comparable_service_time_recovery".into());
                        f.evidence = evidence.clone();
                        out.push(f);
                    }
                    p.reference = None;
                    p.bucket = None;
                    p.recovery = 0.;
                    p.first_at = None;
                    state = "no_current_warning";
                }
            } else {
                let was_pending = p.reference.is_some();
                p.reference = None;
                p.bucket = None;
                p.elevated = 0.;
                p.recovery = 0.;
                p.first_at = None;
                state = "no_current_warning";
                train = !was_pending;
            }
        } else {
            train = true;
        }
        if train {
            p.ring.push(interval);
            if p.ring.len() > policy.maximum_baseline_intervals {
                p.ring
                    .drain(0..p.ring.len() - policy.maximum_baseline_intervals);
            }
        }
        let signal = self.signals.entry(rule).or_default();
        signal.dimension = "performance".into();
        signal.severity = "warning".into();
        signal.state = state.into();
        signal.reason = if state == "warming_up" {
            Some("insufficient_comparable_baseline".into())
        } else if state == "workload_not_comparable" {
            Some("workload_changed".into())
        } else {
            None
        };
        signal.evidence = evidence;
        // A live endpoint from a different workload does not refresh this rule's prior concern.
        signal.stamp = Some(Stamp::from(o, comparable));
        self.performance.insert(direction.into(), p);
    }
    pub fn summary(&self, now_ms: i64, owner_online: bool) -> Value {
        let signals:Vec<_>=self.signals.iter().map(|(rule,s)|json!({"rule_id":rule,"dimension":s.dimension,"state":s.state,"severity":s.severity,"reason":s.reason,"finding_id":s.finding.as_ref().map(|f|&f.finding_id),"observation":s.stamp.as_ref().map(|s|s.value(now_ms,owner_online&&self.active)).unwrap_or_else(unknown_observation),"evidence":s.evidence})).collect();
        let findings: Vec<_> = self
            .signals
            .values()
            .filter_map(|s| s.finding.as_ref())
            .collect();
        json!({"source_id":self.source.source_id,"node_id":self.source.node_id,"object_id":if self.source.object_id.is_empty(){Value::Null}else{json!(self.source.object_id)},"resource_id":self.source.resource_id,"collector":self.source.collector,"scope":self.source.scope,"active":self.active,"policy_version":POLICY_VERSION,"policy":Policy::default(),"device_identity_confidence":self.device_identity_confidence,"observation":self.last_stamp.as_ref().map(|s|s.value(now_ms,owner_online&&self.active)).unwrap_or_else(unknown_observation),"signals":signals,"findings":findings,"replacement_forecast":{"state":"insufficient_data","estimated_failure_at":null}})
    }
}
