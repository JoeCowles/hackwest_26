//! Hardware classification uses reported identifiers, never hostnames or prefixes.
use serde::{
    Deserialize, Deserializer,
    de::{Error, MapAccess, Visitor},
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fmt, sync::OnceLock};

#[derive(Deserialize)]
struct Model {
    machine_family: String,
    display_name: String,
    source: String,
}
#[derive(Deserialize)]
struct Catalog {
    version: String,
    #[serde(deserialize_with = "unique_models")]
    models: BTreeMap<String, Model>,
}

fn unique_models<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeMap<String, Model>, D::Error> {
    struct Models;
    impl<'de> Visitor<'de> for Models {
        type Value = BTreeMap<String, Model>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("unique exact hardware identifiers")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
            let mut result = BTreeMap::new();
            while let Some((key, model)) = map.next_entry::<String, Model>()? {
                if !valid_identifier(&key)
                    || !["macbook", "mac_mini", "imac"].contains(&model.machine_family.as_str())
                    || model.display_name.is_empty()
                    || model.display_name.len() > 256
                    || !model.source.starts_with("https://support.apple.com/")
                {
                    return Err(M::Error::custom("invalid hardware catalog entry"));
                }
                if result.insert(key, model).is_some() {
                    return Err(M::Error::custom("duplicate hardware identifier"));
                }
            }
            Ok(result)
        }
    }
    deserializer.deserialize_map(Models)
}

fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        serde_json::from_str(include_str!("../data/apple-hardware-models.json"))
            .expect("embedded hardware catalog must pass its validation tests")
    })
}
fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b',')
}

