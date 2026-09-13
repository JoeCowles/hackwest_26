//! Durable, bounded SMS delivery. Credentials are never persisted or logged.
use crate::{
    error::{ApiError, ApiResult},
    store::{self, AppState},
};
use serde_json::{Value, json};
use sqlx::{Row, Sqlite, Transaction, sqlite::SqliteRow};
use std::{collections::BTreeMap, time::Duration};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
const MAX_OUTBOX: i64 = 1000;
fn masked(s: &str) -> Value {
    if s.is_empty() {
        Value::Null
    } else {
        json!(format!("***{}", &s[s.len().saturating_sub(4)..]))
    }
}
fn view(r: &SqliteRow) -> Value {
    json!({"enabled":r.get::<i64,_>("enabled")!=0,"sender":masked(r.get("sender")),"recipient":masked(r.get("recipient")),"revision":r.get::<String,_>("revision"),"credential_state":r.get::<String,_>("credential_state"),"worker_seen_at":r.get::<Option<i64>,_>("worker_seen_at").map(store::timestamp),"admission_limited":r.get::<i64,_>("admission_limited")!=0,"updated_at":store::timestamp(r.get("updated_at"))})
}
fn e164(s: &str) -> bool {
    s.starts_with('+')
        && (3..=16).contains(&s.len())
        && s.as_bytes()[1] != b'0'
        && s.as_bytes()[1..].iter().all(u8::is_ascii_digit)
}
pub async fn settings(state: &AppState) -> ApiResult<Value> {
    Ok(view(
        &sqlx::query("SELECT * FROM notification_settings WHERE id=1")
            .fetch_one(&state.db)
            .await?,
    ))
}
pub async fn configure(
    tx: &mut Transaction<'_, Sqlite>,
    request: &Value,
    actor: &str,
    now: i64,
) -> ApiResult<Value> {
    let obj = request
        .as_object()
        .ok_or_else(|| ApiError::field("settings", "Expected an object"))?;
    for key in obj.keys() {
        if !matches!(
            key.as_str(),
            "expected_revision" | "enabled" | "sender" | "recipient"
        ) {
            return Err(ApiError::field(key, "Unknown setting"));
        }
    }
    let row = sqlx::query("SELECT * FROM notification_settings WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    if request["expected_revision"].as_str() != Some(row.get("revision")) {
        return Err(ApiError::conflict("Notification settings revision changed"));
    }
    let enabled = request["enabled"]
        .as_bool()
        .ok_or_else(|| ApiError::field("enabled", "Expected boolean"))?;
    let mut phones = Vec::new();
    for field in ["sender", "recipient"] {
        let v = match request.get(field) {
            None => row.get::<String, _>(field),
            Some(v) => v
                .as_str()
                .ok_or_else(|| ApiError::field(field, "Expected E.164 string"))?
                .to_owned(),
        };
        if (!v.is_empty() && !e164(&v)) || (enabled && v.is_empty()) {
            return Err(ApiError::field(
                field,
                "An E.164 phone number is required when enabled",
            ));
        }
        phones.push(v);
    }
    sqlx::query("UPDATE notification_settings SET enabled=?,sender=?,recipient=?,revision=?,updated_at=?,actor_hash=? WHERE id=1").bind(enabled).bind(&phones[0]).bind(&phones[1]).bind(Uuid::new_v4().to_string()).bind(now).bind(store::fingerprint(actor.as_bytes())).execute(&mut **tx).await?;
    sqlx::query("UPDATE notification_outbox SET state='suppressed',reason='configuration_changed',updated_at=? WHERE state='queued'").bind(now).execute(&mut **tx).await?;
    cancel_intent(tx, None, "configuration_changed", now).await?;
    Ok(view(
        &sqlx::query("SELECT * FROM notification_settings WHERE id=1")
            .fetch_one(&mut **tx)
            .await?,
    ))
}
pub(crate) fn intent_view(value: Option<String>) -> Value {
    let Some(mut value) = value.and_then(|s| serde_json::from_str::<Value>(&s).ok()) else {
        return Value::Null;
    };
    for key in ["created_at", "updated_at"] {
        if let Some(at) = value[key].as_i64() {
            value[key] = json!(store::timestamp(at));
        }
    }
    value
}
pub(crate) async fn cancel_intent(
    tx: &mut Transaction<'_, Sqlite>,
    episode: Option<&str>,
    reason: &str,
    now: i64,
) -> ApiResult<()> {
    sqlx::query("UPDATE attention_episodes SET notification_intent_json=json_set(notification_intent_json,'$.state','suppressed','$.reason',?,'$.updated_at',?) WHERE (? IS NULL OR id=?) AND json_extract(notification_intent_json,'$.state')='pending_admission'")
        .bind(reason).bind(now).bind(episode).bind(episode).execute(&mut **tx).await?;
    Ok(())
}
pub(crate) async fn enqueue(
    tx: &mut Transaction<'_, Sqlite>,
    episode: &str,
    revision: &str,
    now: i64,
) -> ApiResult<()> {
    let settings = sqlx::query("SELECT enabled,revision FROM notification_settings WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    admit(
        tx,
        episode,
        revision,
        settings.get("revision"),
        settings.get::<i64, _>("enabled") != 0,
        now,
        now,
    )
    .await?;
    Ok(())
}
async fn admit(
    tx: &mut Transaction<'_, Sqlite>,
    episode: &str,
    revision: &str,
    settings_revision: &str,
    enabled: bool,
    created: i64,
    now: i64,
) -> ApiResult<bool> {
    let transition = format!("{episode}:{revision}");
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM notification_outbox WHERE transition_key=?)",
    )
    .bind(&transition)
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        sqlx::query("UPDATE attention_episodes SET notification_intent_json=NULL WHERE id=? AND json_extract(notification_intent_json,'$.transition_revision')=?").bind(episode).bind(revision).execute(&mut **tx).await?;
        return Ok(true);
    }
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox")
        .fetch_one(&mut **tx)
        .await?;
    let removed = if count >= MAX_OUTBOX {
        sqlx::query("DELETE FROM notification_outbox WHERE id IN (SELECT id FROM notification_outbox WHERE state NOT IN ('queued','sending','accepted') ORDER BY created_at,id LIMIT 1)").execute(&mut **tx).await?.rows_affected() as i64
    } else {
        0
    };
    if count - removed >= MAX_OUTBOX {
        // One bounded intent per episode survives unchanged source observations.
        let intent = json!({"state":if enabled{"pending_admission"}else{"suppressed"},"transition_revision":revision,"settings_revision":settings_revision,"created_at":created,"updated_at":now,"reason":if enabled{"outbox_full"}else{"disabled"}});
        sqlx::query("UPDATE attention_episodes SET notification_intent_json=? WHERE id=?")
            .bind(intent.to_string())
            .bind(episode)
            .execute(&mut **tx)
            .await?;
        if enabled {
            sqlx::query("UPDATE notification_settings SET admission_limited=1 WHERE id=1")
                .execute(&mut **tx)
                .await?;
        }
        return Ok(false);
    }
    sqlx::query("INSERT OR IGNORE INTO notification_outbox(id,episode_id,transition_key,state,next_attempt_at,created_at,updated_at,reason,settings_revision) VALUES(?,?,?,?,?,?,?,?,?)")
        .bind(Uuid::new_v4().to_string()).bind(episode).bind(transition).bind(if enabled{"queued"}else{"suppressed"}).bind(now).bind(created).bind(now).bind(if enabled{None}else{Some("disabled")}).bind(settings_revision).execute(&mut **tx).await?;
    sqlx::query("UPDATE attention_episodes SET notification_intent_json=NULL WHERE id=?")
        .bind(episode)
        .execute(&mut **tx)
        .await?;
    Ok(true)
}
pub(crate) async fn retry_pending(tx: &mut Transaction<'_, Sqlite>, now: i64) -> ApiResult<()> {
    let settings = sqlx::query("SELECT enabled,revision FROM notification_settings WHERE id=1")
        .fetch_one(&mut **tx)
        .await?;
    let settings_revision: String = settings.get("revision");
    let rows=sqlx::query("SELECT id,status,revision,ack_json,notification_intent_json FROM attention_episodes WHERE json_extract(notification_intent_json,'$.state')='pending_admission' ORDER BY json_extract(notification_intent_json,'$.created_at'),id LIMIT 10000").fetch_all(&mut **tx).await?;
    for row in rows {
        let id: String = row.get("id");
        let mut intent: Value = serde_json::from_str(row.get("notification_intent_json"))
            .map_err(|_| ApiError::conflict("Invalid persisted notification intent"))?;
        let reason = if settings.get::<i64, _>("enabled") == 0
            || intent["settings_revision"] != settings_revision
        {
            Some("configuration_changed")
        } else if row.get::<String, _>("status") != "open" {
            Some("condition_resolved")
        } else if row.get::<Option<String>, _>("ack_json").is_some() {
            Some("acknowledged")
        } else if intent["transition_revision"] != row.get::<String, _>("revision") {
            Some("superseded")
        } else {
            None
        };
        if let Some(reason) = reason {
            cancel_intent(tx, Some(&id), reason, now).await?;
            continue;
        }
        let created = intent["created_at"]
            .as_i64()
            .ok_or_else(|| ApiError::conflict("Invalid notification intent time"))?;
        if now.saturating_sub(created) > 24 * 60 * 60 * 1000 {
            intent["state"] = json!("failed");
            intent["reason"] = json!("admission_expired");
            intent["updated_at"] = json!(now);
            sqlx::query("UPDATE attention_episodes SET notification_intent_json=? WHERE id=?")
                .bind(intent.to_string())
                .bind(&id)
                .execute(&mut **tx)
                .await?;
            continue;
        }
        if !admit(
            tx,
            &id,
            intent["transition_revision"].as_str().unwrap(),
            &settings_revision,
            true,
            created,
            now,
        )
        .await?
        {
            break;
        }
    }
    sqlx::query("UPDATE notification_settings SET admission_limited=1 WHERE id=1 AND EXISTS(SELECT 1 FROM attention_episodes WHERE json_extract(notification_intent_json,'$.state')='pending_admission')").execute(&mut **tx).await?;
    Ok(())
}

