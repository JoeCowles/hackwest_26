use super::*;
/// Parse an isolated worker result. Failed observations carry unavailable metrics;
/// old cached values stay dated in State and are never relabelled as current quota.
pub fn parse_nfs_quota(bytes: &[u8], resource_id: &str) -> Result<Collected> {
    let value = json(bytes)?;
    let status = text(&value, "status")?;
    ensure!(
        [
            "available",
            "no_quota",
            "permission_denied",
            "timeout",
            "unavailable",
            "unsupported",
            "parse_error"
        ]
        .contains(&status),
        "invalid quota result"
    );
    let mut metrics = vec![Metric::reading(
        "storage.nfs.quota.status",
        status.into(),
        "live",
        None,
    )?];
    for field in [
        "used_bytes",
        "block_soft_limit_bytes",
        "block_hard_limit_bytes",
        "used_inodes",
        "inode_soft_limit",
        "inode_hard_limit",
        "block_grace_seconds_raw",
        "inode_grace_seconds_raw",
        "block_size_bytes",
        "active",
    ] {
        let name = format!("storage.nfs.quota.{field}");
        let metric = if status == "available" {
            let field_value = value
                .get(field)
                .context("incomplete available quota response")?;
            if field == "active" {
                Metric::reading(&name, field_value.clone(), "live", None)?
            } else {
                Metric::integer(&name, uint(field_value)?, "live", None)?
            }
        } else {
            let def = crate::model::catalog()
                .get(&name)
                .context("unknown quota metric")?;
            Metric {
                name,
                kind: def.kind.clone(),
                unit: def.unit.clone(),
                availability: match status {
                    "no_quota" => "not_collected",
                    "unavailable" => "failed",
                    other => other,
                }
                .into(),
                attributes: Attributes::new(),
                freshness: None,
                value_type: None,
                value: None,
                counter_epoch: None,
                reason: Some(status.into()),
                source_field: Some(def.source_field.clone()),
                effective_at: None,
                extensions: None,
            }
        };
        metric.validate()?;
        metrics.push(metric);
    }
    let mut collected = Collected::default();
    collected.sample(
        resource_id,
        "nfs.rquota",
        metrics,
        match status {
            "available" => "ok",
            "no_quota" => "partial",
            "unavailable" => "failed",
            other => other,
        },
        "ONC-RPC-rquota-v1-udp",
    );
    validate_samples(&collected)?;
    Ok(collected)
}
