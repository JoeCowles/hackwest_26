//! Optional source observation; hardware families are classified by the server.
use crate::model::{attrs, Attributes};
use serde_json::json;
pub fn hardware_attributes(model: Result<String, String>, observed_at: &str) -> Attributes {
    let model = model
        .ok()
        .filter(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control));
    let ok = model.is_some();
    attrs(
        json!({"model_identifier":model,"hardware_state":if ok {"ok"} else {"unavailable"},
        "hardware_source":"sysctl.hw.model","hardware_observed_at":observed_at,
        "hardware_reason":if ok {None} else {Some("model_query_failed")}}),
    )
}
