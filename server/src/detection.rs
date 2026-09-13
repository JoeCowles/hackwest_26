//! Deterministic, persistable per-driver activity detection. No collection or I/O.
use crate::cider_wire::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const RULE_ID: &str = "storage.activity.high_rate.v1";
pub const POLICY_VERSION: &str = "1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Policy {
    pub version: String,
    pub baseline_horizon_seconds: f64,
    pub required_baseline_seconds: f64,
    pub required_baseline_intervals: usize,
    pub maximum_baseline_intervals: usize,
    pub percentile: f64,
    pub high_multiplier: f64,
    pub high_excess_bytes_per_second: f64,
    pub recovery_multiplier: f64,
    pub recovery_excess_bytes_per_second: f64,
    pub opening_seconds: f64,
    pub recovery_seconds: f64,
    pub minimum_interval_seconds: f64,
    pub maximum_interval_seconds: f64,
    pub maximum_baseline_bytes: usize,
    pub maximum_source_bytes: usize,
    pub maximum_active_sources: usize,
    pub closed_finding_retention_seconds: u64,
}
impl Default for Policy {
    fn default() -> Self {
        Self {
            version: POLICY_VERSION.into(),
            baseline_horizon_seconds: 1800.,
            required_baseline_seconds: 600.,
            required_baseline_intervals: 60,
            maximum_baseline_intervals: 1800,
            percentile: 0.95,
            high_multiplier: 3.,
            high_excess_bytes_per_second: 1_000_000.,
            recovery_multiplier: 2.,
            recovery_excess_bytes_per_second: 500_000.,
            opening_seconds: 120.,
            recovery_seconds: 60.,
            minimum_interval_seconds: 1.,
            maximum_interval_seconds: 15.,
            maximum_baseline_bytes: 256 * 1024,
            maximum_source_bytes: 512 * 1024,
            maximum_active_sources: 8192,
            closed_finding_retention_seconds: 30 * 86400,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceIdentity {
    pub source_id: String,
    pub node_id: String,
    pub object_id: String,
    pub resource_id: String,
    pub direction: String,
    pub metric: String,
    pub collector: String,
    pub scope: String,
    pub attributes: BTreeMap<String, serde_json::Value>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Continuity {
    pub boot_id: String,
    pub agent_generation: String,
    pub agent_session_id: String,
    pub clock_id: String,
    pub counter_epoch: String,
    pub source_version: String,
    pub adapter_version: String,
    pub policy_version: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Observation {
    pub collection_id: String,
    pub continuity: Continuity,
    pub finished_monotonic_ns: Decimal,
    pub observed_at: String,
    pub received_at_ms: i64,
    pub counter: Option<Decimal>,
    pub unavailable_reason: Option<String>,
    pub age_at_receipt_seconds: f64,
    pub stale_after_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BaselineSummary {
    pub state: String,
    pub covered_seconds: f64,
    pub interval_count: usize,
    pub required_seconds: f64,
    pub required_intervals: usize,
    pub reference_rate_bytes_per_second: Option<f64>,
    pub high_threshold_bytes_per_second: Option<f64>,
    pub recovery_threshold_bytes_per_second: Option<f64>,
    pub first_observed_at: Option<String>,
    pub last_observed_at: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpisodeSummary {
    pub state: String,
    pub finding_id: Option<String>,
    pub elevated_seconds: f64,
    pub recovery_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObservationSummary {
    pub state: String,
    pub reason: Option<String>,
    pub observed_at: Option<String>,
    pub received_at: Option<String>,
    pub age_seconds: Option<f64>,
    pub stale_after_seconds: f64,
    pub rate_bytes_per_second: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceSummary {
    pub source_id: String,
    pub node_id: String,
    pub object_id: String,
    pub resource_id: String,
    pub direction: String,
    pub metric: String,
    pub rule_id: String,
    pub policy_version: String,
    pub baseline_revision: String,
    pub active: bool,
    pub support_state: String,
    pub baseline: BaselineSummary,
    pub episode: EpisodeSummary,
    pub observation: ObservationSummary,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntervalEvidence {
    pub collection_id: String,
    pub previous_collection_id: String,
    pub counter_start: Decimal,
    pub counter_end: Decimal,
    pub monotonic_start_ns: Decimal,
    pub monotonic_end_ns: Decimal,
    pub interval_seconds: f64,
    pub rate_bytes_per_second: f64,
    pub observed_at: String,
    pub received_at: String,
    pub continuity: Continuity,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FindingEvidence {
    pub first: IntervalEvidence,
    pub latest: IntervalEvidence,
    pub peak: IntervalEvidence,
    pub elevated_seconds: f64,
    pub recovery_seconds: f64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Finding {
    pub finding_id: String,
    pub source_id: String,
    pub node_id: String,
    pub object_id: String,
    pub resource_id: String,
    pub direction: String,
    pub rule_id: String,
    pub status: String,
    pub severity: String,
    pub summary: String,
    pub first_seen_at: String,
    pub last_seen_at: String,
    pub opened_at: String,
    pub updated_at: String,
    pub ended_at: Option<String>,
    pub reason: Option<String>,
    pub baseline_revision: String,
    pub policy: Policy,
    pub baseline: BaselineSummary,
    pub evidence: FindingEvidence,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct BaselineInterval {
    start: Decimal,
    end: Decimal,
    rate: f64,
    observed_ms: i64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Episode {
    baseline: BaselineSummary,
    evidence: FindingEvidence,
    finding: Option<Finding>,
    elevated_ns: Decimal,
    recovery_ns: Decimal,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SourceState {
    pub source: SourceIdentity,
    pub baseline_revision: u64,
    pub active: bool,
    watermark: Option<Observation>,
    previous: Option<Observation>,
    baseline: Vec<BaselineInterval>,
    #[serde(skip)]
    baseline_bytes: usize,
    episode: Option<Episode>,
    last_rate: Option<f64>,
    unavailable_reason: Option<String>,
}
impl SourceState {
    pub fn new(source: SourceIdentity) -> Self {
        Self {
            source,
            baseline_revision: 1,
            active: true,
            watermark: None,
            previous: None,
            baseline: vec![],
            baseline_bytes: 0,
            episode: None,
            last_rate: None,
            unavailable_reason: Some("no_observations".into()),
        }
    }
    pub fn continuity(&self) -> Option<&Continuity> {
        self.watermark.as_ref().map(|o| &o.continuity)
    }
    /// Only forward acquisition attempts enter this state machine. The receipt
    /// transaction is the outer immutable-ID/replay boundary.
    pub fn observe(&mut self, mut observation: Observation) -> Vec<Finding> {
        let mut changes = vec![];
        let timing_usable = observation.age_at_receipt_seconds.is_finite()
            && observation.age_at_receipt_seconds >= 0.
            && observation.stale_after_seconds.is_finite()
            && observation.stale_after_seconds > 0.
            && observation.age_at_receipt_seconds <= observation.stale_after_seconds.min(15.)
            && parsed_ms(&observation.observed_at).is_some();
        if let Some(last) = &self.watermark {
            if observation.collection_id == last.collection_id {
                return changes;
            }
            if observation.continuity == last.continuity {
                if observation.finished_monotonic_ns <= last.finished_monotonic_ns {
                    return changes;
                }
            } else {
                // A delayed old clock domain must not undo a newer domain. The
                // native adapter also checks the current accepted resource.
                if observation.received_at_ms < last.received_at_ms
                    || parsed_ms(&observation.observed_at)
                        .zip(parsed_ms(&last.observed_at))
                        .is_some_and(|(new, old)| new <= old)
                {
                    return changes;
                }
                if !timing_usable {
                    // An ineligible acquisition cannot prove a clock/identity
                    // transition. Break observation continuity, but preserve the
                    // canonical domain and watermark so its next valid sample
                    // cannot trigger a second, false identity interruption.
                    // In particular, do not age the baseline on this other clock.
                    self.previous = None;
                    self.break_interval(
                        observation
                            .unavailable_reason
                            .as_deref()
                            .unwrap_or("timing_unknown"),
                        observation.received_at_ms,
                        &mut changes,
                    );
                    return changes;
                }
                if let Some(f) = self.interrupt("identity_changed", observation.received_at_ms) {
                    changes.push(f);
                }
                let Some(revision) = self.baseline_revision.checked_add(1) else {
                    self.unavailable_reason = Some("baseline_revision_exhausted".into());
                    return changes;
                };
                self.baseline_revision = revision;
            }
        } else if !timing_usable {
            // With no canonical endpoint, an ineligible attempt cannot establish
            // an identity that a later valid observation would have to replace.
            self.previous = None;
            self.break_interval(
                observation
                    .unavailable_reason
                    .as_deref()
                    .unwrap_or("timing_unknown"),
                observation.received_at_ms,
                &mut changes,
            );
            return changes;
        }
        self.active = true;
        if let Some(ms) = parsed_ms(&observation.observed_at) {
            observation.observed_at = timestamp(ms);
        }
        // Persist JSON-safe unavailable attempts, including pathological adapter
        // input, without admitting their counter into a future interval.
        if !observation.age_at_receipt_seconds.is_finite() {
            observation.age_at_receipt_seconds = 0.;
        }
        if !observation.stale_after_seconds.is_finite() || observation.stale_after_seconds <= 0. {
            observation.stale_after_seconds = 15.;
        }
        let previous = self.previous.take();
        self.watermark = Some(observation.clone());
        self.prune_baseline(observation.finished_monotonic_ns.get());
        let failure = observation
            .unavailable_reason
            .clone()
            .or_else(|| (!timing_usable).then(|| "timing_unknown".into()))
            .or_else(|| {
                observation
                    .counter
                    .is_none()
                    .then(|| "field_unavailable".into())
            })
            .or_else(|| {
                (observation.continuity.counter_epoch.is_empty())
                    .then(|| "counter_epoch_unknown".into())
            })
            .or_else(|| {
                (observation.continuity.policy_version != POLICY_VERSION)
                    .then(|| "policy_version_unknown".into())
            });
        if let Some(reason) = failure {
            self.break_interval(&reason, observation.received_at_ms, &mut changes);
            return changes;
        }
        self.previous = Some(observation.clone());
        let Some(previous) = previous else {
            self.break_interval(
                "insufficient_endpoints",
                observation.received_at_ms,
                &mut changes,
            );
            return changes;
        };
        let elapsed_ns = observation
            .finished_monotonic_ns
            .get()
            .checked_sub(previous.finished_monotonic_ns.get());
        let elapsed_seconds = elapsed_ns.map(|ns| ns as f64 / 1e9).unwrap_or(0.);
        let elapsed_receipt = observation
            .received_at_ms
            .saturating_sub(previous.received_at_ms);
        if !(1. ..=15.).contains(&elapsed_seconds)
            || elapsed_receipt > 15_000
            || elapsed_receipt < 0
        {
            self.break_interval(
                "interval_unusable",
                observation.received_at_ms,
                &mut changes,
            );
            return changes;
        }
        let start = previous.counter.expect("usable endpoint has a counter");
        let end = observation.counter.expect("validated counter");
        let Some(delta) = end.get().checked_sub(start.get()) else {
            self.break_interval(
                "counter_decreased",
                observation.received_at_ms,
                &mut changes,
            );
            return changes;
        };
        let rate = delta as f64 / elapsed_seconds;
        if !rate.is_finite() {
            self.break_interval("rate_nonfinite", observation.received_at_ms, &mut changes);
            return changes;
        }
        self.last_rate = Some(rate);
        self.unavailable_reason = None;
        let interval = IntervalEvidence {
            collection_id: observation.collection_id.clone(),
            previous_collection_id: previous.collection_id,
            counter_start: start,
            counter_end: end,
            monotonic_start_ns: previous.finished_monotonic_ns,
            monotonic_end_ns: observation.finished_monotonic_ns,
            interval_seconds: elapsed_seconds,
            rate_bytes_per_second: rate,
            observed_at: observation.observed_at.clone(),
            received_at: timestamp(observation.received_at_ms),
            continuity: observation.continuity.clone(),
        };
        if let Some(mut episode) = self.episode.take() {
            let high = episode
                .baseline
                .high_threshold_bytes_per_second
                .expect("frozen ready reference");
            let low = episode
                .baseline
                .recovery_threshold_bytes_per_second
                .expect("frozen ready reference");
            if episode.finding.is_none() && rate <= high {
                // Neither the high candidate nor its terminating observation is
                // admitted as ordinary training.
                return changes;
            }
            episode.evidence.latest = interval.clone();
            if rate > episode.evidence.peak.rate_bytes_per_second {
                episode.evidence.peak = interval.clone();
            }
            if rate > high {
                episode.elevated_ns = episode
                    .elevated_ns
                    .get()
                    .saturating_add(elapsed_ns.unwrap())
                    .into();
            }
            if episode.finding.is_some() && rate <= low {
                episode.recovery_ns = episode
                    .recovery_ns
                    .get()
                    .saturating_add(elapsed_ns.unwrap())
                    .into();
            } else {
                episode.recovery_ns = 0u64.into();
            }
            episode.evidence.elevated_seconds = episode.elevated_ns.get() as f64 / 1e9;
            episode.evidence.recovery_seconds = episode.recovery_ns.get() as f64 / 1e9;
            if let Some(mut finding) = episode.finding.take() {
                finding.evidence = episode.evidence.clone();
                finding.last_seen_at = interval.observed_at;
                finding.updated_at = timestamp(observation.received_at_ms);
                if episode.recovery_ns.get() >= 60_000_000_000 {
                    finding.status = "resolved".into();
                    finding.reason = Some("returned_below_recovery_threshold".into());
                    finding.ended_at = Some(timestamp(observation.received_at_ms));
                    changes.push(finding);
                    return changes;
                }
                changes.push(finding.clone());
                episode.finding = Some(finding);
            } else if episode.elevated_ns.get() >= 120_000_000_000 {
                let finding = self.make_finding(&episode, observation.received_at_ms);
                changes.push(finding.clone());
                episode.finding = Some(finding);
            }
            self.episode = Some(episode);
        } else {
            let reference = self.baseline_summary(observation.finished_monotonic_ns.get());
            if reference
                .high_threshold_bytes_per_second
                .is_some_and(|high| rate > high)
            {
                self.episode = Some(Episode {
                    baseline: reference,
                    evidence: FindingEvidence {
                        first: interval.clone(),
                        latest: interval.clone(),
                        peak: interval,
                        elevated_seconds: elapsed_seconds,
                        recovery_seconds: 0.,
                    },
                    finding: None,
                    elevated_ns: elapsed_ns.unwrap().into(),
                    recovery_ns: 0u64.into(),
                });
            } else {
                let training = BaselineInterval {
                    start: previous.finished_monotonic_ns,
                    end: observation.finished_monotonic_ns,
                    rate,
                    observed_ms: parsed_ms(&observation.observed_at).expect("validated date"),
                };
                self.baseline_bytes += encoded_interval_len(&training);
                self.baseline.push(training);
                self.prune_baseline(observation.finished_monotonic_ns.get());
            }
        }
        changes
    }

    fn break_interval(&mut self, reason: &str, now_ms: i64, changes: &mut Vec<Finding>) {
        self.last_rate = None;
        self.unavailable_reason = Some(reason.into());
        if let Some(mut episode) = self.episode.take() {
            if let Some(mut finding) = episode.finding.take() {
                episode.recovery_ns = 0u64.into();
                episode.evidence.recovery_seconds = 0.;
                finding.evidence = episode.evidence.clone();
                finding.updated_at = timestamp(now_ms);
                changes.push(finding.clone());
                episode.finding = Some(finding);
                self.episode = Some(episode);
            }
        }
    }

    pub fn interrupt(&mut self, reason: &str, now_ms: i64) -> Option<Finding> {
        let finding = self.episode.take().and_then(|e| e.finding).map(|mut f| {
            f.status = "interrupted".into();
            f.reason = Some(reason.into());
            f.ended_at = Some(timestamp(now_ms));
            f.updated_at = timestamp(now_ms);
            f
        });
        self.active = false;
        self.baseline.clear();
        self.baseline_bytes = 0;
        self.previous = None;
        self.last_rate = None;
        self.unavailable_reason = Some(reason.into());
        finding
    }

    pub fn rebaseline(&mut self, reason: &str, now_ms: i64) -> Result<Option<Finding>, String> {
        if !matches!(reason, "planned_workload_change" | "operator_reassessment") {
            return Err("unsupported rebaseline reason".into());
        }
        let revision = self
            .baseline_revision
            .checked_add(1)
            .ok_or("baseline revision exhausted")?;
        let was_active = self.active;
        let finding = self.interrupt("administrator_rebaseline", now_ms);
        self.baseline_revision = revision;
        self.active = was_active;
        Ok(finding)
    }

    /// The adapter must also cap immutable source/provenance inputs: clearing
    /// dynamic training cannot make an oversized identity fit the state budget.
    pub fn enforce_size_limit(&mut self, now_ms: i64) -> Option<Finding> {
        if serde_json::to_vec(self)
            .map_or(true, |v| v.len() > Policy::default().maximum_source_bytes)
        {
            self.interrupt("capacity_limited", now_ms)
        } else {
            None
        }
    }

    fn prune_baseline(&mut self, end_ns: u128) {
        if self.baseline_bytes == 0 {
            self.baseline_bytes = self.baseline.iter().map(encoded_interval_len).sum();
        }
        let cutoff = end_ns.saturating_sub(1_800_000_000_000);
        let expired = self.baseline.partition_point(|i| i.end.get() <= cutoff);
        for item in self.baseline.drain(..expired) {
            self.baseline_bytes -= encoded_interval_len(&item);
        }
        if let Some(first) = self.baseline.first_mut() {
            if first.start.get() < cutoff {
                self.baseline_bytes -= encoded_interval_len(first);
                first.start = cutoff.into();
                self.baseline_bytes += encoded_interval_len(first);
            }
        }
        let excess = self.baseline.len().saturating_sub(1800);
        for item in self.baseline.drain(..excess) {
            self.baseline_bytes -= encoded_interval_len(&item);
        }
        // JSON brackets plus comma delimiters are part of the actual ring bound.
        while 2 + self.baseline_bytes + self.baseline.len().saturating_sub(1) > 256 * 1024 {
            if self.baseline.is_empty() {
                break;
            }
            self.baseline_bytes -= encoded_interval_len(&self.baseline.remove(0));
        }
    }

    fn baseline_summary(&self, now_ns: u128) -> BaselineSummary {
        let cutoff = now_ns.saturating_sub(1_800_000_000_000);
        let mut weighted: Vec<_> = self
            .baseline
            .iter()
            .filter_map(|i| {
                let duration = i.end.get().saturating_sub(i.start.get().max(cutoff));
                (duration > 0).then_some((i.rate, duration, i.observed_ms))
            })
            .collect();
        let total: u128 = weighted.iter().map(|i| i.1).sum();
        let count = weighted.len();
        let ready = total >= 600_000_000_000 && count >= 60;
        let first = weighted.iter().map(|i| i.2).min().map(timestamp);
        let last = weighted.iter().map(|i| i.2).max().map(timestamp);
        weighted.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut accumulated = 0u128;
        let reference = if ready {
            weighted.iter().find_map(|i| {
                accumulated += i.1;
                // Exact integer weighting avoids roundoff at the nearest-rank edge.
                (accumulated * 100 >= total * 95).then_some(i.0)
            })
        } else {
            None
        };
        BaselineSummary {
            state: if ready { "ready" } else { "learning" }.into(),
            covered_seconds: total as f64 / 1e9,
            interval_count: count,
            required_seconds: 600.,
            required_intervals: 60,
            reference_rate_bytes_per_second: reference,
            high_threshold_bytes_per_second: reference.map(|b| (3. * b).max(b + 1_000_000.)),
            recovery_threshold_bytes_per_second: reference.map(|b| (2. * b).max(b + 500_000.)),
            first_observed_at: first,
            last_observed_at: last,
        }
    }

    fn make_finding(&self, episode: &Episode, now_ms: i64) -> Finding {
        // Stable across deterministic replay; persistence commits this transition
        // with its accepted collection, so a restart cannot create a second ID.
        let key = serde_json::to_vec(&(
            self.source.source_id.as_str(),
            self.baseline_revision,
            episode.evidence.first.collection_id.as_str(),
        ))
        .expect("serializable identity");
        let finding_id = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, &key).to_string();
        Finding {
            finding_id,
            source_id: self.source.source_id.clone(),
            node_id: self.source.node_id.clone(),
            object_id: self.source.object_id.clone(),
            resource_id: self.source.resource_id.clone(),
            direction: self.source.direction.clone(),
            rule_id: RULE_ID.into(),
            status: "open".into(),
            severity: "warning".into(),
            summary: format!(
                "Sustained elevated driver {} activity compared with this source's observed baseline",
                self.source.direction
            ),
            first_seen_at: episode.evidence.first.observed_at.clone(),
            last_seen_at: episode.evidence.latest.observed_at.clone(),
            opened_at: timestamp(now_ms),
            updated_at: timestamp(now_ms),
            ended_at: None,
            reason: None,
            baseline_revision: self.baseline_revision.to_string(),
            policy: Policy::default(),
            baseline: episode.baseline.clone(),
            evidence: episode.evidence.clone(),
        }
    }

    pub fn summary(&self, now_ms: i64, owner_online: bool) -> SourceSummary {
        let baseline = self
            .episode
            .as_ref()
            .map(|e| e.baseline.clone())
            .unwrap_or_else(|| {
                // On reads, elapsed receipt time ages the monotonic baseline without
                // inventing unobserved intervals or mutating persisted state.
                let now_ns = self
                    .watermark
                    .as_ref()
                    .map(|o| {
                        o.finished_monotonic_ns.get().saturating_add(
                            now_ms.saturating_sub(o.received_at_ms).max(0) as u128 * 1_000_000,
                        )
                    })
                    .unwrap_or(0);
                self.baseline_summary(now_ns)
            });
        let episode = self
            .episode
            .as_ref()
            .map(|e| EpisodeSummary {
                state: if e.finding.is_some() {
                    "open"
                } else {
                    "pending"
                }
                .into(),
                finding_id: e.finding.as_ref().map(|f| f.finding_id.clone()),
                elevated_seconds: e.evidence.elevated_seconds,
                recovery_seconds: e.evidence.recovery_seconds,
            })
            .unwrap_or(EpisodeSummary {
                state: "quiet".into(),
                finding_id: None,
                elevated_seconds: 0.,
                recovery_seconds: 0.,
            });
        let mut observation = ObservationSummary {
            state: "unavailable".into(),
            reason: self.unavailable_reason.clone(),
            observed_at: None,
            received_at: None,
            age_seconds: None,
            stale_after_seconds: 15.,
            rate_bytes_per_second: self.last_rate,
        };
        if let Some(last) = &self.watermark {
            let age = last.age_at_receipt_seconds
                + now_ms.saturating_sub(last.received_at_ms).max(0) as f64 / 1000.;
            observation.observed_at = Some(last.observed_at.clone());
            observation.received_at = Some(timestamp(last.received_at_ms));
            observation.age_seconds = Some(age);
            observation.stale_after_seconds = last.stale_after_seconds.min(15.);
            if !self.active {
                observation.reason = self
                    .unavailable_reason
                    .clone()
                    .or(Some("inactive_source".into()));
            } else if !owner_online
                || now_ms.saturating_sub(last.received_at_ms) as f64 / 1000.
                    > observation.stale_after_seconds
                || (self.unavailable_reason.is_none() && age > observation.stale_after_seconds)
            {
                observation.state = "stale".into();
                observation.reason = Some(
                    if owner_online {
                        "observation_stale"
                    } else {
                        "owner_offline"
                    }
                    .into(),
                );
            } else if self.unavailable_reason.is_none() {
                observation.state = "current".into();
            }
        }
        SourceSummary {
            source_id: self.source.source_id.clone(),
            node_id: self.source.node_id.clone(),
            object_id: self.source.object_id.clone(),
            resource_id: self.source.resource_id.clone(),
            direction: self.source.direction.clone(),
            metric: self.source.metric.clone(),
            rule_id: RULE_ID.into(),
            policy_version: POLICY_VERSION.into(),
            baseline_revision: self.baseline_revision.to_string(),
            active: self.active,
            support_state: "supported".into(),
            baseline,
            episode,
            observation,
        }
    }
}
fn encoded_interval_len(interval: &BaselineInterval) -> usize {
    serde_json::to_vec(interval)
        .expect("finite baseline interval")
        .len()
}
fn timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .unwrap_or(chrono::DateTime::UNIX_EPOCH)
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn parsed_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn state() -> SourceState {
        SourceState::new(SourceIdentity {
            source_id: "00000000-0000-4000-8000-000000000001".into(),
            node_id: "node".into(),
            object_id: "object".into(),
            resource_id: "resource".into(),
            direction: "read".into(),
            metric: "storage.device.read_bytes_total".into(),
            collector: "iokit.block".into(),
            scope: "driver".into(),
            attributes: BTreeMap::new(),
        })
    }
    fn observation(t: u64, counter: u128) -> Observation {
        Observation {
            collection_id: format!("collection-{t}"),
            continuity: Continuity {
                boot_id: "boot".into(),
                agent_generation: "1".into(),
                agent_session_id: "session".into(),
                clock_id: "clock".into(),
                counter_epoch: "epoch".into(),
                source_version: "1".into(),
                adapter_version: "1".into(),
                policy_version: "1".into(),
            },
            finished_monotonic_ns: (u128::from(t) * 1_000_000_000).into(),
            observed_at: chrono::DateTime::from_timestamp(t as i64, 0)
                .unwrap()
                .to_rfc3339(),
            received_at_ms: t as i64 * 1000,
            counter: Some(counter.into()),
            unavailable_reason: None,
            age_at_receipt_seconds: 0.,
            stale_after_seconds: 15.,
        }
    }
    fn train(s: &mut SourceState) {
        for n in 0..=120 {
            s.observe(observation(n * 5, u128::from(n) * 10_000_000));
        }
    }
    #[test]
    fn opens_after_120_high_seconds_and_resolves_after_60_low_seconds() {
        let mut s = state();
        train(&mut s);
        let b = s.summary(600_000, true).baseline;
        assert_eq!(b.state, "ready");
        assert_eq!(b.covered_seconds, 600.);
        assert_eq!(b.reference_rate_bytes_per_second, Some(2_000_000.));
        for n in 1..24 {
            assert!(
                s.observe(observation(
                    600 + n * 5,
                    1_200_000_000 + u128::from(n) * 40_000_000
                ))
                .is_empty()
            );
        }
        assert_eq!(s.summary(715_000, true).episode.state, "pending");
        let open = s.observe(observation(720, 2_160_000_000));
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].status, "open");
        assert_eq!(open[0].evidence.elevated_seconds, 120.);
        assert_eq!(
            open[0].baseline.high_threshold_bytes_per_second,
            Some(6_000_000.)
        );
        for n in 1..=12 {
            let f = s.observe(observation(
                720 + n * 5,
                2_160_000_000 + u128::from(n) * 20_000_000,
            ));
            assert_eq!(f.len(), 1);
            assert_eq!(f[0].finding_id, open[0].finding_id);
            assert_eq!(f[0].status, if n == 12 { "resolved" } else { "open" });
        }
        assert_eq!(s.summary(780_000, true).episode.state, "quiet");
    }
    #[test]
    fn exact_wide_counters_preserve_small_deltas_and_measured_zero() {
        let mut s = state();
        let base = u128::MAX - 100_000;
        s.observe(observation(0, base));
        s.observe(observation(5, base + 15));
        assert_eq!(
            s.summary(5000, true).observation.rate_bytes_per_second,
            Some(3.)
        );
        s.observe(observation(10, base + 15));
        assert_eq!(
            s.summary(10000, true).observation.rate_bytes_per_second,
            Some(0.)
        );
        assert_eq!(s.summary(10000, true).baseline.covered_seconds, 10.);
    }
    #[test]
    fn duration_weighting_and_count_are_both_required() {
        let mut s = state();
        s.observe(observation(0, 0));
        let mut t = 0;
        let mut c = 0;
        // 570 seconds low + 40 one-second high intervals: weighted P95 is high,
        // despite high intervals being a minority of elapsed time.
        for _ in 0..38 {
            t += 15;
            c += 15_000;
            s.observe(observation(t, c));
        }
        for _ in 0..40 {
            t += 1;
            c += 10_000;
            s.observe(observation(t, c));
        }
        let b = s.summary(t as i64 * 1000, true).baseline;
        assert_eq!(b.state, "ready");
        assert_eq!(b.covered_seconds, 610.);
        assert_eq!(b.reference_rate_bytes_per_second, Some(10_000.));
        let mut s = state();
        for n in 0..=40 {
            s.observe(observation(n * 15, u128::from(n) * 15_000));
        }
        assert_eq!(s.summary(600_000, true).baseline.state, "learning");
    }
    #[test]
    fn strict_onset_and_aborted_candidates_do_not_train() {
        let mut s = state();
        train(&mut s);
        s.observe(observation(605, 1_230_000_000)); // exactly H: ordinary
        assert_eq!(s.summary(605_000, true).episode.state, "quiet");
        s.observe(observation(610, 1_270_000_000));
        assert_eq!(s.summary(610_000, true).episode.state, "pending");
        s.observe(observation(615, 1_280_000_000));
        assert_eq!(s.summary(615_000, true).episode.state, "quiet");
        assert_eq!(s.summary(615_000, true).baseline.interval_count, 121);
        s.observe(observation(620, 1_320_000_000));
        assert_eq!(s.summary(620_000, true).episode.elevated_seconds, 5.);
    }
    fn open_state() -> SourceState {
        let mut s = state();
        train(&mut s);
        for n in 1..=24 {
            s.observe(observation(
                600 + n * 5,
                1_200_000_000 + u128::from(n) * 40_000_000,
            ));
        }
        s
    }
    #[test]
    fn unavailable_and_gaps_break_streaks_without_resolving_findings() {
        let mut s = open_state();
        s.observe(observation(725, 2_160_000_000));
        assert_eq!(s.summary(725_000, true).episode.recovery_seconds, 5.);
        let mut failed = observation(730, 2_160_000_000);
        failed.counter = None;
        failed.unavailable_reason = Some("field_unavailable".into());
        let f = s.observe(failed);
        assert_eq!(f[0].status, "open");
        assert_eq!(s.summary(730_000, true).episode.recovery_seconds, 0.);
        assert_eq!(s.summary(730_000, true).observation.state, "unavailable");
        s.observe(observation(735, 2_160_000_000));
        assert_eq!(s.summary(735_000, true).episode.recovery_seconds, 0.);
        s.observe(observation(740, 2_160_000_000));
        s.observe(observation(760, 2_160_000_000));
        assert_eq!(s.summary(760_000, true).episode.recovery_seconds, 0.);
        assert_eq!(s.summary(760_000, true).episode.state, "open");
    }
    #[test]
    fn duplicates_old_observations_and_restart_do_not_advance() {
        let mut s = open_state();
        let before = serde_json::to_string(&s).unwrap();
        assert!(s.observe(observation(720, 2_160_000_000)).is_empty());
        assert!(s.observe(observation(710, 2_080_000_000)).is_empty());
        assert_eq!(serde_json::to_string(&s).unwrap(), before);
        let mut restored: SourceState = serde_json::from_str(&before).unwrap();
        let f = restored.observe(observation(725, 2_200_000_000));
        assert_eq!(
            f[0].finding_id,
            s.summary(720_000, true).episode.finding_id.unwrap()
        );
        assert_eq!(f[0].evidence.elevated_seconds, 125.);
    }
    #[test]
    fn identity_change_interrupts_and_rebaseline_preserves_watermark() {
        let mut s = open_state();
        let mut reboot = observation(1, 10);
        reboot.continuity.boot_id = "new-boot".into();
        reboot.received_at_ms = 725_000;
        reboot.observed_at = observation(725, 0).observed_at;
        let f = s.observe(reboot);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].status, "interrupted");
        assert_eq!(f[0].reason.as_deref(), Some("identity_changed"));
        assert_eq!(s.baseline_revision, 2);
        assert_eq!(s.summary(725_000, true).baseline.covered_seconds, 0.);
        let mut s = open_state();
        let old = observation(720, 2_160_000_000);
        let f = s
            .rebaseline("planned_workload_change", 725_000)
            .unwrap()
            .unwrap();
        assert_eq!(f.reason.as_deref(), Some("administrator_rebaseline"));
        assert_eq!(s.baseline_revision, 2);
        assert!(s.observe(old).is_empty());
        s.observe(observation(725, 2_200_000_000));
        assert_eq!(s.summary(725_000, true).baseline.covered_seconds, 0.);
        s.observe(observation(730, 2_240_000_000));
        assert_eq!(s.summary(730_000, true).baseline.covered_seconds, 5.);
    }
    #[test]
    fn timing_gate_counter_reset_and_offline_are_unknown() {
        let mut s = state();
        s.observe(observation(0, 100));
        let mut late = observation(5, 200);
        late.age_at_receipt_seconds = 15.001;
        s.observe(late);
        assert_eq!(
            s.summary(5000, true).observation.reason.as_deref(),
            Some("timing_unknown")
        );
        assert_eq!(s.summary(5000, true).baseline.covered_seconds, 0.);
        s.observe(observation(10, 200));
        s.observe(observation(15, 100));
        assert_eq!(
            s.summary(15000, true).observation.reason.as_deref(),
            Some("counter_decreased")
        );
        s.observe(observation(20, 150));
        assert_eq!(s.summary(20000, true).observation.state, "current");
        assert_eq!(s.summary(20000, false).observation.state, "stale");
        assert_eq!(s.summary(35001, true).observation.state, "stale");
    }
    #[test]
    fn frozen_reference_survives_horizon_and_recovery_hysteresis() {
        let mut s = open_state();
        let mut c = 2_160_000_000;
        for t in (725..=2600).step_by(5) {
            c += 40_000_000;
            s.observe(observation(t, c));
        }
        let b = s.summary(2_600_000, true).baseline;
        assert_eq!(b.reference_rate_bytes_per_second, Some(2_000_000.));
        assert_eq!(b.covered_seconds, 600.);
        c += 20_000_000;
        s.observe(observation(2605, c));
        assert_eq!(s.summary(2_605_000, true).episode.recovery_seconds, 5.);
        c += 25_000_000;
        s.observe(observation(2610, c));
        assert_eq!(s.summary(2_610_000, true).episode.recovery_seconds, 0.);
        for n in 1..=12 {
            c += 20_000_000;
            s.observe(observation(2610 + n * 5, c));
        }
        assert_eq!(s.summary(2_670_000, true).episode.state, "quiet");
        assert_eq!(s.summary(2_670_000, true).baseline.state, "learning");
    }
    #[test]
    fn baseline_clips_window_and_serialized_state_stays_bounded() {
        let mut s = state();
        for n in 0..=2500 {
            s.observe(observation(n, u128::from(n) * 1000));
        }
        let b = s.summary(2_500_000, true).baseline;
        assert_eq!(b.covered_seconds, 1800.);
        assert_eq!(b.interval_count, 1800);
        assert!(serde_json::to_vec(&s.baseline).unwrap().len() <= 256 * 1024);
        assert!(serde_json::to_vec(&s).unwrap().len() <= 512 * 1024);
        let mut s = state();
        for n in 0..=121 {
            s.observe(observation(n * 15, u128::from(n) * 15_000));
        }
        // A later short interval clips 5 seconds of the oldest retained interval.
        s.observe(observation(1820, 1_820_000));
        assert_eq!(s.summary(1_820_000, true).baseline.covered_seconds, 1800.);
    }

    #[test]
    fn duration_weighted_p95_is_not_sample_count_p95() {
        let mut s = state();
        s.observe(observation(0, 0));
        let mut t = 0;
        let mut c = 0;
        for _ in 0..40 {
            t += 15;
            c += 15_000;
            s.observe(observation(t, c));
        }
        for _ in 0..20 {
            t += 1;
            c += 10_000;
            s.observe(observation(t, c));
        }
        let b = s.summary(620_000, true).baseline;
        assert_eq!(b.state, "ready");
        assert_eq!(b.interval_count, 60);
        assert_eq!(b.reference_rate_bytes_per_second, Some(1000.));
    }
    #[test]
    fn emits_utc_dates_even_when_source_dates_use_an_offset() {
        let mut s = open_state();
        let mut next = observation(725, 2_200_000_000);
        next.observed_at = "1970-01-01T01:12:05+01:00".into();
        let finding = s.observe(next).remove(0);
        assert_eq!(finding.last_seen_at, "1970-01-01T00:12:05.000Z");
        assert_eq!(
            finding.evidence.latest.observed_at,
            "1970-01-01T00:12:05.000Z"
        );
        assert_eq!(
            s.summary(725_000, true).observation.observed_at.as_deref(),
            Some("1970-01-01T00:12:05.000Z")
        );
    }
    #[test]
    fn fractional_intervals_accumulate_exactly_and_never_open_early() {
        let mut s = state();
        train(&mut s);
        for n in 1..=80u64 {
            let mut o = observation(600 + n, 1_200_000_000 + u128::from(n) * 12_000_000);
            o.finished_monotonic_ns = (600_000_000_000u128 + u128::from(n) * 1_500_000_000).into();
            o.received_at_ms = 600_000 + n as i64 * 1500;
            o.observed_at = timestamp(o.received_at_ms);
            let out = s.observe(o);
            assert_eq!(out.len(), usize::from(n == 80));
        }
        assert_eq!(s.summary(720_000, true).episode.elevated_seconds, 120.);
    }
    #[test]
    fn invalid_rebaseline_and_exhausted_revision_cannot_mutate_state() {
        let mut s = open_state();
        let before = serde_json::to_string(&s).unwrap();
        assert!(s.rebaseline("unknown", 725_000).is_err());
        assert_eq!(serde_json::to_string(&s).unwrap(), before);
        s.baseline_revision = u64::MAX;
        let before = serde_json::to_string(&s).unwrap();
        assert!(s.rebaseline("operator_reassessment", 725_000).is_err());
        assert_eq!(serde_json::to_string(&s).unwrap(), before);
    }
    #[test]
    fn timing_ineligible_clock_does_not_interrupt_open_finding_or_replace_identity() {
        let mut s = open_state();
        let finding_id = s.summary(720_000, true).episode.finding_id.unwrap();
        s.observe(observation(725, 2_160_000_000));
        assert_eq!(s.summary(725_000, true).episode.recovery_seconds, 5.);
        let watermark = serde_json::to_string(&s.watermark).unwrap();
        let baseline = serde_json::to_string(&s.baseline).unwrap();
        let mut wrong_clock = observation(730, 2_160_000_000);
        wrong_clock.continuity.clock_id = "unrelated-clock".into();
        wrong_clock.finished_monotonic_ns = 100_000_000_000_000u128.into();
        wrong_clock.age_at_receipt_seconds = f64::MAX;
        let changed = s.observe(wrong_clock);
        assert_eq!(changed.len(), 1);
        assert_eq!(changed[0].status, "open");
        assert_eq!(changed[0].finding_id, finding_id);
        assert_eq!(changed[0].evidence.recovery_seconds, 0.);
        assert_eq!(s.baseline_revision, 1);
        assert_eq!(serde_json::to_string(&s.watermark).unwrap(), watermark);
        assert_eq!(serde_json::to_string(&s.baseline).unwrap(), baseline);
        assert_eq!(s.continuity().unwrap().clock_id, "clock");
        assert_eq!(s.summary(730_000, true).observation.state, "unavailable");
        assert_eq!(
            s.summary(730_000, true).observation.reason.as_deref(),
            Some("timing_unknown")
        );
        // Recovery must use two new valid endpoints after the failed attempt.
        let changed = s.observe(observation(735, 2_160_000_000));
        assert_eq!(changed[0].status, "open");
        assert_eq!(changed[0].finding_id, finding_id);
        assert_eq!(changed[0].evidence.recovery_seconds, 0.);
        for n in 1..=12 {
            let changed = s.observe(observation(735 + n * 5, 2_160_000_000));
            assert_eq!(changed[0].finding_id, finding_id);
            assert_eq!(changed[0].status, if n == 12 { "resolved" } else { "open" });
        }
        assert_eq!(s.baseline_revision, 1);
    }

    #[test]
    fn timing_ineligible_clock_aborts_pending_without_erasing_training() {
        let mut s = state();
        train(&mut s);
        s.observe(observation(605, 1_240_000_000));
        assert_eq!(s.summary(605_000, true).episode.state, "pending");
        let baseline = serde_json::to_string(&s.baseline).unwrap();
        let mut wrong_clock = observation(610, 1_280_000_000);
        wrong_clock.continuity.clock_id = "unrelated-clock".into();
        wrong_clock.finished_monotonic_ns = 100_000_000_000_000u128.into();
        wrong_clock.age_at_receipt_seconds = f64::MAX;
        assert!(s.observe(wrong_clock).is_empty());
        assert_eq!(s.baseline_revision, 1);
        assert_eq!(serde_json::to_string(&s.baseline).unwrap(), baseline);
        assert_eq!(s.summary(610_000, true).episode.state, "quiet");
        assert_eq!(
            s.summary(610_000, true).observation.reason.as_deref(),
            Some("timing_unknown")
        );
        assert_eq!(s.continuity().unwrap().clock_id, "clock");
        s.observe(observation(615, 1_320_000_000));
        assert_eq!(s.summary(615_000, true).episode.state, "quiet");
        s.observe(observation(620, 1_360_000_000));
        assert_eq!(s.summary(620_000, true).episode.state, "pending");
        assert_eq!(s.summary(620_000, true).episode.elevated_seconds, 5.);
        assert_eq!(s.baseline_revision, 1);
    }
    #[test]
    fn timing_ineligible_first_observation_cannot_seed_a_clock_identity() {
        let mut s = state();
        let mut wrong_clock = observation(5, 0);
        wrong_clock.continuity.clock_id = "unrelated-clock".into();
        wrong_clock.age_at_receipt_seconds = f64::MAX;
        assert!(s.observe(wrong_clock).is_empty());
        assert!(s.continuity().is_none());
        assert_eq!(
            s.summary(5000, true).observation.reason.as_deref(),
            Some("timing_unknown")
        );
        s.observe(observation(10, 100));
        s.observe(observation(15, 115));
        assert_eq!(s.baseline_revision, 1);
        assert_eq!(s.continuity().unwrap().clock_id, "clock");
        assert_eq!(
            s.summary(15000, true).observation.rate_bytes_per_second,
            Some(3.)
        );
    }
    #[test]
    fn early_timing_rejection_preserves_specific_failure_reasons() {
        let mut s = state();
        let mut missing = observation(5, 0);
        missing.counter = None;
        missing.unavailable_reason = Some("missing_metric".into());
        missing.age_at_receipt_seconds = f64::MAX;
        s.observe(missing);
        assert!(s.continuity().is_none());
        assert_eq!(
            s.summary(5000, true).observation.reason.as_deref(),
            Some("missing_metric")
        );

        let mut s = open_state();
        let mut failed = observation(725, 2_160_000_000);
        failed.continuity.clock_id = "unrelated-clock".into();
        failed.counter = None;
        failed.unavailable_reason = Some("collector_failed".into());
        failed.age_at_receipt_seconds = f64::MAX;
        let changed = s.observe(failed);
        assert_eq!(changed[0].status, "open");
        assert_eq!(s.continuity().unwrap().clock_id, "clock");
        assert_eq!(s.baseline_revision, 1);
        assert_eq!(
            s.summary(725_000, true).observation.reason.as_deref(),
            Some("collector_failed")
        );
    }
}
