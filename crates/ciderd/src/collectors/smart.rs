use super::*;
use serde_json::json;

fn smart_uint(value: &Value, field: &str) -> Result<Option<u128>> {
    // smartctl --json=v provides exact string companions for wide integers. Prefer
    // them because older numeric output may already have lost precision.
    if let Some(exact) = value.get(format!("{field}_s")) {
        return Ok(Some(uint(exact)?));
    }
    value.get(field).map(uint).transpose()
}

fn identity_text(value: Option<&Value>, limit: usize) -> Result<Option<&str>> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .context("invalid reported SMART identity field")?;
    ensure!(
        value.len() <= limit && !value.chars().any(char::is_control),
        "reported SMART identity exceeds bounds"
    );
    let value = value.trim();
    Ok((!value.is_empty()).then_some(value))
}

fn identity_epoch(value: &Value, caller_epoch: &str) -> Result<(String, &'static str)> {
    crate::model::check_id(caller_epoch)?;
    if let Some(wwn) = value.get("wwn") {
        let identity = if let Some(fields) = wwn.as_object() {
            ensure!(fields.len() <= 8, "reported WWN exceeds bounds");
            let naa = uint(fields.get("naa").context("reported WWN lacks NAA")?)?;
            let oui = uint(fields.get("oui").context("reported WWN lacks OUI")?)?;
            let id = uint(fields.get("id").context("reported WWN lacks ID")?)?;
            ensure!(
                naa <= 15 && oui <= 0x00ff_ffff && id <= u64::MAX as u128,
                "reported WWN component exceeds bounds"
            );
            // All-zero WWNs do not establish identity; use serial metadata if available.
            (naa != 0 || oui != 0 || id != 0).then(|| format!("{naa:x}:{oui:06x}:{id:016x}"))
        } else {
            let text = identity_text(Some(wwn), 128)?;
            if let Some(text) = text {
                let hex = text
                    .strip_prefix("0x")
                    .or_else(|| text.strip_prefix("0X"))
                    .unwrap_or(text)
                    .replace(':', "");
                ensure!(
                    !hex.is_empty()
                        && hex.len() <= 32
                        && hex.bytes().all(|b| b.is_ascii_hexdigit()),
                    "invalid reported WWN encoding"
                );
                let number =
                    u128::from_str_radix(&hex, 16).context("reported WWN exceeds bounds")?;
                (number != 0).then(|| format!("{number:032x}"))
            } else {
                None
            }
        };
        if let Some(identity) = identity {
            return Ok((
                scoped_id(caller_epoch, "", "smart-counter-wwn", &identity),
                "reported_wwn",
            ));
        }
    }
    if let Some(serial) = identity_text(value.get("serial_number"), 256)? {
        let model = identity_text(value.get("model_name"), 256)?;
        let protocol = identity_text(value.pointer("/device/protocol"), 32)?;
        let identity = serde_json::to_string(&(serial, model, protocol))?;
        return Ok((
            scoped_id(
                caller_epoch,
                "",
                "smart-counter-serial-model-protocol",
                &identity,
            ),
            "reported_serial",
        ));
    }
    Ok((caller_epoch.to_owned(), "caller_epoch_only"))
}

