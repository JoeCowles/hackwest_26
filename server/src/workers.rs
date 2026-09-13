use crate::{
    error::ApiResult,
    store::{AppState, Statistics, timestamp},
};
use chrono::Utc;
use sqlx::Row;
use tokio_util::sync::CancellationToken;

pub async fn maintain(state: &AppState, now: i64) -> ApiResult<()> {
    let _guard = state.writer.lock().await;
    let mut tx = state.db.begin().await?;
    // Retain a complete oldest hour so a partially pruned bucket is never rewritten.
    let cutoff = (now - 86_400_000) / 3_600_000 * 3_600_000;
    for resolution in [300_i64, 3600] {
        let bucket_ms = resolution * 1000;
        let upper = now / bucket_ms * bucket_ms;
        sqlx::query("INSERT INTO metric_rollups(resolution_seconds,bucket_start,object_id,boot_id,generation,name,kind,unit,source,scope,labels_json,min,max,mean,sample_count,valid_sample_count,state_counts_json)
          SELECT ?, (observed_at/?)*?,object_id,boot_id,generation,name,kind,unit,source,scope,labels_json,
          MIN(CASE WHEN kind='counter' THEN rate_per_second ELSE numeric_value END),
          MAX(CASE WHEN kind='counter' THEN rate_per_second ELSE numeric_value END),
          AVG(CASE WHEN kind='counter' THEN rate_per_second ELSE numeric_value END), COUNT(*),
          COUNT(CASE WHEN kind='counter' THEN rate_per_second ELSE numeric_value END),
          json_object('ok',SUM(state='ok'),'unsupported',SUM(state='unsupported'),'permission_denied',SUM(state='permission_denied'),
          'stale',SUM(state='stale'),'timeout',SUM(state='timeout'),'parse_error',SUM(state='parse_error'),'failed',SUM(state='failed'))
          FROM metric_samples WHERE observed_at>=? AND observed_at<? AND kind IN ('counter','gauge')
          GROUP BY (observed_at/?),object_id,boot_id,generation,name,kind,unit,source,scope,labels_json
          ON CONFLICT DO UPDATE SET min=excluded.min,max=excluded.max,mean=excluded.mean,sample_count=excluded.sample_count,
          valid_sample_count=excluded.valid_sample_count,state_counts_json=excluded.state_counts_json")
            .bind(resolution).bind(bucket_ms).bind(bucket_ms).bind(cutoff).bind(upper).bind(bucket_ms)
            .execute(&mut *tx).await?;
    }
    sqlx::query("DELETE FROM metric_samples WHERE observed_at<?")
        .bind(cutoff)
        .execute(&mut *tx)
        .await?;
    sqlx::query("UPDATE batches SET raw_json=NULL WHERE received_at<? AND raw_json IS NOT NULL")
        .bind(now - 86_400_000)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM batches WHERE received_at<?")
        .bind(now - 30 * 86_400_000)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM metric_rollups WHERE (resolution_seconds=300 AND bucket_start<?) OR (resolution_seconds=3600 AND bucket_start<?)")
        .bind(now-30*86_400_000).bind(now-365*86_400_000).execute(&mut *tx).await?;
    sqlx::query("DELETE FROM request_replays WHERE expires_at<?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM enrollment_tokens WHERE expires_at<?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM change_log WHERE committed_at<?")
        .bind(now - 86_400_000)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM events WHERE occurred_at<?")
        .bind(now - 30 * 86_400_000)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM inventory_generations WHERE generation NOT IN (SELECT generation FROM inventory_generations recent WHERE recent.node_id=inventory_generations.node_id ORDER BY generation DESC LIMIT 64)").execute(&mut *tx).await?;
    crate::cider_api::retain(&mut tx, now).await?;
    tx.commit().await?;
    Ok(())
}

pub async fn run(state: AppState, stop: CancellationToken) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(5));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut ticks = 0;
    let mut last_maintenance = None;
    let mut error = None;
    loop {
        tokio::select! { _=stop.cancelled()=>break, _=interval.tick()=>{} }
        if ticks % 12 == 0 {
            match maintain(&state, Utc::now().timestamp_millis()).await {
                Ok(()) => {
                    last_maintenance = Some(timestamp(Utc::now().timestamp_millis()));
                    error = None;
                }
                Err(e) => {
                    tracing::error!(error=?e,"Retention/rollup worker failed");
                    error = Some("Retention/rollup worker failed".to_owned());
                }
            }
        }
        ticks += 1;
        match sqlx::query("SELECT (SELECT COUNT(*) FROM nodes) AS nodes,(SELECT COUNT(*) FROM metric_samples) AS samples,(SELECT COUNT(*) FROM batches) AS batches").fetch_one(&state.db).await {
            Ok(row)=>{if let Ok(mut stats)=state.statistics.write(){*stats=Statistics{nodes:row.get("nodes"),samples:row.get("samples"),batches:row.get("batches"),last_maintenance:last_maintenance.clone(),error:error.clone()};}}
            Err(e)=>{tracing::error!(error=%e,"Statistics refresh failed");if let Ok(mut stats)=state.statistics.write(){stats.error=Some("Database unavailable".to_owned());}}
        }
    }
}
