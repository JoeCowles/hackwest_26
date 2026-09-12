use super::*;
use serde_json::json;

fn disk_id(node: &str, boot: &str, bsd: &str) -> String {
    scoped_id(node, boot, "diskutil-media", bsd)
}
fn filesystem_id(node: &str, boot: &str, bsd: &str, uuid: Option<&str>) -> String {
    scoped_id(
        node,
        boot,
        "filesystem",
        &format!("{}:{bsd}", uuid.unwrap_or("unresolved")),
    )
}
fn bsd(value: &Value, field: &str) -> Result<String> {
    let name = text(value, field)?;
    ensure!(
        name.starts_with("disk")
            && name.len() <= 64
            && name.len() > 4
            && name[4..]
                .split('s')
                .all(|part| !part.is_empty() && part.bytes().all(|v| v.is_ascii_digit())),
        "invalid BSD disk locator"
    );
    Ok(name.into())
}

/// Accepts `diskutil list -plist physical` only. The fixed command selection supplies
/// physical provenance that the plist itself cannot distinguish from disk images.
pub fn parse_inventory(bytes: &[u8], node: &str, boot: &str) -> Result<Collected> {
    let value = plist(bytes)?;
    let mut result = Collected::complete();
    for disk in array(&value, "AllDisksAndPartitions")? {
        let name = bsd(disk, "DeviceIdentifier")?;
        ensure!(
            disk.get("APFSVolumes").is_none(),
            "physical inventory unexpectedly contains a synthesized APFS disk"
        );
        let id = disk_id(node, boot, &name);
        let mut fields = crate::model::attrs(
            json!({"bsd_name":name,"smart_eligible":true,"source":"diskutil.list.physical","identity_policy":"boot_locator"}),
        );
        for (input, output) in [
            ("Content", "content"),
            ("Size", "size_bytes"),
            ("OSInternal", "os_internal"),
        ] {
            attribute(&mut fields, disk, input, output);
        }
        result.resource(Resource::new(id.clone(), "physical_device", fields))?;
        if let Some(parts) = disk.get("Partitions") {
            for part in parts.as_array().context("Partitions is not an array")? {
                let name = bsd(part, "DeviceIdentifier")?;
                let part_id = disk_id(node, boot, &name);
                let mut fields = crate::model::attrs(
                    json!({"bsd_name":name,"source":"diskutil.list.physical","identity_policy":"boot_locator"}),
                );
                for (input, output) in [
                    ("Content", "content"),
                    ("Size", "size_bytes"),
                    ("DiskUUID", "reported_uuid"),
                ] {
                    attribute(&mut fields, part, input, output);
                }
                result.resource(Resource::new(part_id.clone(), "media", fields))?;
                result
                    .relationships
                    .push(relation(&id, &part_id, "contains"));
                if let Some(uuid) = part.get("VolumeUUID").and_then(Value::as_str) {
                    let fs_id = filesystem_id(node, boot, &name, Some(uuid));
                    let mut fields = crate::model::attrs(
                        json!({"bsd_name":name,"reported_uuid":uuid,"apfs":false,"source":"diskutil.list.physical"}),
                    );
                    attribute(&mut fields, part, "VolumeName", "volume_name");
                    attribute(&mut fields, part, "MountPoint", "mount_path");
                    result.resource(Resource::new(fs_id.clone(), "filesystem", fields))?;
                    result
                        .relationships
                        .push(relation(&fs_id, &part_id, "backed_by"));
                }
            }
        }
    }
    validate_samples(&result)?;
    Ok(result)
}

