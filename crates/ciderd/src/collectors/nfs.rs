use super::*;
use serde_json::json;

// Output labels from Apple's nfsstat JSON adapter. Families remain separate:
// a Compound RPC and its inner Read operation are not independent RPC totals.
const V3: &[&str] = &[
    "access", "commit", "create", "fsinfo", "fsstat", "getattr", "link", "lookup", "mkdir",
    "mknod", "null", "pathconf", "rdirplus", "read", "readdir", "readlink", "remove", "rename",
    "rmdir", "setattr", "symlink", "write",
];
const V4: &[&str] = &[
    "access",
    "close",
    "commit",
    "confirm",
    "create",
    "delegpurge",
    "delegreturn",
    "getattr",
    "getfh",
    "link",
    "lock",
    "lockt",
    "locku",
    "lookup",
    "lookupp",
    "nverify",
    "open",
    "open_conf",
    "open_dgrd",
    "openattr",
    "putfh",
    "putpubfh",
    "putrootfh",
    "read",
    "readdir",
    "readlink",
    "rel_lkowner",
    "remove",
    "rename",
    "renew",
    "restorefh",
    "savefh",
    "secinfo",
    "setattr",
    "setclientid",
    "verify",
    "write",
];
const CALLBACK: &[&str] = &["cb_getattr", "cb_recall"];

fn attributed_integer(
    name: &str,
    value: u128,
    epoch: &str,
    attributes: Attributes,
) -> Result<Metric> {
    let definition = crate::model::catalog()
        .get(name)
        .context("unknown NFS metric")?;
    let metric = Metric {
        name: name.into(),
        kind: definition.kind.clone(),
        unit: definition.unit.clone(),
        availability: "available".into(),
        attributes,
        freshness: Some("live".into()),
        value_type: Some("integer".into()),
        value: Some(value.to_string().into()),
        counter_epoch: Some(epoch.into()),
        reason: None,
        source_field: Some(definition.source_field.clone()),
        effective_at: None,
        extensions: None,
    };
    metric.validate()?;
    Ok(metric)
}

pub fn parse_nfs(bytes: &[u8], node: &str, boot: &str) -> Result<Collected> {
    let value = json(bytes)?;
    let client = value
        .get("Client Info")
        .filter(|v| v.is_object())
        .context("missing nfsstat Client Info")?;
    let id = scoped_id(node, "", "nfs-client", "macos");
    let epoch = scoped_id(node, boot, "nfs-client-counter", "macos");
    let mut metrics = Vec::new();
    let mut partial = false;
    for (table, family, operations) in [
        ("NFSv3 RPC Counts", "v3_procedure", V3),
        ("NFSv4 RPC Counts", "v4_rpc", &["compound", "null"][..]),
        ("NFSv4 Operation Counts", "v4_operation", V4),
        ("NFSv4 Callback Operation Counts", "v4_callback", CALLBACK),
    ] {
        let Some(table) = client.get(table) else {
            partial = true;
            continue;
        };
        let fields = table.as_object().context("invalid NFS operation table")?;
        if fields.len() != operations.len() {
            partial = true;
        }
        for (operation, value) in fields {
            let operation = operation.to_ascii_lowercase().replace(' ', "_");
            if !operations.contains(&operation.as_str()) {
                partial = true;
                continue;
            }
            metrics.push(attributed_integer(
                "storage.nfs.client.operations_total",
                uint(value)?,
                &epoch,
                crate::model::attrs(json!({"family":family,"operation":operation})),
            )?);
        }
    }
    // Callback RPC counts deliberately omitted: only callback inner operations are
    // represented in v4_callback, avoiding ambiguous duplicate series.
    if let Some(rpc) = client.get("RPC Info") {
        ensure!(rpc.is_object(), "invalid RPC table");
        for (field, name) in [
            ("TimedOut", "storage.nfs.client.rpc_timeouts_total"),
            ("Retries", "storage.nfs.client.rpc_retries_total"),
            ("Invalid", "storage.nfs.client.rpc_invalid_replies_total"),
            (
                "X Replies",
                "storage.nfs.client.rpc_unexpected_replies_total",
            ),
        ] {
            if rpc.get(field).is_none() {
                partial = true;
            }
            integer_field(&mut metrics, rpc, field, name, "live", Some(&epoch))?;
        }
    } else {
        partial = true;
    }
    if let Some(cache) = client.get("Cache Info") {
        ensure!(cache.is_object(), "invalid cache table");
        for (label, family) in [
            ("Accs", "accs"),
            ("Attr", "attr"),
            ("BioD", "biod"),
            ("BioR", "bior"),
            ("BioRL", "biorl"),
            ("BioW", "biow"),
            ("DirE", "dire"),
            ("Lkup", "lkup"),
        ] {
            for (label_suffix, metric_suffix) in [("Hits", "hits"), ("Misses", "misses")] {
                if let Some(value) = cache.get(format!("{label} {label_suffix}")) {
                    metrics.push(attributed_integer(
                        &format!("storage.nfs.client.cache_{metric_suffix}_total"),
                        uint(value)?,
                        &epoch,
                        crate::model::attrs(json!({"cache":family})),
                    )?);
                } else {
                    partial = true;
                }
            }
        }
    } else {
        partial = true;
    }
    ensure!(
        !metrics.is_empty(),
        "nfsstat contains no recognized counters"
    );
    let mut result = Collected {
        complete: !partial,
        ..Collected::default()
    };
    result.resource(Resource::new(
        id.clone(),
        "nfs_client",
        crate::model::attrs(
            json!({"implementation":"macos","scope":"host_client","source":"nfsstat.client"}),
        ),
    ))?;
    result.sample(
        &id,
        "nfsstat.client",
        metrics,
        if partial { "partial" } else { "ok" },
        "macos-nfsstat-json-v1",
    );
    validate_samples(&result)?;
    Ok(result)
}

pub fn parse_nfs_status(bytes: &[u8], resource_id: &str) -> Result<Collected> {
    let value = json(bytes)?;
    let status = text(&value, "status")?;
    ensure!(
        ["ok", "gone", "failed", "unsupported", "permission_denied"].contains(&status),
        "invalid NSTATUS status"
    );
    let mut result = Collected {
        complete: status == "ok",
        ..Collected::default()
    };
    let mut flags = Vec::new();
    let mut counts = Vec::new();
    if status == "ok" {
        for (field, name) in [
            ("not_responding", "storage.nfs.mount.not_responding"),
            ("dead", "storage.nfs.mount.dead"),
        ] {
            let value = value
                .get(field)
                .and_then(Value::as_bool)
                .context("missing native mount status flag")?;
            flags.push(Metric::reading(name, value.into(), "live", None)?);
        }
        for (field, name) in [
            (
                "outstanding_request_entries",
                "storage.nfs.mount.outstanding_request_entries",
            ),
            (
                "oldest_request_age_seconds",
                "storage.nfs.mount.oldest_request_age_seconds",
            ),
        ] {
            let value = uint(value.get(field).context("missing NSTATUS count")?)?;
            ensure!(
                value <= u32::MAX as u128,
                "native NSTATUS integer exceeds SDK field width"
            );
            counts.push(Metric::integer(name, value, "live", None)?);
        }
    }
    result.sample(
        resource_id,
        "nfs.mount_status",
        flags,
        status,
        "SDK-VFS_CTL_NSTATUS-v1",
    );
    result.sample(
        resource_id,
        "nfs.nstatus",
        counts,
        status,
        "SDK-VFS_CTL_NSTATUS-v1",
    );
    Ok(result)
}