struct Credentials {
    account: String,
    user: String,
    secret: String,
    default_sender: Option<String>,
}
impl Credentials {
    fn load() -> Option<Self> {
        // Parse without mutating process environment (safe on the multithreaded runtime).
        let mut file = BTreeMap::new();
        if let Ok(iter) = dotenvy::from_path_iter(".env") {
            for pair in iter {
                let (k, v) = pair.ok()?;
                file.insert(k, v);
            }
        }
        Self::from_values(|key| std::env::var(key).ok().or_else(|| file.get(key).cloned()))
    }
    fn from_values(mut get: impl FnMut(&str) -> Option<String>) -> Option<Self> {
        let alias = get("TWILIO_SID");
        let account =
            get("TWILIO_ACCOUNT_SID").or_else(|| alias.clone().filter(|s| s.starts_with("AC")))?;
        let user = alias
            .filter(|s| s.starts_with("SK"))
            .unwrap_or_else(|| account.clone());
        let secret = get("TWILIO_SECRET")?;
        if !sid(&account, "AC")
            || !(sid(&user, "AC") || sid(&user, "SK"))
            || secret.is_empty()
            || secret.len() > 4096
        {
            return None;
        }
        Some(Self {
            account,
            user,
            secret,
            default_sender: get("TWILIO_FROM").filter(|s| e164(s)),
        })
    }
}
fn sid(value: &str, prefix: &str) -> bool {
    value.len() == 34
        && value.starts_with(prefix)
        && value.as_bytes()[2..].iter().all(u8::is_ascii_hexdigit)
}
#[derive(Debug)]
struct Outcome {
    state: &'static str,
    sid: Option<String>,
    provider: Option<String>,
    reason: Option<&'static str>,
}
fn uncertain(reason: &'static str) -> Outcome {
    Outcome {
        state: "uncertain",
        sid: None,
        provider: None,
        reason: Some(reason),
    }
}
struct Twilio {
    client: reqwest::Client,
    base: String,
    credentials: Credentials,
}
impl Twilio {
    fn new(credentials: Credentials) -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .connect_timeout(Duration::from_secs(5))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            base: "https://api.twilio.com".into(),
            credentials,
        })
    }
    async fn request(
        &self,
        message: Option<&str>,
        sender: &str,
        recipient: &str,
        body: &str,
    ) -> Outcome {
        if message.is_some_and(|m| !sid(m, "SM")) {
            return uncertain("invalid_message_sid");
        }
        let base = format!(
            "{}/2010-04-01/Accounts/{}/Messages",
            self.base, self.credentials.account
        );
        let builder = if let Some(id) = message {
            self.client.get(format!("{base}/{id}.json"))
        } else {
            self.client.post(format!("{base}.json")).form(&[
                ("From", sender),
                ("To", recipient),
                ("Body", body),
            ])
        };
        let response = builder
            .basic_auth(&self.credentials.user, Some(&self.credentials.secret))
            .send()
            .await;
        let mut response = match response {
            Ok(r) => r,
            Err(_) => return uncertain("transport_outcome_unknown"),
        };
        let status = response.status();
        if status.as_u16() == 429 {
            return Outcome {
                state: "queued",
                sid: None,
                provider: None,
                reason: Some("rate_limited"),
            };
        }
        if status.is_client_error() {
            return Outcome {
                state: "failed",
                sid: None,
                provider: None,
                reason: Some("provider_rejected"),
            };
        }
        if !status.is_success() {
            return uncertain("provider_outcome_unknown");
        }
        let mut bytes = Vec::new();
        loop {
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if bytes.len() + chunk.len() > 16384 {
                        return uncertain("oversized_provider_response");
                    }
                    bytes.extend_from_slice(&chunk);
                }
                Ok(None) => break,
                Err(_) => return uncertain("incomplete_provider_response"),
            }
        }
        let value: Value = match serde_json::from_slice(&bytes) {
            Ok(v) => v,
            Err(_) => return uncertain("malformed_provider_response"),
        };
        let Some(id) = value["sid"].as_str().filter(|v| sid(v, "SM")) else {
            return uncertain("invalid_message_sid");
        };
        if message.is_some_and(|m| m != id) {
            return uncertain("message_sid_mismatch");
        }
        let provider = value["status"].as_str().unwrap_or("");
        let state = match provider {
            "delivered" => "delivered",
            "failed" | "undelivered" | "canceled" => "failed",
            "accepted" | "queued" | "sending" | "sent" => "accepted",
            _ => "uncertain",
        };
        Outcome {
            state,
            sid: Some(id.into()),
            provider: if provider.len() <= 32 {
                Some(provider.into())
            } else {
                None
            },
            reason: if state == "uncertain" {
                Some("unknown_provider_status")
            } else {
                None
            },
        }
    }
}
async fn recover(state: &AppState, now: i64) -> ApiResult<()> {
    let _guard = state.writer.lock().await;
    sqlx::query("UPDATE notification_outbox SET state='uncertain',reason='interrupted_send',updated_at=? WHERE state='sending'").bind(now).execute(&state.db).await?;
    Ok(())
}
async fn step(state: &AppState, client: Option<&Twilio>, now: i64) -> ApiResult<()> {
    let (job, sender, recipient, poll) = {
        let _guard = state.writer.lock().await;
        let mut tx = state.db.begin().await?;
        sqlx::query(
            "UPDATE notification_settings SET credential_state=?,worker_seen_at=? WHERE id=1",
        )
        .bind(if client.is_some() {
            "configured"
        } else {
            "missing_or_invalid"
        })
        .bind(now)
        .execute(&mut *tx)
        .await?;
        // run awaits each step serially. A prior committed send claim here has no
        // current HTTP owner: even an outcome-write failure must never resend it.
        sqlx::query("UPDATE notification_outbox SET state='uncertain',reason='outcome_not_recorded',updated_at=? WHERE state='sending'").bind(now).execute(&mut *tx).await?;
        // Neither an expired queued alert nor a delivery deadline may imply delivery.
        sqlx::query("UPDATE notification_outbox SET state='failed',reason='queue_expired',updated_at=? WHERE state='queued' AND created_at<?").bind(now).bind(now-24*60*60*1000).execute(&mut *tx).await?;
        sqlx::query("UPDATE notification_outbox SET state='uncertain',reason='delivery_deadline',updated_at=? WHERE state='accepted' AND created_at<?").bind(now).bind(now-24*60*60*1000).execute(&mut *tx).await?;
        retry_pending(&mut tx, now).await?;
        let settings = sqlx::query("SELECT * FROM notification_settings WHERE id=1")
            .fetch_one(&mut *tx)
            .await?;
        let sender: String = settings.get("sender");
        let recipient: String = settings.get("recipient");
        if client.is_none() {
            tx.commit().await?;
            return Ok(());
        }
        // A dedicated persisted send timestamp is unaffected by delivery polling.
        // Admit a due send every 30 seconds, using other cycles for status reads.
        let last_send: Option<i64> = settings.get("last_send_claim_at");
        let send_ready = last_send.is_none_or(|at| now.saturating_sub(at) >= 30_000);
        let queued = if send_ready
            && settings.get::<i64, _>("enabled") != 0
            && e164(&sender)
            && e164(&recipient)
        {
            sqlx::query("SELECT o.* FROM notification_outbox o JOIN attention_episodes e ON e.id=o.episode_id WHERE o.state='queued' AND o.next_attempt_at<=? AND o.settings_revision=? AND e.status='open' AND e.ack_json IS NULL ORDER BY o.next_attempt_at,o.id LIMIT 1").bind(now).bind(settings.get::<String,_>("revision")).fetch_optional(&mut *tx).await?
        } else {
            None
        };
        let (job, poll) = if let Some(queued) = queued {
            (Some(queued), false)
        } else {
            (sqlx::query("SELECT * FROM notification_outbox WHERE state='accepted' AND next_attempt_at<=? ORDER BY next_attempt_at,id LIMIT 1").bind(now).fetch_optional(&mut *tx).await?,true)
        };
        let Some(job) = job else {
            tx.commit().await?;
            return Ok(());
        };
        if poll {
            sqlx::query("UPDATE notification_outbox SET next_attempt_at=? WHERE id=?")
                .bind(now + 60_000)
                .bind(job.get::<String, _>("id"))
                .execute(&mut *tx)
                .await?;
        } else {
            sqlx::query("UPDATE notification_outbox SET state='sending',attempts=attempts+1,updated_at=? WHERE id=? AND state='queued'").bind(now).bind(job.get::<String,_>("id")).execute(&mut *tx).await?;
            sqlx::query("UPDATE notification_settings SET last_send_claim_at=? WHERE id=1")
                .bind(now)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        (job, sender, recipient, poll)
    };
    // No SQLite transaction or writer guard survives into the HTTP operation.
    let message: Option<String> = job.get("message_sid");
    let body = format!(
        "Cider: a storage concern requires review. Attention {}. Open the dashboard for evidence and uncertainty.",
        job.get::<String, _>("episode_id")
    );
    let mut outcome = client
        .unwrap()
        .request(
            if poll { message.as_deref() } else { None },
            &sender,
            &recipient,
            &body,
        )
        .await;
    let attempts = job.get::<i64, _>("attempts") + i64::from(!poll);
    if poll && matches!(outcome.state, "queued" | "uncertain" | "failed") && outcome.sid.is_none() {
        outcome.state = "accepted";
    }
    if !poll && outcome.state == "queued" && attempts >= 4 {
        outcome.state = "failed";
        outcome.reason = Some("retry_limit");
    }
    let next = if outcome.state == "queued" {
        now + 30_000 * (1i64 << attempts.min(4))
    } else {
        now + 60_000
    };
    let _guard = state.writer.lock().await;
    sqlx::query("UPDATE notification_outbox SET state=?,next_attempt_at=?,updated_at=?,message_sid=COALESCE(?,message_sid),provider_status=COALESCE(?,provider_status),reason=? WHERE id=?")
 .bind(outcome.state).bind(next).bind(now).bind(outcome.sid).bind(outcome.provider).bind(outcome.reason).bind(job.get::<String,_>("id")).execute(&state.db).await?;
    Ok(())
}
pub async fn run(state: AppState, stop: CancellationToken) {
    let client = Credentials::load().and_then(|c| Twilio::new(c).ok());
    let now = chrono::Utc::now().timestamp_millis();
    if recover(&state, now).await.is_err() {
        tracing::error!("Notification startup recovery failed");
        return;
    }
    if let Some(default) = client
        .as_ref()
        .and_then(|c| c.credentials.default_sender.as_ref())
    {
        let _guard = state.writer.lock().await;
        if sqlx::query("UPDATE notification_settings SET sender=? WHERE id=1 AND revision='initial' AND sender='' AND enabled=0").bind(default).execute(&state.db).await.is_err(){tracing::error!("Notification sender initialization failed");}
    }
    let mut interval = tokio::time::interval(Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {_=stop.cancelled()=>break,_=interval.tick()=>{if step(&state,client.as_ref(),chrono::Utc::now().timestamp_millis()).await.is_err(){tracing::error!("Notification worker database operation failed");}}}
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn fixture(response: &str) -> Twilio {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let response = response.to_owned();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            let _ = socket.read(&mut request).await;
            socket.write_all(response.as_bytes()).await.unwrap();
        });
        let mut client = Twilio::new(Credentials {
            account: format!("AC{}", "0".repeat(32)),
            user: format!("AC{}", "0".repeat(32)),
            secret: "fixture-secret".into(),
            default_sender: None,
        })
        .unwrap();
        client.base = format!("http://{address}");
        client
    }
    #[tokio::test]
    async fn accepted_is_not_delivered_and_only_explicit_429_retries() {
        let payload = format!(r#"{{"sid":"SM{}","status":"queued"}}"#, "0".repeat(32));
        let c = fixture(&format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
            payload.len()
        ))
        .await;
        assert_eq!(
            c.request(None, "+15555550100", "+15555550101", "test")
                .await
                .state,
            "accepted"
        );
        let c = fixture("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n").await;
        assert_eq!(c.request(None, "", "", "").await.state, "queued");
        let c = fixture("HTTP/1.1 500 Server Error\r\nContent-Length: 0\r\n\r\n").await;
        assert_eq!(c.request(None, "", "", "").await.state, "uncertain");
    }
    #[tokio::test]
    async fn malformed_success_and_truncated_transport_are_uncertain() {
        let c = fixture("HTTP/1.1 201 Created\r\nContent-Length: 2\r\n\r\n{}").await;
        assert_eq!(c.request(None, "", "", "").await.state, "uncertain");
        let c = fixture("HTTP/1.1 201 Created\r\nContent-Length: 100\r\n\r\n{}").await;
        assert_eq!(c.request(None, "", "", "").await.state, "uncertain");
    }
    #[tokio::test]
    async fn delivery_poll_requires_matching_sid() {
        let id = format!("SM{}", "1".repeat(32));
        let payload = format!(r#"{{"sid":"{id}","status":"delivered"}}"#);
        let c = fixture(&format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{payload}",
            payload.len()
        ))
        .await;
        assert_eq!(c.request(Some(&id), "", "", "").await.state, "delivered");
    }
    async fn queued() -> (tempfile::TempDir, AppState) {
        let dir = tempfile::tempdir().unwrap();
        let state = AppState::open(&dir.path().join("test.db"), "admin")
            .await
            .unwrap();
        let mut tx = state.db.begin().await.unwrap();
        configure(&mut tx,&json!({"expected_revision":"initial","enabled":true,"sender":"+15555550100","recipient":"+15555550101"}),"actor",1000).await.unwrap();
        crate::attention::observe_condition(
            &mut tx,
            &crate::attention::Condition {
                key: "test".into(),
                kind: "reliability".into(),
                node_id: "n".into(),
                object_id: None,
                status: "open".into(),
                severity: "critical".into(),
                observation_state: "ok".into(),
                summary: "Warning".into(),
                evidence: json!({}),
            },
            2000,
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        (dir, state)
    }
    #[tokio::test]
    async fn restart_never_retries_inflight_send() {
        let (_d, state) = queued().await;
        sqlx::query("UPDATE notification_outbox SET state='sending'")
            .execute(&state.db)
            .await
            .unwrap();
        recover(&state, 3000).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT state FROM notification_outbox")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(status, "uncertain");
    }
    #[tokio::test]
    async fn retry_is_bounded_and_missing_credentials_do_not_look_delivered() {
        let (_d, state) = queued().await;
        step(&state, None, 3000).await.unwrap();
        let status: String = sqlx::query_scalar("SELECT state FROM notification_outbox")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(status, "queued");
        for n in 0..4 {
            let client =
                fixture("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n").await;
            step(&state, Some(&client), 100_000 + n * 1_000_000)
                .await
                .unwrap();
        }
        let row = sqlx::query("SELECT state,attempts FROM notification_outbox")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("state"), "failed");
        assert_eq!(row.get::<i64, _>("attempts"), 4);
    }
    #[test]
    fn credentials_support_account_and_api_key_without_mutating_environment() {
        let account = format!("AC{}", "0".repeat(32));
        let key = format!("SK{}", "1".repeat(32));
        let values = BTreeMap::from([
            ("TWILIO_ACCOUNT_SID", account.clone()),
            ("TWILIO_SID", key.clone()),
            ("TWILIO_SECRET", "fixture".into()),
        ]);
        let loaded = Credentials::from_values(|name| values.get(name).cloned()).unwrap();
        assert_eq!(loaded.account, account);
        assert_eq!(loaded.user, key);
        let values = BTreeMap::from([
            ("TWILIO_SID", account.clone()),
            ("TWILIO_SECRET", "fixture".into()),
        ]);
        assert_eq!(
            Credentials::from_values(|name| values.get(name).cloned())
                .unwrap()
                .user,
            account
        );
        let values = BTreeMap::from([("TWILIO_SID", key), ("TWILIO_SECRET", "fixture".into())]);
        assert!(Credentials::from_values(|name| values.get(name).cloned()).is_none());
    }
    #[tokio::test]
    async fn polling_rate_limit_preserves_sid_and_never_requeues_send() {
        let (_d, state) = queued().await;
        let id = format!("SM{}", "2".repeat(32));
        sqlx::query("UPDATE notification_outbox SET state='accepted',message_sid=?,attempts=1")
            .bind(&id)
            .execute(&state.db)
            .await
            .unwrap();
        let client = fixture("HTTP/1.1 429 Too Many Requests\r\nContent-Length: 0\r\n\r\n").await;
        step(&state, Some(&client), 3000).await.unwrap();
        let row = sqlx::query("SELECT state,message_sid,attempts FROM notification_outbox")
            .fetch_one(&state.db)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("state"), "accepted");
        assert_eq!(row.get::<String, _>("message_sid"), id);
        assert_eq!(row.get::<i64, _>("attempts"), 1);
    }
    #[tokio::test]
    async fn send_releases_database_lock_before_http() {
        let (_d, state) = queued().await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let handler_state = state.clone();
        let handler = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = vec![0; 8192];
            socket.read(&mut request).await.unwrap();
            let _guard = handler_state.writer.lock().await;
            let mut tx = handler_state.db.begin().await.unwrap();
            sqlx::query("UPDATE notification_settings SET worker_seen_at=42")
                .execute(&mut *tx)
                .await
                .unwrap();
            tx.commit().await.unwrap();
            socket
                .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });
        let mut client = Twilio::new(Credentials {
            account: format!("AC{}", "0".repeat(32)),
            user: format!("AC{}", "0".repeat(32)),
            secret: "fixture".into(),
            default_sender: None,
        })
        .unwrap();
        client.base = format!("http://{address}");
        tokio::time::timeout(Duration::from_secs(2), step(&state, Some(&client), 3000))
            .await
            .expect("DB writer was held across HTTP")
            .unwrap();
        handler.await.unwrap();
    }
    #[tokio::test]
    async fn oversized_success_and_redirect_are_not_retried() {
        let body = "x".repeat(16385);
        let client = fixture(&format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
        .await;
        assert_eq!(client.request(None, "", "", "").await.state, "uncertain");
        let client = fixture(
            "HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:1/\r\nContent-Length: 0\r\n\r\n",
        )
        .await;
        assert_eq!(
            client.request(None, "", "", "").await.reason,
            Some("provider_outcome_unknown")
        );
    }
    #[tokio::test]
    async fn terminal_history_cannot_starve_new_notifications() {
        let (_d, state) = queued().await;
        let mut tx = state.db.begin().await.unwrap();
        let episode: String =
            sqlx::query_scalar("SELECT episode_id FROM notification_outbox LIMIT 1")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        sqlx::query("UPDATE notification_outbox SET state='suppressed'")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM n WHERE value<999) INSERT INTO notification_outbox(id,episode_id,transition_key,state,next_attempt_at,created_at,updated_at) SELECT 'history:'||value,?,'history:'||value,'suppressed',1000,1000,1000 FROM n").bind(&episode).execute(&mut *tx).await.unwrap();
        enqueue(&mut tx, &episode, "next-transition", 5000)
            .await
            .unwrap();
        let (count, queued): (i64, i64) =
            sqlx::query_as("SELECT COUNT(*),SUM(state='queued') FROM notification_outbox")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        assert_eq!(count, 1000);
        assert_eq!(queued, 1);
    }
    #[tokio::test]
    async fn due_polls_cannot_starve_sends_and_polling_runs_between_claims() {
        let (_d, state) = queued().await;
        let mut tx = state.db.begin().await.unwrap();
        let episode: String =
            sqlx::query_scalar("SELECT episode_id FROM notification_outbox LIMIT 1")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
        enqueue(&mut tx, &episode, "second", 2500).await.unwrap();
        let old_sid = format!("SM{}", "0".repeat(32));
        sqlx::query("WITH RECURSIVE n(value) AS (SELECT 1 UNION ALL SELECT value+1 FROM n WHERE value<12) INSERT INTO notification_outbox(id,episode_id,transition_key,state,attempts,next_attempt_at,created_at,updated_at,message_sid) SELECT 'accepted:'||value,?,'accepted:'||value,'accepted',1,1000,1000,1000,? FROM n").bind(&episode).bind(&old_sid).execute(&mut *tx).await.unwrap();
        tx.commit().await.unwrap();
        for (now, expected_queued, message_sid) in [
            (30_000, 1, format!("SM{}", "9".repeat(32))),
            (35_000, 1, old_sid),
            (60_000, 0, format!("SM{}", "8".repeat(32))),
        ] {
            let body = json!({"sid":message_sid,"status":"queued"}).to_string();
            let client = fixture(&format!(
                "HTTP/1.1 201 Created\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ))
            .await;
            step(&state, Some(&client), now).await.unwrap();
            let queued: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox WHERE state='queued'")
                    .fetch_one(&state.db)
                    .await
                    .unwrap();
            assert_eq!(queued, expected_queued, "at {now}");
            if now == 35_000 {
                let polled:i64=sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox WHERE id LIKE 'accepted:%' AND next_attempt_at=95000").fetch_one(&state.db).await.unwrap();
                assert_eq!(polled, 1);
            }
        }
    }
    #[tokio::test]
    async fn full_outbox_intent_is_durable_and_admitted_once_after_capacity_frees() {
        let (_d, state) = queued().await;
        let episode: String =
            sqlx::query_scalar("SELECT episode_id FROM notification_outbox LIMIT 1")
                .fetch_one(&state.db)
                .await
                .unwrap();
        sqlx::query("WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<999) INSERT INTO notification_outbox(id,episode_id,transition_key,state,next_attempt_at,created_at,updated_at) SELECT 'busy:'||v,?,'busy:'||v,'accepted',100000,2000,2000 FROM n").bind(&episode).execute(&state.db).await.unwrap();
        let mut tx = state.db.begin().await.unwrap();
        let c = crate::attention::Condition {
            key: "waiting".into(),
            kind: "capacity".into(),
            node_id: "n".into(),
            object_id: None,
            status: "open".into(),
            severity: "warning".into(),
            observation_state: "ok".into(),
            summary: "Capacity warning".into(),
            evidence: json!({}),
        };
        let waiting = crate::attention::observe_condition(&mut tx, &c, 3000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(waiting["notification_intent"]["state"], "pending_admission");
        tx.commit().await.unwrap();
        crate::attention::reconcile(&state, 4000).await.unwrap();
        assert_eq!(settings(&state).await.unwrap()["admission_limited"], true);
        sqlx::query("DELETE FROM notification_outbox WHERE id='busy:1'")
            .execute(&state.db)
            .await
            .unwrap();
        crate::attention::reconcile(&state, 5000).await.unwrap();
        crate::attention::reconcile(&state, 6000).await.unwrap();
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox WHERE episode_id=?")
                .bind(waiting["id"].as_str().unwrap())
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(count, 1);
        let (rows, _) = crate::attention::list(&state, &Default::default(), 6000)
            .await
            .unwrap();
        let row = rows
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == waiting["id"])
            .unwrap();
        assert!(row["notification_intent"].is_null());
        assert_eq!(row["notification"]["state"], "queued");
    }
    #[tokio::test]
    async fn orphaned_send_is_uncertain_on_next_serial_step_without_restart() {
        let (_d, state) = queued().await;
        sqlx::query("UPDATE notification_outbox SET state='sending',attempts=1")
            .execute(&state.db)
            .await
            .unwrap();
        step(&state, None, 3000).await.unwrap();
        let (status, attempts): (String, i64) =
            sqlx::query_as("SELECT state,attempts FROM notification_outbox")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(status, "uncertain");
        assert_eq!(attempts, 1);
    }
    #[tokio::test]
    async fn accepted_response_database_failure_recovers_without_duplicate_send() {
        let (_d, state) = queued().await;
        sqlx::query("CREATE TRIGGER reject_outcome BEFORE UPDATE ON notification_outbox WHEN NEW.state='accepted' BEGIN SELECT RAISE(ABORT,'fixture outcome write failure'); END").execute(&state.db).await.unwrap();
        let body = json!({"sid":format!("SM{}","0".repeat(32)),"status":"queued"}).to_string();
        let client = fixture(&format!(
            "HTTP/1.1 201 Created\r\nContent-Length: {}\r\n\r\n{body}",
            body.len()
        ))
        .await;
        assert!(step(&state, Some(&client), 3000).await.is_err());
        sqlx::query("DROP TRIGGER reject_outcome")
            .execute(&state.db)
            .await
            .unwrap();
        step(&state, None, 4000).await.unwrap();
        let (status, attempts, reason): (String, i64, String) =
            sqlx::query_as("SELECT state,attempts,reason FROM notification_outbox")
                .fetch_one(&state.db)
                .await
                .unwrap();
        assert_eq!(
            (status.as_str(), attempts, reason.as_str()),
            ("uncertain", 1, "outcome_not_recorded")
        );
    }
    #[tokio::test]
    async fn pending_admission_respects_ack_recovery_settings_expiry_and_disabled_creation() {
        for action in ["ack", "recovery", "settings", "expiry", "disabled"] {
            let (_d, state) = queued().await;
            let episode: String =
                sqlx::query_scalar("SELECT episode_id FROM notification_outbox LIMIT 1")
                    .fetch_one(&state.db)
                    .await
                    .unwrap();
            // All entries are active; accepted entries cannot be cancelled by configuration.
            sqlx::query("UPDATE notification_outbox SET state='accepted'")
                .execute(&state.db)
                .await
                .unwrap();
            sqlx::query("WITH RECURSIVE n(v) AS (SELECT 1 UNION ALL SELECT v+1 FROM n WHERE v<999) INSERT INTO notification_outbox(id,episode_id,transition_key,state,next_attempt_at,created_at,updated_at) SELECT 'busy:'||v,?,'busy:'||v,'accepted',100000,2000,2000 FROM n").bind(&episode).execute(&state.db).await.unwrap();
            let mut tx = state.db.begin().await.unwrap();
            if action == "disabled" {
                let revision: String =
                    sqlx::query_scalar("SELECT revision FROM notification_settings")
                        .fetch_one(&mut *tx)
                        .await
                        .unwrap();
                configure(
                    &mut tx,
                    &json!({"expected_revision":revision,"enabled":false}),
                    "admin",
                    2500,
                )
                .await
                .unwrap();
            }
            let mut c = crate::attention::Condition {
                key: "pending".into(),
                kind: "capacity".into(),
                node_id: "n".into(),
                object_id: None,
                status: "open".into(),
                severity: "warning".into(),
                observation_state: "ok".into(),
                summary: "Capacity warning".into(),
                evidence: json!({}),
            };
            let waiting = crate::attention::observe_condition(&mut tx, &c, 3000)
                .await
                .unwrap()
                .unwrap();
            let id = waiting["id"].as_str().unwrap();
            let expected = match action {
                "ack" => {
                    crate::attention::acknowledge(
                        &mut tx,
                        id,
                        waiting["revision"].as_str().unwrap(),
                        "reviewed",
                        "admin",
                        4000,
                    )
                    .await
                    .unwrap();
                    "acknowledged"
                }
                "recovery" => {
                    c.status = "resolved".into();
                    crate::attention::observe_condition(&mut tx, &c, 4000)
                        .await
                        .unwrap();
                    "condition_resolved"
                }
                "settings" | "disabled" => {
                    let revision: String =
                        sqlx::query_scalar("SELECT revision FROM notification_settings")
                            .fetch_one(&mut *tx)
                            .await
                            .unwrap();
                    configure(
                        &mut tx,
                        &json!({"expected_revision":revision,"enabled":true}),
                        "admin",
                        4000,
                    )
                    .await
                    .unwrap();
                    if action == "disabled" {
                        "disabled"
                    } else {
                        "configuration_changed"
                    }
                }
                "expiry" => "admission_expired",
                _ => unreachable!(),
            };
            sqlx::query("DELETE FROM notification_outbox WHERE id='busy:1'")
                .execute(&mut *tx)
                .await
                .unwrap();
            retry_pending(
                &mut tx,
                if action == "expiry" {
                    24 * 60 * 60 * 1000 + 4000
                } else {
                    5000
                },
            )
            .await
            .unwrap();
            let intent: String = sqlx::query_scalar(
                "SELECT notification_intent_json FROM attention_episodes WHERE id=?",
            )
            .bind(id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            let intent: Value = serde_json::from_str(&intent).unwrap();
            assert_eq!(intent["reason"], expected, "{action}");
            assert_eq!(
                intent["state"],
                if action == "expiry" {
                    "failed"
                } else {
                    "suppressed"
                },
                "{action}"
            );
            let count: i64 =
                sqlx::query_scalar("SELECT COUNT(*) FROM notification_outbox WHERE episode_id=?")
                    .bind(id)
                    .fetch_one(&mut *tx)
                    .await
                    .unwrap();
            assert_eq!(count, 0, "{action}");
        }
    }
}
