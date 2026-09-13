use crate::error::{ApiError, ApiResult};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{
    Row, Sqlite, SqlitePool, Transaction,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions},
};
use std::{
    path::Path,
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};
use uuid::Uuid;

#[derive(Clone, Default)]
pub struct Statistics {
    pub nodes: i64,
    pub samples: i64,
    pub batches: i64,
    pub last_maintenance: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct AppState {
    pub db: SqlitePool,
    pub admin_hash: String,
    pub viewer_token: String,
    pub viewer_expires_at: i64,
    pub writer: Arc<Mutex<()>>,
    pub permits: Arc<Semaphore>,
    pub statistics: Arc<RwLock<Statistics>>,
}

pub fn fingerprint(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn secret(prefix: &str) -> String {
    format!(
        "{prefix}_{}{}",
        Uuid::new_v4().simple(),
        Uuid::new_v4().simple()
    )
}
pub fn timestamp(millis: i64) -> String {
    chrono::DateTime::from_timestamp_millis(millis)
        .expect("valid timestamp")
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

impl AppState {
    pub async fn open(path: &Path, admin_token: &str) -> anyhow::Result<Self> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_secs(5));
        let db = SqlitePoolOptions::new()
            .max_connections(5)
            .acquire_timeout(Duration::from_secs(5))
            .connect_with(options)
            .await?;
        let version: i64 = sqlx::query_scalar("PRAGMA user_version")
            .fetch_one(&db)
            .await?;
        anyhow::ensure!(
            version <= 7,
            "Database schema is newer than this application"
        );
        let mut migration = db.begin().await?;
        if version == 0 {
            sqlx::raw_sql(include_str!("../migrations/001_init.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 2 {
            sqlx::raw_sql(include_str!("../migrations/002_ciderd.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 3 {
            sqlx::raw_sql(include_str!("../migrations/003_detection.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 4 {
            sqlx::raw_sql(include_str!("../migrations/004_reliability.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 5 {
            sqlx::raw_sql(include_str!("../migrations/005_attention.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 6 {
            sqlx::raw_sql(include_str!("../migrations/006_security_rules.sql"))
                .execute(&mut *migration).await?;
        }
        if version < 7 {
            sqlx::raw_sql(include_str!("../migrations/007_device_watch.sql"))
                .execute(&mut *migration).await?;
        }
        migration.commit().await?;
        Ok(Self {
            db,
            admin_hash: fingerprint(admin_token.as_bytes()),
            viewer_token: secret("viewer"),
            viewer_expires_at: Utc::now().timestamp_millis() + 8 * 60 * 60 * 1000,
            writer: Arc::new(Mutex::new(())),
            permits: Arc::new(Semaphore::new(64)),
            statistics: Arc::new(RwLock::new(Statistics::default())),
        })
    }

    /// The native application invokes the same token transaction as the HTTP API.
    pub async fn local_enrollment_token(&self) -> ApiResult<Value> {
        let _guard = self.writer.lock().await;
        let mut tx = self.db.begin().await?;
        let result = issue_token(&mut tx, 600).await?;
        tx.commit().await?;
        Ok(result)
    }
}

pub async fn issue_token(tx: &mut Transaction<'_, Sqlite>, ttl: u64) -> ApiResult<Value> {
    if !(30..=3600).contains(&ttl) {
        return Err(ApiError::field(
            "expires_in_seconds",
            "Expected 30-3600 seconds",
        ));
    }
    let token = secret("enroll");
    let expires = Utc::now().timestamp_millis() + ttl as i64 * 1000;
    sqlx::query("INSERT INTO enrollment_tokens(token_hash, expires_at) VALUES (?,?)")
        .bind(fingerprint(token.as_bytes()))
        .bind(expires)
        .execute(&mut **tx)
        .await?;
    Ok(json!({"enrollment_token":token,"expires_at":timestamp(expires),"single_use":true}))
}

pub async fn change(
    tx: &mut Transaction<'_, Sqlite>,
    event_type: &str,
    node: Option<&str>,
    payload: Value,
) -> ApiResult<i64> {
    Ok(sqlx::query(
        "INSERT INTO change_log(event_type,node_id,payload_json,committed_at) VALUES (?,?,?,?)",
    )
    .bind(event_type)
    .bind(node)
    .bind(payload.to_string())
    .bind(Utc::now().timestamp_millis())
    .execute(&mut **tx)
    .await?
    .last_insert_rowid())
}

pub async fn node_row(
    tx: &mut Transaction<'_, Sqlite>,
    node_id: &str,
) -> ApiResult<sqlx::sqlite::SqliteRow> {
    sqlx::query("SELECT * FROM nodes WHERE node_id=?")
        .bind(node_id)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| {
            ApiError::new(
                axum::http::StatusCode::NOT_FOUND,
                "not_found",
                "Node not found",
            )
        })
}

pub fn default_scope(kind: &str) -> &str {
    match kind {
        "node" => "node",
        "device" => "device",
        "apfs_container" => "container",
        "apfs_volume" => "volume",
        "mount" | "nfs_mount" => "mount",
        _ => "object",
    }
}

/// A rate needs two valid observations in the same boot and inventory generation.
pub fn counter_rate(
    current: &Value,
    state: &str,
    at: i64,
    previous: Option<&sqlx::sqlite::SqliteRow>,
) -> (Option<f64>, &'static str) {
    if state != "ok" {
        return (None, "unavailable");
    }
    let Some(previous) = previous else {
        return (None, "insufficient_data");
    };
    if previous.get::<String, _>("state") != "ok" {
        return (None, "unavailable");
    }
    let delta_ms = at - previous.get::<i64, _>("observed_at");
    let name: String = previous.get("name");
    let max_gap = if name.starts_with("node_swap_") || name.starts_with("nfs_") {
        90_000
    } else {
        15_000
    };
    if delta_ms <= 0 || delta_ms > max_gap {
        return (None, "unavailable");
    }
    let old: Value =
        serde_json::from_str(&previous.get::<String, _>("value_json")).unwrap_or(Value::Null);
    let delta =
        crate::model::counter(current).and_then(|v| v.checked_sub(crate::model::counter(&old)?));
    match delta {
        Some(delta) => (Some(delta as f64 * 1000.0 / delta_ms as f64), "ok"),
        None => (None, "reset"),
    }
}