/// Classify optional host attributes while preserving their original provenance.
/// Family is a presentation hint; state and node availability remain independent.
pub fn hardware_view(attributes: &Value, acquisition: &Value, current_boot: Option<&str>) -> Value {
    let catalog = catalog();
    let identifier = attributes["model_identifier"]
        .as_str()
        .filter(|s| valid_identifier(s));
    let model = identifier.and_then(|id| catalog.models.get(id));
    let source = attributes["hardware_source"].as_str();
    let boot = acquisition["boot_id"].as_str();
    let reported_state = attributes["hardware_state"].as_str();
    let (state, reason) = if reported_state == Some("unavailable") {
        ("unavailable", Some("model_query_failed"))
    } else if identifier.is_none() {
        ("unknown", Some("model_not_reported"))
    } else if source != Some("sysctl.hw.model") || !matches!(reported_state, Some("ok" | "stale")) {
        ("unknown", Some("hardware_observation_unverified"))
    } else if boot.is_none() || current_boot.is_none() {
        ("unknown", Some("acquisition_context_missing"))
    } else if boot != current_boot || reported_state == Some("stale") {
        ("stale", Some("hardware_observation_stale"))
    } else if model.is_none() {
        ("unknown", Some("identifier_unmapped"))
    } else {
        ("ok", None)
    };
    let verified = matches!(state, "ok" | "stale");
    let family = if verified {
        model
            .map(|m| m.machine_family.as_str())
            .unwrap_or("unknown")
    } else {
        "unknown"
    };
    let display = model
        .map(|m| m.display_name.as_str())
        .or(identifier)
        .or_else(|| attributes["model"].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("Model not reported");
    let observed_at = attributes["hardware_observed_at"]
        .as_str()
        .or_else(|| acquisition["observed_at"].as_str());
    json!({"model_identifier":identifier,"machine_family":family,"display_name":display,
        "state":state,"reason":reason,"source":source,"observed_at":observed_at,
        "boot_id":boot,"catalog_version":catalog.version})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observed(identifier: &str) -> Value {
        json!({"model_identifier":identifier,"hardware_state":"ok",
            "hardware_source":"sysctl.hw.model","hardware_observed_at":"2026-09-12T12:00:00Z"})
    }
    fn context() -> Value {
        json!({"boot_id":"boot-a","observed_at":"2026-09-12T12:00:00Z"})
    }

    #[test]
    fn exact_identifiers_distinguish_overlapping_mac_prefixes() {
        for (identifier, family) in [
            ("Mac14,7", "macbook"),
            ("Mac14,3", "mac_mini"),
            ("Mac14,12", "mac_mini"),
            ("Mac16,3", "imac"),
            ("MacBookAir10,1", "macbook"),
            ("Macmini9,1", "mac_mini"),
            ("iMac21,1", "imac"),
        ] {
            let result = hardware_view(&observed(identifier), &context(), Some("boot-a"));
            assert_eq!(result["machine_family"], family, "{identifier}");
            assert_eq!(result["state"], "ok");
            assert_eq!(result["model_identifier"], identifier);
            assert_eq!(result["source"], "sysctl.hw.model");
            assert_eq!(result["observed_at"], "2026-09-12T12:00:00Z");
        }
    }

    #[test]
    fn unknown_identifiers_and_legacy_names_never_guess_a_mesh() {
        let unknown = hardware_view(&observed("Mac999,1"), &context(), Some("boot-a"));
        assert_eq!(unknown["machine_family"], "unknown");
        assert_eq!(unknown["state"], "unknown");
        assert_eq!(unknown["reason"], "identifier_unmapped");
        assert_eq!(unknown["display_name"], "Mac999,1");
        let legacy = hardware_view(
            &json!({"model":"MacBook Pro","hostname":"Mac mini"}),
            &Value::Null,
            None,
        );
        assert_eq!(legacy["machine_family"], "unknown");
        assert_eq!(legacy["state"], "unknown");
        assert_eq!(legacy["display_name"], "MacBook Pro");
    }

    #[test]
    fn failed_missing_and_old_boot_observations_keep_explicit_states() {
        let failed = hardware_view(
            &json!({"hardware_state":"unavailable",
            "hardware_reason":"model_query_failed","hardware_source":"sysctl.hw.model"}),
            &context(),
            Some("boot-a"),
        );
        assert_eq!(failed["state"], "unavailable");
        assert_eq!(failed["reason"], "model_query_failed");
        assert!(failed["model_identifier"].is_null());
        let missing = hardware_view(&json!({}), &Value::Null, None);
        assert_eq!(missing["state"], "unknown");
        assert_eq!(missing["display_name"], "Model not reported");
        let stale = hardware_view(&observed("Mac14,12"), &context(), Some("boot-b"));
        assert_eq!(stale["state"], "stale");
        assert_eq!(stale["boot_id"], "boot-a");
        assert_eq!(stale["machine_family"], "mac_mini");
        let no_provenance = hardware_view(&observed("Mac14,12"), &Value::Null, Some("boot-a"));
        assert_eq!(no_provenance["state"], "unknown");
        assert_eq!(no_provenance["reason"], "acquisition_context_missing");
    }

    #[test]
    fn malformed_or_untrusted_hardware_values_cannot_select_a_family() {
        for identifier in ["", "Mac14,12\n", "mac14,12", "Mac14", "Mac14,12 "] {
            let value = hardware_view(&observed(identifier), &context(), Some("boot-a"));
            assert_eq!(value["machine_family"], "unknown", "{identifier:?}");
        }
        let spoof = hardware_view(
            &json!({"machine_family":"mac_mini","model_identifier":"Mac999,1"}),
            &context(),
            Some("boot-a"),
        );
        assert_eq!(spoof["machine_family"], "unknown");
    }

    #[test]
    fn catalog_rejects_duplicate_identifiers_and_invalid_families() {
        let duplicate = r#"{"version":"test","models":{"Mac1,1":{"machine_family":"imac","display_name":"one","source":"https://support.apple.com/one"},"Mac1,1":{"machine_family":"mac_mini","display_name":"two","source":"https://support.apple.com/two"}}}"#;
        assert!(serde_json::from_str::<Catalog>(duplicate).is_err());
        let bad_family = r#"{"version":"test","models":{"Mac1,1":{"machine_family":"desktop","display_name":"one","source":"https://support.apple.com/one"}}}"#;
        assert!(serde_json::from_str::<Catalog>(bad_family).is_err());
        assert!(!catalog().models.is_empty());
        assert!(!catalog().version.is_empty());
    }
}