pub fn parse_smart(
    bytes: &[u8],
    exit_code: u8,
    resource_id: &str,
    epoch: &str,
) -> Result<Collected> {
    let value = json(bytes)?;
    let smartctl = value
        .get("smartctl")
        .filter(|v| v.is_object())
        .context("missing smartctl metadata")?;
    if let Some(reported) = smartctl.get("exit_status") {
        ensure!(
            uint(reported)? == exit_code as u128,
            "smartctl exit status disagrees with process"
        );
    }
    let version = smartctl
        .get("version")
        .and_then(Value::as_array)
        .context("missing smartctl version")?;
    ensure!(
        !version.is_empty() && version.len() <= 4,
        "invalid smartctl version"
    );
    let version = version
        .iter()
        .map(|v| uint(v).map(|n| n.to_string()))
        .collect::<Result<Vec<_>>>()?
        .join(".");
    let mut result = Collected::complete();
    let mut metrics = Vec::new();
    let mut owner = resource_id.to_owned();
    // Only a fixed-size hash enters the epoch. Reported serial/model/WWN values
    // never become routine metric labels, resource IDs, or extension payloads.
    let (mut scoped_epoch, counter_identity) = identity_epoch(&value, epoch)?;
    let nvme = value.get("nvme_smart_health_information_log");
    let mut unknown_scope = false;
    if let Some(nvme) = nvme {
        ensure!(nvme.is_object(), "invalid NVMe health log");
        if let Some(nsid) = nvme.get("nsid") {
            let aggregate = nsid.as_i64() == Some(-1)
                || nsid.as_str() == Some("-1")
                || uint(nsid).ok() == Some(u32::MAX as u128);
            if !aggregate {
                let nsid = uint(nsid)?;
                ensure!(
                    nsid > 0 && nsid < u32::MAX as u128,
                    "invalid NVMe namespace scope"
                );
                owner = scoped_id(resource_id, "", "nvme-namespace", &nsid.to_string());
                scoped_epoch = scoped_id(&scoped_epoch, "", "nvme-namespace", &nsid.to_string());
                result.resource(Resource::new(owner.clone(),"media",crate::model::attrs(json!({"namespace_id":nsid.to_string(),"smart_owner_id":resource_id,"scope":"nvme_namespace","source":"smartctl"}))))?;
                result
                    .relationships
                    .push(relation(resource_id, &owner, "contains"));
            }
        } else {
            unknown_scope = true;
        }
    }
    if !unknown_scope {
        if let Some(status) = value.get("smart_status") {
            let passed = status
                .get("passed")
                .and_then(Value::as_bool)
                .context("invalid SMART overall status")?;
            metrics.push(Metric::reading(
                "storage.media.smart_passed",
                passed.into(),
                "live",
                None,
            )?);
        }
        if let Some(temperature) = value.get("temperature").and_then(|v| v.get("current")) {
            let celsius = temperature
                .as_f64()
                .filter(|v| v.is_finite())
                .context("invalid Celsius temperature")?;
            // smartctl already normalizes NVMe temperature from Kelvin to Celsius.
            ensure!(
                (-273.15..=1000.0).contains(&celsius),
                "temperature outside physical/source bound"
            );
            metrics.push(Metric::reading(
                "storage.media.temperature_celsius",
                temperature.clone(),
                "live",
                None,
            )?);
        }
        if let Some(nvme) = nvme {
            for (field, name) in [
                ("critical_warning", "critical_warning_bits"),
                ("available_spare", "available_spare_percent"),
                (
                    "available_spare_threshold",
                    "available_spare_threshold_percent",
                ),
                ("percentage_used", "endurance_used_percent"),
                ("data_units_read", "data_units_read_total"),
                ("data_units_written", "data_units_written_total"),
                ("host_reads", "host_read_commands_total"),
                ("host_writes", "host_write_commands_total"),
                ("controller_busy_time", "controller_busy_minutes_total"),
                ("power_cycles", "power_cycles_total"),
                ("power_on_hours", "power_on_hours_total"),
                ("unsafe_shutdowns", "unsafe_shutdowns_total"),
                ("media_errors", "media_errors_total"),
                ("num_err_log_entries", "error_log_entries_total"),
            ] {
                if let Some(value) = smart_uint(nvme, field)? {
                    let counter = name.ends_with("_total");
                    let mut metric = Metric::integer(
                        &format!("storage.nvme.{name}"),
                        value,
                        "live",
                        counter.then_some(scoped_epoch.as_str()),
                    )?;
                    if counter {
                        // This is provenance, not a series dimension. A live
                        // reading can still have only caller-scoped identity.
                        metric.extensions = Some(crate::model::attrs(
                            json!({"counter_identity":counter_identity}),
                        ));
                    }
                    metrics.push(metric);
                }
            }
        }
    }
    let standby = value
        .get("power_mode")
        .and_then(Value::as_str)
        .is_some_and(|v| matches!(v, "STANDBY" | "SLEEP"));
    // Low three bits indicate acquisition problems. Higher health/history bits
    // do not invalidate successfully decoded readings.
    let status = if metrics.is_empty() {
        if standby {
            "skipped"
        } else if unknown_scope {
            "partial"
        } else if exit_code & 7 != 0 {
            "failed"
        } else {
            "unsupported"
        }
    } else if exit_code & 7 != 0 {
        "partial"
    } else {
        "ok"
    };
    result.complete = status == "ok";
    result.sample(
        &owner,
        "smartctl",
        metrics,
        status,
        &format!("smartctl-{version}-json-v1"),
    );
    result.samples.last_mut().expect("just inserted").exit_code = Some(exit_code);
    validate_samples(&result)?;
    Ok(result)
}
