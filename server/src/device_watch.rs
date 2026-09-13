//! USB connection presence and negotiated-link observations. No hardware I/O.
use crate::{
    attention::{self, Condition},
    cider_wire::{Collection, Heartbeat, Resource},
    device_snapshot::{DeviceSnapshot, UsbDevice},
    error::{ApiError, ApiResult},
    store,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

const STALE_MS: i64 = 15_000;
const MAX_NODE: usize = 128;
const MAX_GLOBAL: i64 = 8192;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
struct Context {
    boot: String,
    generation: String,
    session: String,
    clock: String,
}
impl Context {
    fn from(h: &Heartbeat) -> Self {
        Self {
            boot: store::fingerprint(h.boot_id.as_bytes()),
            generation: h.agent_generation.to_string(),
            session: store::fingerprint(h.agent_session_id.as_bytes()),
            clock: store::fingerprint(h.clock_id.as_bytes()),
        }
    }
    fn matches(&self, node: &Value) -> bool {
        node["boot_id"]
            .as_str()
            .is_some_and(|s| store::fingerprint(s.as_bytes()) == self.boot)
            && node["agent_generation"] == self.generation
            && node["agent_session_id"]
                .as_str()
                .is_some_and(|s| store::fingerprint(s.as_bytes()) == self.session)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Acquisition {
    context: Context,
    collection_id: String,
    monotonic_ns: String,
    observed_at: String,
    received_at: i64,
    age_ms: i64,
    state: String,
    reason: Option<String>,
}
impl Acquisition {
    fn observation(&self, now: i64, online: bool) -> Value {
        let elapsed = now.checked_sub(self.received_at).filter(|v| *v >= 0);
        let age = elapsed.map(|e| self.age_ms.saturating_add(e));
        let state = if self.state != "current" {
            self.state.as_str()
        } else if !online || age.is_none_or(|a| a > STALE_MS) {
            "stale"
        } else {
            "current"
        };
        json!({"state":state,"observed_at":self.observed_at,"received_at":store::timestamp(self.received_at),"age_seconds":age.map(|a|a as f64/1000.),"stale_after_seconds":15})
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct NodeState {
    acquisition: Acquisition,
    context: Context,
    admission_limited: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct LinkSample {
    negotiated_bps: String,
    collection_id: String,
    observed_at: String,
    driver_resource_id: String,
    object_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Watch {
    watch_id: String,
    node_id: String,
    identity: String,
    identity_basis: String,
    identity_scope: String,
    driver_resource_id: String,
    object_id: String,
    bsd_name: Option<String>,
    context: Context,
    acquisition: Acquisition,
    presence: String,
    armed: bool,
    present_count: u8,
    absent_count: u8,
    presence_open: bool,
    #[serde(default)]
    connection_epoch: String,
    link_open: bool,
    link_state: String,
    reason: Option<String>,
    baseline: Option<LinkSample>,
    candidate: Option<LinkSample>,
    candidate_count: u8,
    current_link: Option<LinkSample>,
    reported_bps: Option<String>,
    attributed: bool,
    low_count: u8,
    recovery_count: u8,
}
fn decode<T: serde::de::DeserializeOwned>(s: &str) -> ApiResult<T> {
    serde_json::from_str(s).map_err(|_| ApiError::conflict("Invalid device-watch state"))
}
fn encode(v: &impl Serialize) -> ApiResult<String> {
    serde_json::to_string(v).map_err(|_| ApiError::unavailable())
}
fn object_id(node: &str, resource: &str) -> String {
    Uuid::new_v5(
        &Uuid::parse_str(node).expect("enrolled node UUID"),
        format!("ciderd:{resource}").as_bytes(),
    )
    .to_string()
}
fn watch_id(node: &str, d: &UsbDevice, c: &Context) -> String {
    let key = if d.identity_basis == "reported_usb_serial" {
        format!("usb-watch:{}", d.identity)
    } else {
        format!("usb-watch:{}:{}:{}", d.identity, c.boot, c.session)
    };
    Uuid::new_v5(
        &Uuid::parse_str(node).expect("enrolled node UUID"),
        key.as_bytes(),
    )
    .to_string()
}
impl Watch {
    fn new(node: &str, d: &UsbDevice, a: &Acquisition) -> Self {
        Self {
            watch_id: watch_id(node, d, &a.context),
            node_id: node.into(),
            identity: d.identity.clone(),
            identity_basis: d.identity_basis.clone(),
            identity_scope: d.identity_scope.clone(),
            driver_resource_id: d.driver_resource_id.clone(),
            object_id: object_id(node, &d.driver_resource_id),
            bsd_name: d.bsd_name.clone(),
            context: a.context.clone(),
            acquisition: a.clone(),
            presence: "unknown".into(),
            armed: false,
            present_count: 0,
            absent_count: 0,
            presence_open: false,
            connection_epoch: String::new(),
            link_open: false,
            link_state: "unknown".into(),
            reason: None,
            baseline: None,
            candidate: None,
            candidate_count: 0,
            current_link: None,
            reported_bps: None,
            attributed: true,
            low_count: 0,
            recovery_count: 0,
        }
    }
    fn break_streaks(&mut self) {
        self.present_count = 0;
        self.absent_count = 0;
        self.candidate = None;
        self.candidate_count = 0;
        self.low_count = 0;
        self.recovery_count = 0;
    }
    fn unknown(&mut self, a: &Acquisition, reason: &str) {
        self.break_streaks();
        self.acquisition = a.clone();
        self.presence = "unknown".into();
        self.link_state = "unknown".into();
        self.reported_bps = None;
        self.acquisition.state = "unavailable".into();
        self.acquisition.reason = Some(reason.into());
        self.reason = Some(reason.into());
    }
    fn observe(&mut self, d: Option<&UsbDevice>, a: &Acquisition, continuous: bool) {
        if self.context != a.context {
            self.break_streaks();
            self.armed = false;
            self.attributed = false;
            self.context = a.context.clone();
        }
        if !continuous {
            self.break_streaks();
        }
        self.acquisition = a.clone();
        self.reason = None;
        let Some(d) = d else {
            self.presence = if self.armed { "absent" } else { "unknown" }.into();
            self.reported_bps = None;
            self.present_count = 0;
            self.absent_count = self.absent_count.saturating_add(1).min(2);
            self.candidate = None;
            self.candidate_count = 0;
            self.low_count = 0;
            self.recovery_count = 0;
            self.link_state = "unknown".into();
            self.reason = Some(
                if self.armed {
                    "connection_absent"
                } else {
                    "presence_not_established_in_session"
                }
                .into(),
            );
            if self.armed && self.absent_count >= 2 {
                self.presence_open = true;
            }
            return;
        };
        self.driver_resource_id = d.driver_resource_id.clone();
        self.attributed = true;
        self.object_id = object_id(&self.node_id, &d.driver_resource_id);
        self.bsd_name = d.bsd_name.clone();
        self.reported_bps = d.negotiated_bps.clone();
        self.presence = "present".into();
        self.absent_count = 0;
        let confirming_presence = self.present_count == 1;
        self.present_count = self.present_count.saturating_add(1).min(2);
        if self.present_count >= 2 {
            if confirming_presence || self.connection_epoch.is_empty() {
                self.connection_epoch = Uuid::new_v4().to_string();
            }
            self.armed = true;
            self.presence_open = false;
        }
        if self.identity_basis != "reported_usb_serial" {
            self.link_state = "unsupported".into();
            self.reason = Some("weak_identity_same_session_presence_only".into());
            return;
        }
        let Some(speed) = d
            .negotiated_bps
            .as_ref()
            .filter(|_| d.speed_state == "available")
        else {
            self.candidate = None;
            self.candidate_count = 0;
            self.low_count = 0;
            self.recovery_count = 0;
            self.link_state = if d.speed_state == "unsupported" {
                "unsupported"
            } else {
                "unknown"
            }
            .into();
            self.reason = d
                .reason
                .clone()
                .or(Some("negotiated_speed_unavailable".into()));
            return;
        };
        let sample = LinkSample {
            negotiated_bps: speed.clone(),
            collection_id: a.collection_id.clone(),
            observed_at: a.observed_at.clone(),
            driver_resource_id: d.driver_resource_id.clone(),
            object_id: self.object_id.clone(),
        };
        self.current_link = Some(sample.clone());
        let rate = speed.parse::<u64>().expect("validated bitrate");
        let prior = self
            .baseline
            .as_ref()
            .map(|b| b.negotiated_bps.parse::<u64>().expect("stored bitrate"));
        if self
            .candidate
            .as_ref()
            .is_some_and(|c| c.negotiated_bps == *speed)
        {
            self.candidate_count = self.candidate_count.saturating_add(1).min(2);
        } else {
            self.candidate = Some(sample.clone());
            self.candidate_count = 1;
        }
        if self.candidate_count >= 2 && prior.is_none_or(|b| rate > b) {
            self.baseline = Some(sample.clone());
        }
        let Some(baseline) = self
            .baseline
            .as_ref()
            .map(|b| b.negotiated_bps.parse::<u64>().expect("stored bitrate"))
        else {
            self.link_state = "warming_up".into();
            self.reason = Some("two_matching_speed_observations_required".into());
            return;
        };
        if rate < baseline {
            self.low_count = self.low_count.saturating_add(1).min(2);
            self.recovery_count = 0;
            if self.low_count >= 2 {
                self.link_open = true;
            }
            self.link_state = if self.link_open {
                "warning"
            } else {
                "warming_up"
            }
            .into();
            self.reason = Some(
                if self.link_open {
                    "below_previously_confirmed_link_speed"
                } else {
                    "link_degradation_pending"
                }
                .into(),
            );
        } else {
            self.low_count = 0;
            self.recovery_count = self.recovery_count.saturating_add(1).min(2);
            if self.recovery_count >= 2 {
                self.link_open = false;
            }
            self.link_state = if self.link_open {
                "warning"
            } else {
                "no_current_warning"
            }
            .into();
            if self.link_open {
                self.reason = Some("link_recovery_pending".into());
            }
        }
    }
    fn evidence(&self) -> Value {
        json!({"policy_version":"1","watch_id":self.watch_id,"connection_epoch":self.connection_epoch,"connection_context":self.context,"identity":self.identity,"identity_basis":self.identity_basis,"identity_scope":self.identity_scope,"driver_resource_id":self.driver_resource_id,"object_id":self.object_id,"bsd_name":self.bsd_name,"presence":self.presence,"armed":self.armed,"present_observations":self.present_count,"absent_observations":self.absent_count,"collection_id":self.acquisition.collection_id,"observed_at":self.acquisition.observed_at,"baseline":self.baseline,"current":self.current_link,"required_observations":2,"minimum_observation_spacing_seconds":1,"reason":self.reason})
    }
    fn projection(&self, now: i64, online: bool, limited: bool) -> Value {
        let observation = self.acquisition.observation(now, online);
        let current = observation["state"] == "current";
        json!({"watch_id":self.watch_id,"identity_basis":self.identity_basis,"identity_scope":self.identity_scope,"presence":if current{self.presence.as_str()}else{"unknown"},"armed":self.armed,"link_state":if current{self.link_state.as_str()}else{"unknown"},"negotiated_bps":if current && self.presence=="present"{self.reported_bps.clone()}else{None},"baseline_bps":self.baseline.as_ref().map(|s|s.negotiated_bps.clone()),"reason":if !current{Some("source_or_owner_not_current")}else if limited{Some("admission_limited")}else{self.reason.as_deref()},"observation":observation,"evidence":self.evidence()})
    }
    fn conditions(&self, now: i64, online: bool) -> Vec<Condition> {
        let observation = self.acquisition.observation(now, online);
        let current = observation["state"] == "current";
        [
            (
                "device_presence",
                "usb_connection_absent",
                self.presence_open,
                current && self.presence != "unknown",
                self.present_count >= 2 && self.presence == "present",
                "Previously observed USB storage connection is absent",
            ),
            (
                "usb_link",
                "transport_link_degradation",
                self.link_open,
                current
                    && self.presence == "present"
                    && matches!(self.link_state.as_str(), "warning" | "no_current_warning"),
                self.link_state == "no_current_warning",
                "USB connection negotiated below its previously observed speed",
            ),
        ]
        .into_iter()
        .map(|(key, rule, open, usable, clear, summary)| {
            let mut evidence = self.evidence();
            evidence["rule_id"] = json!(rule);
            evidence["classification"] = json!(rule);
            evidence["source_observation"] = observation.clone();
            Condition {
                key: format!("{key}:{}", self.watch_id),
                kind: "reliability".into(),
                node_id: self.node_id.clone(),
                object_id: Some(self.object_id.clone()),
                status: if !usable {
                    "interrupted"
                } else if open {
                    "open"
                } else if clear {
                    "resolved"
                } else {
                    "interrupted"
                }
                .into(),
                severity: "warning".into(),
                observation_state: if usable {
                    "current"
                } else if observation["state"] == "stale" {
                    "stale"
                } else {
                    "unknown"
                }
                .into(),
                summary: summary.into(),
                evidence,
            }
        })
        .collect()
    }
}
async fn save(tx: &mut Transaction<'_, Sqlite>, w: &Watch, now: i64) -> ApiResult<()> {
    sqlx::query("INSERT INTO device_watch_devices(watch_id,node_id,state_json,updated_at) VALUES(?,?,?,?) ON CONFLICT(watch_id) DO UPDATE SET state_json=excluded.state_json,updated_at=excluded.updated_at")
        .bind(&w.watch_id).bind(&w.node_id).bind(encode(w)?).bind(now).execute(&mut **tx).await?;
    Ok(())
}
fn time(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis())
}

/// Called only inside the authenticated, accepted heartbeat's SQLite transaction.
pub async fn ingest(
    tx: &mut Transaction<'_, Sqlite>,
    hb: &Heartbeat,
    resources: &BTreeMap<String, Resource>,
    graph_known: bool,
    accepted_collections: &BTreeSet<String>,
    now: i64,
) -> ApiResult<()> {
    let previous: Option<String> =
        sqlx::query_scalar("SELECT state_json FROM device_watch_nodes WHERE node_id=?")
            .bind(&hb.node_id)
            .fetch_optional(&mut **tx)
            .await?;
    let previous = previous.as_deref().map(decode::<NodeState>).transpose()?;
    let context = Context::from(hb);
    // A heartbeat clock/session transition invalidates pending comparisons even
    // when its graph is not yet known or no new host acquisition was delivered.
    if let Some(previous) = previous.as_ref().filter(|p| p.context != context) {
        let rows =
            sqlx::query("SELECT state_json FROM device_watch_devices WHERE node_id=? LIMIT 129")
                .bind(&hb.node_id)
                .fetch_all(&mut **tx)
                .await?;
        for row in rows {
            let mut w: Watch = decode(row.get("state_json"))?;
            let old = w.acquisition.clone();
            w.unknown(&old, "source_context_changed");
            w.armed = false;
            save(tx, &w, now).await?;
            for c in w.conditions(now, false) {
                attention::observe_condition(tx, &c, now).await?;
            }
        }
        let mut latest = previous.clone();
        latest.context = context.clone();
        store_node(tx, &hb.node_id, &latest, now).await?;
    }
    if !graph_known {
        return Ok(());
    }
    // Monotonic values from different clock segments are incomparable. The
    // collector's explicit latest-attempt reference selects the host snapshot.
    let candidate = hb
        .collector_states
        .iter()
        .find(|state| {
            state.collector == "iokit.block"
                && resources
                    .get(&state.resource_id)
                    .is_some_and(|r| r.resource_type == "host")
        })
        .and_then(|state| {
            state.last_attempt_id.as_ref().and_then(|id| {
                hb.collections.iter().find(|c| {
                    c.collection_id == *id
                        && c.resource_id == state.resource_id
                        && c.collector == "iokit.block"
                })
            })
        });
    let Some(c) = candidate.filter(|c| accepted_collections.contains(&c.collection_id)) else {
        return Ok(());
    };
    let mut acquisition_context = context.clone();
    acquisition_context.clock = store::fingerprint(c.clock_id.as_bytes());
    let same = c.clock_id == hb.clock_id
        && previous
            .as_ref()
            .is_some_and(|p| p.acquisition.context == acquisition_context);
    let mono = c.finished_monotonic_ns.get();
    let prior_mono = previous
        .as_ref()
        .and_then(|p| p.acquisition.monotonic_ns.parse::<u128>().ok());
    if same
        && previous.as_ref().is_some_and(|p| {
            p.acquisition.collection_id == c.collection_id || prior_mono.is_some_and(|m| mono <= m)
        })
    {
        return Ok(());
    }
    let subsecond = same && prior_mono.is_some_and(|m| mono.saturating_sub(m) < 1_000_000_000);
    let mut reason = None;
    let age = hb
        .monotonic_ns
        .get()
        .checked_sub(mono)
        .map(|n| n / 1_000_000)
        .and_then(|n| i64::try_from(n).ok());
    let skew = time(&hb.created_at)
        .map(|t| now.abs_diff(t))
        .and_then(|n| i64::try_from(n).ok());
    let age_ms = age
        .zip(skew)
        .map(|(a, s)| a.saturating_add(s))
        .unwrap_or(STALE_MS + 1);
    if c.clock_id != hb.clock_id
        || age_ms > STALE_MS
        || time(&c.finished_at).is_none_or(|t| now.abs_diff(t) > STALE_MS as u64)
    {
        reason = Some("stale_or_mismatched_acquisition");
    }
    if c.status != "ok" {
        reason = Some("enumeration_not_successful");
    }
    let snapshot = parse_snapshot(c, resources);
    if let Err(r) = &snapshot {
        reason = Some(*r);
    }
    if subsecond {
        reason = Some("observation_spacing_too_short");
    }
    let a = Acquisition {
        context: acquisition_context,
        collection_id: c.collection_id.clone(),
        monotonic_ns: mono.to_string(),
        observed_at: time(&c.finished_at)
            .map(store::timestamp)
            .unwrap_or_else(|| store::timestamp(now)),
        received_at: now,
        age_ms,
        state: if reason.is_none() {
            "current"
        } else {
            "unavailable"
        }
        .into(),
        reason: reason.map(str::to_owned),
    };
    let continuous = same
        && previous
            .as_ref()
            .is_some_and(|p| p.context == context && p.acquisition.state == "current")
        && prior_mono.is_some_and(|m| mono.saturating_sub(m) <= 15_000_000_000);
    let rows = sqlx::query("SELECT state_json FROM device_watch_devices WHERE node_id=? LIMIT 129")
        .bind(&hb.node_id)
        .fetch_all(&mut **tx)
        .await?;
    if rows.len() > MAX_NODE {
        return Err(ApiError::unavailable());
    }
    let mut watches: BTreeMap<String, Watch> = rows
        .iter()
        .map(|r| decode::<Watch>(r.get("state_json")).map(|w| (w.watch_id.clone(), w)))
        .collect::<ApiResult<_>>()?;
    let mut limited = previous.as_ref().is_some_and(|p| p.admission_limited) && reason.is_some();
    let devices = snapshot
        .as_ref()
        .ok()
        .filter(|_| reason.is_none())
        .map(|s| s.devices.as_slice())
        .unwrap_or(&[]);
    let by_id: BTreeMap<_, _> = devices
        .iter()
        .map(|d| (watch_id(&hb.node_id, d, &context), d))
        .collect();
    let mut total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM device_watch_devices")
        .fetch_one(&mut **tx)
        .await?;
    for (id, d) in &by_id {
        if !watches.contains_key(id) {
            if watches.len() >= MAX_NODE || total >= MAX_GLOBAL {
                limited = true;
                continue;
            }
            watches.insert(id.clone(), Watch::new(&hb.node_id, d, &a));
            total += 1;
        }
    }
    for w in watches.values_mut() {
        if let Some(r) = reason {
            w.unknown(&a, r);
        } else if w.identity_basis == "boot_registry" && w.context != context {
            w.unknown(&a, "weak_identity_prior_session");
        } else if !by_id.contains_key(&w.watch_id)
            && devices
                .iter()
                .any(|d| d.driver_resource_id == w.driver_resource_id)
        {
            w.unknown(&a, "driver_identity_ambiguous");
            w.attributed = false;
            w.armed = false;
        } else {
            w.observe(by_id.get(&w.watch_id).copied(), &a, continuous);
        }
        save(tx, w, now).await?;
        for c in w.conditions(now, true) {
            attention::observe_condition(tx, &c, now).await?;
        }
    }
    store_node(
        tx,
        &hb.node_id,
        &NodeState {
            acquisition: a,
            context,
            admission_limited: limited,
        },
        now,
    )
    .await?;
    Ok(())
}
async fn store_node(
    tx: &mut Transaction<'_, Sqlite>,
    node: &str,
    state: &NodeState,
    now: i64,
) -> ApiResult<()> {
    sqlx::query("INSERT INTO device_watch_nodes(node_id,state_json,updated_at) VALUES(?,?,?) ON CONFLICT(node_id) DO UPDATE SET state_json=excluded.state_json,updated_at=excluded.updated_at")
        .bind(node).bind(encode(state)?).bind(now).execute(&mut **tx).await?;
    Ok(())
}

fn parse_snapshot(
    c: &Collection,
    resources: &BTreeMap<String, Resource>,
) -> Result<DeviceSnapshot, &'static str> {
    let value = c
        .extensions
        .as_ref()
        .and_then(|e| e.get("usb_device_snapshot"))
        .ok_or("usb_snapshot_unsupported")?;
    let snapshot: DeviceSnapshot =
        serde_json::from_value(value.clone()).map_err(|_| "invalid_usb_snapshot")?;
    snapshot.validate().map_err(|_| "invalid_usb_snapshot")?;
    if !snapshot.complete {
        return Err("usb_enumeration_partial");
    }
    if snapshot.devices.iter().any(|d| {
        !resources.get(&d.driver_resource_id).is_some_and(|r| {
            r.resource_type == "controller"
                && r.attributes.get("source") == Some(&json!("IOBlockStorageDriver"))
                && r.attributes.get("scope") == Some(&json!("driver"))
        })
    }) {
        return Err("usb_driver_reference_unresolved");
    }
    Ok(snapshot)
}
fn node_from_row(r: &sqlx::sqlite::SqliteRow) -> Value {
    json!({"last_seen_at":r.get::<Option<i64>,_>("last_seen_at"),
        "goodbye":r.get::<Option<i64>,_>("goodbye_at").is_some(),
        "revoked":r.get::<Option<i64>,_>("revoked_at").is_some(),
        "boot_id":r.get::<Option<String>,_>("boot_id"),
        "agent_generation":r.get::<Option<String>,_>("agent_generation"),
        "agent_session_id":r.get::<Option<String>,_>("agent_session_id")})
}
async fn loaded(tx: &mut Transaction<'_, Sqlite>) -> ApiResult<Vec<(Watch, Value, NodeState)>> {
    let (count,bytes):(i64,i64)=sqlx::query_as("SELECT COUNT(*),COALESCE(SUM(length(CAST(state_json AS BLOB))),0) FROM device_watch_devices")
        .fetch_one(&mut **tx).await?;
    if count > MAX_GLOBAL || bytes > 32 * 1024 * 1024 {
        return Err(ApiError::unavailable());
    }
    // Extract only small receiver fields. Repeating the full retained graph for
    // each watch would multiply an 8 MiB graph by every attached device.
    let rows=sqlx::query("SELECT d.state_json,n.last_seen_at,n.goodbye_at,n.revoked_at,n.boot_id,json_extract(c.state_json,'$.generation') AS agent_generation,json_extract(c.state_json,'$.session') AS agent_session_id,w.state_json AS watch_node FROM device_watch_devices d JOIN nodes n ON n.node_id=d.node_id LEFT JOIN cider_nodes c ON c.node_id=d.node_id JOIN device_watch_nodes w ON w.node_id=d.node_id ORDER BY d.watch_id LIMIT 8193")
        .fetch_all(&mut **tx).await?;
    rows.iter()
        .map(|r| {
            Ok((
                decode(r.get("state_json"))?,
                node_from_row(r),
                decode(r.get("watch_node"))?,
            ))
        })
        .collect()
}
fn online(node: &Value, now: i64) -> bool {
    node["last_seen_at"]
        .as_i64()
        .is_some_and(|t| t <= now && now - t < 30_000)
        && node["goodbye"] == false
        && node["revoked"] == false
}

/// Correlate a proven inventory edge with one armed watch in this source context.
/// This is identity evidence only; absence still requires its normal observations.
pub(crate) async fn watch_for_driver(
    tx: &mut Transaction<'_, Sqlite>, hb: &Heartbeat, driver: &str,
) -> ApiResult<Option<(String,String)>> {
    let rows = sqlx::query("SELECT state_json FROM device_watch_devices WHERE node_id=? LIMIT 129")
        .bind(&hb.node_id).fetch_all(&mut **tx).await?;
    if rows.len() > MAX_NODE { return Err(ApiError::unavailable()); }
    let context = Context::from(hb);
    let mut matches = Vec::new();
    for row in rows {
        let w: Watch = decode(row.get("state_json"))?;
        if w.driver_resource_id == driver && w.attributed && w.armed && w.context == context && !w.connection_epoch.is_empty() {
            matches.push((w.watch_id,w.connection_epoch));
        }
    }
    Ok(if matches.len() == 1 { matches.pop() } else { None })
}

pub async fn conditions(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<Vec<Condition>> {
    Ok(loaded(tx)
        .await?
        .into_iter()
        .flat_map(|(w, n, latest)| {
            let current = online(&n, now) && w.context.matches(&n) && w.context == latest.context;
            w.conditions(now, current)
        })
        .collect())
}
pub async fn summaries(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<Vec<Value>> {
    Ok(loaded(tx).await?.into_iter().map(|(w,n,latest)|{
        let current=online(&n,now)&&w.context.matches(&n)&&w.context==latest.context;
        json!({"node_id":w.node_id,"driver_object_id":w.object_id,"context":w.context,"attributed":w.attributed,
            "projection":w.projection(now,current,latest.admission_limited)})
    }).collect())
}
pub async fn node_summaries(
    tx: &mut Transaction<'_, Sqlite>,
    now: i64,
) -> ApiResult<BTreeMap<String, Value>> {
    let rows=sqlx::query("SELECT w.node_id,w.state_json,n.last_seen_at,n.goodbye_at,n.revoked_at,n.boot_id,json_extract(c.state_json,'$.generation') AS agent_generation,json_extract(c.state_json,'$.session') AS agent_session_id,(SELECT COUNT(*) FROM device_watch_devices d WHERE d.node_id=w.node_id) AS watched FROM device_watch_nodes w JOIN nodes n ON n.node_id=w.node_id LEFT JOIN cider_nodes c ON c.node_id=w.node_id LIMIT 2049")
        .fetch_all(&mut **tx).await?;
    if rows.len() > 2048 {
        return Err(ApiError::unavailable());
    }
    rows.iter().map(|r|{
        let state:NodeState=decode(r.get("state_json"))?;
        let node=node_from_row(r);
        let observation=state.acquisition.observation(now,online(&node,now)
            && state.acquisition.context.matches(&node)&&state.acquisition.context==state.context);
        let status=if observation["state"]=="current" && state.admission_limited {"partial"}
            else if state.acquisition.reason.as_deref()==Some("usb_snapshot_unsupported") {"unsupported"}
            else {observation["state"].as_str().unwrap_or("unknown")};
        Ok((r.get("node_id"),json!({"state":status,"reason":if state.admission_limited {Some("admission_limited")}else{state.acquisition.reason.as_deref()},
            "observation":observation,"admission_limited":state.admission_limited,"watched_devices":r.get::<i64,_>("watched"),
            "maximum_devices_per_node":MAX_NODE,"maximum_devices_global":MAX_GLOBAL})))
    }).collect()
}
pub fn unknown_node() -> Value {
    json!({"state":"unknown","reason":"usb_snapshot_not_observed","observation":{"state":"unknown","observed_at":null,"received_at":null,"age_seconds":null,"stale_after_seconds":15},
        "admission_limited":false,"watched_devices":0,"maximum_devices_per_node":MAX_NODE,"maximum_devices_global":MAX_GLOBAL})
}
pub fn disk_projection(
    node: &Value,
    disk: &Value,
    objects: &[Value],
    summaries: &[Value],
) -> Value {
    let drivers: Vec<_> = objects
        .iter()
        .filter(|o| {
            o["node_id"] == node["node_id"]
                && o["active"] == true
                && o["properties"]["ciderd_resource_type"] == "controller"
                && o["properties"]["source"] == "IOBlockStorageDriver"
                && o["properties"]["scope"] == "driver"
                && o["topology_state"] == "resolved"
                && o["physical_disk_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.len() == 1 && ids[0] == disk["object_id"])
                && ["boot_id", "agent_generation", "agent_session_id"]
                    .iter()
                    .all(|k| o["properties"]["ciderd_acquisition"][*k] == node[*k])
        })
        .collect();
    if drivers.len() != 1 {
        return Value::Null;
    }
    let candidates: Vec<_> = summaries
        .iter()
        .filter(|s| {
            s["node_id"] == node["node_id"]
                && s["driver_object_id"] == drivers[0]["object_id"]
                && s["attributed"] == true
                && serde_json::from_value::<Context>(s["context"].clone())
                    .is_ok_and(|c| c.matches(node))
        })
        .collect();
    if candidates.len() == 1 {
        candidates[0]["projection"].clone()
    } else {
        Value::Null
    }
}
