use ciderd::hardware::hardware_attributes;
#[test]
fn optional_model_preserves_raw_identifier_and_failure_is_unavailable() {
    let attrs = hardware_attributes(Ok("Mac14,7".into()), "2026-09-12T00:00:00Z");
    assert_eq!(attrs["model_identifier"], "Mac14,7");
    assert_eq!(attrs["hardware_state"], "ok");
    assert!(!attrs.contains_key("machine_family"));
    for model in [
        Err("failed".into()),
        Ok("".into()),
        Ok("a".repeat(129)),
        Ok("Mac\0bad".into()),
    ] {
        let attrs = hardware_attributes(model, "2026-09-12T00:00:00Z");
        assert_eq!(attrs["hardware_state"], "unavailable");
        assert_eq!(attrs["hardware_reason"], "model_query_failed");
        assert!(attrs["model_identifier"].is_null());
    }
}
#[test]
fn failed_optional_query_still_produces_a_valid_initial_heartbeat() {
    let mut hb =
        ciderd::runtime::initial_heartbeat("node", 1, "session", "boot", "test", "test", "test", 5);
    hb.resources[0].attributes.extend(hardware_attributes(
        Err("native failure".into()),
        &hb.created_at,
    ));
    hb.validate().unwrap();
    assert_eq!(hb.agent.heartbeat_interval_seconds, 5);
    assert_eq!(hb.boot_id, "boot");
}
