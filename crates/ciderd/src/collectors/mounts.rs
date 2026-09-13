use super::*;
use serde_json::json;

fn capacity_metrics(value: &Value, freshness: &str) -> Result<Vec<Metric>> {
    ensure!(value.is_object(), "missing capacity object");
    let size = value.get("block_size").map(uint).transpose()?;
    if let Some(size) = size {
        ensure!(
            size > 0 && size <= u32::MAX as u128,
            "invalid filesystem block size"
        );
    }
    let mut metrics = Vec::new();
    for (field, name) in [
        ("blocks", "storage.filesystem.total_bytes"),
        ("blocks_free", "storage.filesystem.free_bytes"),
        ("blocks_available", "storage.filesystem.available_bytes"),
    ] {
        if let (Some(size), Some(blocks)) = (size, value.get(field)) {
            let bytes = uint(blocks)?
                .checked_mul(size)
                .context("capacity multiplication overflow")?;
            metrics.push(Metric::integer(name, bytes, freshness, None)?);
        }
    }
    if let (Some(size), Some(total), Some(free)) =
        (size, value.get("blocks"), value.get("blocks_free"))
    {
        let used = uint(total)?
            .checked_sub(uint(free)?)
            .context("free blocks exceed total")?
            .checked_mul(size)
            .context("used capacity overflow")?;
        metrics.push(Metric::integer(
            "storage.filesystem.block_accounted_used_bytes",
            used,
            freshness,
            None,
        )?);
    }
    for (field, name) in [
        ("files", "storage.filesystem.reported_files"),
        ("files_free", "storage.filesystem.reported_free_files"),
    ] {
        integer_field(&mut metrics, value, field, name, freshness, None)?;
    }
    Ok(metrics)
}

pub fn parse_mounts(bytes: &[u8], node: &str, boot: &str) -> Result<Collected> {
    let value = json(bytes)?;
    ensure!(
        value.get("version").and_then(Value::as_u64) == Some(1),
        "unsupported mount worker version"
    );
    let mut result = Collected::complete();
    for mount in array(&value, "mounts")? {
        let fsid: [i32; 2] =
            serde_json::from_value(mount.get("fsid").context("missing fsid")?.clone())
                .context("invalid fsid")?;
        let path = text(mount, "mount_path")?;
        let source = text(mount, "source")?;
        let filesystem_type = text(mount, "filesystem_type")?;
        let raw_path = text(mount, "mount_path_hex")?;
        let raw_source = text(mount, "source_hex")?;
        ensure!(
            raw_path.len() <= 8192
                && raw_source.len() <= 8192
                && raw_path.len() % 2 == 0
                && raw_source.len() % 2 == 0
                && raw_path
                    .bytes()
                    .chain(raw_source.bytes())
                    .all(|v| v.is_ascii_hexdigit()),
            "invalid mount byte encoding"
        );
        let identity = serde_json::to_string(&(fsid, raw_path, raw_source, filesystem_type))?;
        let generation = scoped_id(node, boot, "mount-generation", &identity);
        let id = scoped_id(node, boot, "mount", &generation);
        let read_only = mount
            .get("read_only")
            .and_then(Value::as_bool)
            .context("missing mount flags")?;
        let local = mount
            .get("local")
            .and_then(Value::as_bool)
            .context("missing mount local flag")?;
        let attrs = crate::model::attrs(
            json!({"fsid":fsid,"mount_path":path,"source":source,"filesystem_type":filesystem_type,
            "mount_path_hex":raw_path,"source_hex":raw_source,"mount_generation":generation,"local":local,"source_api":"getfsstat-MNT_NOWAIT"}),
        );
        result.resource(Resource::new(id.clone(), "mount", attrs))?;
        let mut metrics = vec![Metric::reading(
            "storage.mount.read_only",
            read_only.into(),
            "cached",
            None,
        )?];
        metrics.extend(capacity_metrics(
            mount.get("capacity").context("missing cached capacity")?,
            "cached",
        )?);
        // All values share the cached getfsstat acquisition. A separate statfs
        // refresh owns filesystem.capacity's last-attempt and worker phase.
        result.sample(&id, "mount.inventory", metrics, "ok", "getfsstat-nowait-v1");
        if filesystem_type == "nfs" {
            let export_id = scoped_id(node, "", "nfs-export-observation", raw_source);
            if !result.resources.iter().any(|v| v.resource_id == export_id) {
                result.resource(Resource::new(export_id.clone(),"nfs_export",crate::model::attrs(json!({"configured_source":source,"source_hex":raw_source,"identity_policy":"host_source_observation"}))))?;
            }
            result
                .relationships
                .push(relation(&id, &export_id, "accesses"));
        }
    }
    validate_samples(&result)?;
    Ok(result)
}

pub fn parse_capacity(bytes: &[u8], resource_id: &str) -> Result<Collected> {
    let value = json(bytes)?;
    let status = text(&value, "status")?;
    ensure!(
        ["ok", "gone", "failed", "unsupported", "permission_denied"].contains(&status),
        "invalid capacity status"
    );
    let metrics = if status == "ok" {
        capacity_metrics(value.get("capacity").context("missing capacity")?, "live")?
    } else {
        Vec::new()
    };
    ensure!(
        status != "ok" || !metrics.is_empty(),
        "empty successful capacity result"
    );
    let mut result = Collected {
        complete: status == "ok",
        ..Collected::default()
    };
    result.sample(
        resource_id,
        "filesystem.capacity",
        metrics,
        status,
        "statfs-v1",
    );
    Ok(result)
}
