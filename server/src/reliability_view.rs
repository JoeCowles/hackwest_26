//! Evidence association uses the normalized, current physical inventory only.
use serde_json::{Value, json};

fn same_context(context: &Value, node: &Value) -> bool {
    ["boot_id", "agent_generation", "agent_session_id"]
        .iter()
        .all(|key| {
            context[*key].as_str().is_some_and(|v| !v.is_empty()) && context[*key] == node[*key]
        })
}
fn unique_owner(object: &Value, disk: &Value) -> bool {
    object["object_id"] == disk["object_id"]
        || object["physical_disk_ids"]
            .as_array()
            .is_some_and(|ids| ids.len() == 1 && ids[0] == disk["object_id"])
}
pub fn disk_assessment(
    node: &Value,
    disk: &Value,
    objects: &[Value],
    sources: &[Value],
    findings: &[Value],
) -> Value {
    let sources: Vec<Value> = sources
        .iter()
        .filter(|s| s["active"] == true && same_context(&s["acquisition"], node))
        .filter(|s| {
            let Some(object) = objects
                .iter()
                .find(|o| o["object_id"] == s["object_id"] && o["active"] == true)
            else {
                return false;
            };
            if object["topology_state"] != "resolved"
                || !same_context(&object["properties"]["ciderd_acquisition"], node)
                || !unique_owner(object, disk)
            {
                return false;
            }
            s["collector"] != "iokit.block"
                || (disk["io"]["linkage_state"] == "resolved"
                    && disk["io"]["driver_object_id"] == s["object_id"])
        })
        .cloned()
        .collect();
    let current_sources = sources
        .iter()
        .filter(|s| {
            s["support_state"] == "supported"
                && s["observation"]["state"] == "current"
                && node["availability"] == "online"
        })
        .count();
    let mut selected = Vec::new();
    let mut known = std::collections::BTreeSet::new();
    for source in &sources {
        let signals = source["signals"].as_array().cloned().unwrap_or_default();
        for signal in &signals {
            if source["support_state"] == "supported"
                && node["availability"] == "online"
                && signal["observation"]["state"] == "current"
                && matches!(
                    signal["state"].as_str(),
                    Some("no_current_warning" | "warning" | "open" | "recovering")
                )
            {
                if let Some(dimension) = signal["dimension"].as_str() {
                    known.insert(dimension.to_owned());
                }
            }
        }
        for finding in findings
            .iter()
            .filter(|f| f["source_id"] == source["source_id"] && f["status"] == "open")
        {
            let mut finding = finding.clone();
            finding["current"] = json!(
                source["support_state"] == "supported"
                    && node["availability"] == "online"
                    && signals
                        .iter()
                        .any(|s| s["finding_id"] == finding["finding_id"]
                            && s["observation"]["state"] == "current")
            );
            selected.push(finding);
        }
    }
    for finding in selected.iter().filter(|f|f["current"] != true) {
        if let Some(dimension)=finding["dimension"].as_str() {
            if !selected.iter().any(|f|f["current"]==true && f["dimension"]==dimension) {known.remove(dimension);}
        }
    }
    let assessment = ["critical", "warning"]
        .into_iter()
        .find(|severity| {
            selected
                .iter()
                .any(|f| f["current"] == true && f["severity"] == *severity)
        })
        .unwrap_or(if known.is_empty() || !selected.is_empty() {
            "unknown"
        } else {
            "no_current_warning"
        });
    let observation = if current_sources > 0 {
        "current"
    } else if sources.iter().any(|s| s["observation"]["state"] == "stale") {
        "stale"
    } else if sources.is_empty() {
        "unknown"
    } else {
        "unavailable"
    };
    json!({"assessment":assessment,"observation_state":observation,"source_count":sources.len(),"current_source_count":current_sources,"sources":sources,"findings":selected,
        "unknown_dimensions":(["media_health","performance"].into_iter().filter(|d|!known.contains(*d)).collect::<Vec<_>>()),"replacement_forecast":{"state":"insufficient_data","estimated_failure_at":null}})
}
pub fn apply_health(health: &mut Value, findings: &[Value]) {
    for dimension in ["media_health", "performance"] {
        let evidence: Vec<_> = findings
            .iter()
            .filter(|f| {
                f["current"] == true && f["status"] == "open" && f["dimension"] == dimension
            })
            .cloned()
            .collect();
        if evidence.is_empty() {
            continue;
        }
        let severity = if evidence.iter().any(|f| f["severity"] == "critical") {
            "critical"
        } else {
            "warning"
        };
        health["dimensions"][dimension] = json!({"status":severity,"reasons":["current_reliability_evidence"],"evidence":evidence});
        if severity == "critical" || health["overall"] != "critical" {
            health["overall"] = json!(severity);
        }
        if let Some(unknown) = health["unknown_dimensions"].as_array_mut() {
            unknown.retain(|v| v != dimension);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value, Vec<Value>, Vec<Value>, Vec<Value>) {
        let c = json!({"boot_id":"b","agent_generation":"1","agent_session_id":"s"});
        (
            json!({"boot_id":"b","agent_generation":"1","agent_session_id":"s","availability":"online"}),
            json!({"object_id":"disk","io":{"linkage_state":"resolved","driver_object_id":"driver"}}),
            vec![
                json!({"object_id":"driver","active":true,"topology_state":"resolved","physical_disk_ids":["disk"],"properties":{"ciderd_acquisition":c}}),
            ],
            vec![
                json!({"source_id":"src","object_id":"driver","active":true,"support_state":"supported","collector":"iokit.block","acquisition":c,"observation":{"state":"current"},"signals":[{"rule_id":"read","dimension":"media_health","state":"open","finding_id":"f","observation":{"state":"current"}}]}),
            ],
            vec![
                json!({"finding_id":"f","source_id":"src","rule_id":"read","status":"open","dimension":"media_health","severity":"warning"}),
            ],
        )
    }
    #[test]
    fn attributed_current_fault_warns_but_partial_or_stale_evidence_does_not() {
        let (n, d, mut o, mut s, f) = fixture();
        let v = disk_assessment(&n, &d, &o, &s, &f);
        assert_eq!(v["assessment"], "warning");
        assert_eq!(v["findings"][0]["current"], true);
        s[0]["signals"][0]["observation"]["state"] = json!("unavailable");
        let v = disk_assessment(&n, &d, &o, &s, &f);
        assert_eq!(v["assessment"], "unknown");
        assert_eq!(v["findings"][0]["current"], false);
        o[0]["physical_disk_ids"] = json!(["disk", "other"]);
        assert_eq!(disk_assessment(&n, &d, &o, &s, &f)["source_count"], 0);
    }
    #[test]
    fn prior_session_gauges_do_not_follow_reused_device_locator() {
        let (n, d, o, mut s, f) = fixture();
        s[0]["acquisition"]["agent_session_id"] = json!("old");
        let v = disk_assessment(&n, &d, &o, &s, &f);
        assert_eq!(v["assessment"], "unknown");
        assert_eq!(v["source_count"], 0);
    }
    #[test]
    fn another_clear_rule_cannot_reassure_over_unobserved_open_finding() {
        let (n, d, o, mut s, f) = fixture();
        s[0]["signals"][0]["observation"]["state"] = json!("unavailable");
        s[0]["signals"].as_array_mut().unwrap().push(json!({"rule_id":"another","dimension":"media_health","state":"no_current_warning","observation":{"state":"current"}}));
        let v = disk_assessment(&n, &d, &o, &s, &f);
        assert_eq!(v["assessment"], "unknown");
        assert!(
            v["unknown_dimensions"]
                .as_array()
                .unwrap()
                .contains(&json!("media_health"))
        );
    }
}