pub fn parse_apfs(bytes: &[u8], node: &str, boot: &str) -> Result<Collected> {
    let value = plist(bytes)?;
    let mut result = Collected::complete();
    let mut physical_stores = BTreeSet::new();
    for container in array(&value, "Containers")? {
        let name = bsd(container, "ContainerReference")?;
        let uuid = text(container, "APFSContainerUUID")?;
        // Including the locator prevents duplicated/cloned UUIDs from silently merging.
        let id = scoped_id(node, boot, "apfs-container", &format!("{uuid}:{name}"));
        result.resource(Resource::new(
            id.clone(),
            "apfs_container",
            crate::model::attrs(
                json!({"bsd_name":name,"reported_uuid":uuid,"source":"diskutil.apfs.list"}),
            ),
        ))?;
        let mut metrics = Vec::new();
        integer_field(
            &mut metrics,
            container,
            "CapacityCeiling",
            "storage.apfs.container_capacity_bytes",
            "live",
            None,
        )?;
        integer_field(
            &mut metrics,
            container,
            "CapacityFree",
            "storage.apfs.container_free_bytes",
            "live",
            None,
        )?;
        if let (Some(total), Some(free)) = (
            container.get("CapacityCeiling"),
            container.get("CapacityFree"),
        ) {
            ensure!(
                uint(free)? <= uint(total)?,
                "APFS container free capacity exceeds total"
            );
        }
        result.sample(
            &id,
            "apfs.accounting",
            metrics,
            "ok",
            "diskutil-apfs-plist-v1",
        );
        for store in array(container, "PhysicalStores")? {
            let name = bsd(store, "DeviceIdentifier")?;
            let store_id = disk_id(node, boot, &name);
            // Stores are explicit inventory endpoints even when reconciliation races list physical.
            if physical_stores.insert(store_id.clone()) {
                let mut fields = crate::model::attrs(
                    json!({"bsd_name":name,"source":"diskutil.apfs.list","identity_policy":"boot_locator"}),
                );
                attribute(&mut fields, store, "DiskUUID", "reported_uuid");
                attribute(&mut fields, store, "Size", "size_bytes");
                result.resource(Resource::new(store_id.clone(), "media", fields))?;
            }
            result
                .relationships
                .push(relation(&id, &store_id, "backed_by"));
        }
        for volume in array(container, "Volumes")? {
            let name = bsd(volume, "DeviceIdentifier")?;
            let uuid = text(volume, "APFSVolumeUUID")?;
            let volume_id = filesystem_id(node, boot, &name, Some(uuid));
            let mut fields = crate::model::attrs(
                json!({"bsd_name":name,"reported_uuid":uuid,"apfs":true,"source":"diskutil.apfs.list","capacity_pool_id":id}),
            );
            for (input, output) in [
                ("Name", "volume_name"),
                ("MountPoint", "mount_path"),
                ("Roles", "roles"),
                ("Locked", "locked"),
                ("FileVault", "filevault"),
            ] {
                attribute(&mut fields, volume, input, output);
            }
            result.resource(Resource::new(volume_id.clone(), "filesystem", fields))?;
            result
                .relationships
                .push(relation(&id, &volume_id, "contains"));
            let mut metrics = Vec::new();
            for (field, name) in [
                ("CapacityInUse", "storage.apfs.volume_used_bytes"),
                ("CapacityQuota", "storage.apfs.volume_quota_bytes"),
                ("CapacityReserve", "storage.apfs.volume_reserve_bytes"),
            ] {
                integer_field(&mut metrics, volume, field, name, "live", None)?;
            }
            result.sample(
                &volume_id,
                "apfs.accounting",
                metrics,
                "ok",
                "diskutil-apfs-plist-v1",
            );
        }
    }
    validate_samples(&result)?;
    Ok(result)
}

pub fn parse_snapshots(bytes: &[u8], resource_id: &str) -> Result<Collected> {
    let value = plist(bytes)?;
    let snapshots = array(&value, "Snapshots")?;
    let mut result = Collected::complete();
    for snapshot in snapshots {
        let uuid = text(snapshot, "SnapshotUUID")?;
        let id = scoped_id(resource_id, "", "apfs-snapshot", uuid);
        let mut fields = crate::model::attrs(
            json!({"reported_uuid":uuid,"source":"diskutil.apfs.listSnapshots"}),
        );
        for (input, output) in [
            ("Name", "snapshot_name"),
            ("XID", "transaction_id"),
            ("Purgeable", "purgeable"),
        ] {
            attribute(&mut fields, snapshot, input, output);
        }
        result.resource(Resource::new(id.clone(), "snapshot", fields))?;
        result
            .relationships
            .push(relation(&id, resource_id, "snapshot_of"));
    }
    result.sample(
        resource_id,
        "apfs.snapshots",
        vec![Metric::integer(
            "storage.apfs.snapshot_count",
            snapshots.len() as u128,
            "live",
            None,
        )?],
        "ok",
        "diskutil-snapshots-plist-v1",
    );
    Ok(result)
}

pub fn parse_iokit(bytes: &[u8], node: &str, boot: &str) -> Result<Collected> {
    let value = plist(bytes)?;
    let rows = value.as_array().context("IOKit output must be an array")?;
    let mut result = Collected::complete();
    for row in rows {
        let registry = uint(
            row.get("registry_id")
                .context("missing registry identity")?,
        )?;
        ensure!(registry <= u64::MAX as u128, "invalid registry identity");
        let id = scoped_id(node, boot, "iokit-driver", &registry.to_string());
        result.resource(Resource::new(id.clone(),"controller",crate::model::attrs(json!({"registry_entry_id":registry.to_string(),"source":"IOBlockStorageDriver","scope":"driver","smart_eligible":false}))))?;
        let stats = row
            .get("statistics")
            .filter(|v| v.is_object())
            .context("missing IOKit statistics dictionary")?;
        let epoch = scoped_id(node, boot, "iokit-counter", &registry.to_string());
        let mut metrics = Vec::new();
        for (direction, label) in [("read", "Read"), ("write", "Write")] {
            for (suffix, field) in [
                ("bytes_total", "Bytes"),
                ("operations_total", "Operations"),
                ("errors_total", "Errors"),
                ("retries_total", "Retries"),
                ("accounted_time_nanoseconds_total", "Total Time"),
            ] {
                integer_field(
                    &mut metrics,
                    stats,
                    &format!("{field} ({label})"),
                    &format!("storage.device.{direction}_{suffix}"),
                    "live",
                    Some(&epoch),
                )?;
            }
        }
        let status = if metrics.is_empty() {
            "unsupported"
        } else {
            "ok"
        };
        result.sample(
            &id,
            "iokit.block",
            metrics,
            status,
            "IOBlockStorageDriver-statistics-v1",
        );
    }
    validate_samples(&result)?;
    Ok(result)
}
