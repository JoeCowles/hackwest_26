use super::*;
use crate::config::Config;
use std::time::Duration;

#[test]
fn failure_delays_are_bounded_and_retry_after_is_respected() {
    let mut policy = RetryPolicy::new(Duration::from_secs(15), Duration::from_secs(60));
    for _ in 0..50 {
        let delay = policy.failed(None);
        assert!(delay >= Duration::from_secs(15));
        assert!(delay <= Duration::from_secs(60));
    }
    policy.succeeded();
    assert_eq!(
        policy.failed(Some(Duration::from_secs(45))),
        Duration::from_secs(45)
    );
    assert_eq!(
        policy.failed(Some(Duration::from_secs(999))),
        Duration::from_secs(60)
    );
    assert_eq!(retry_after("45"), Some(Duration::from_secs(45)));
    assert_eq!(retry_after("invalid"), None);
}

#[test]
fn private_ca_is_opt_in_absolute_bounded_and_nonempty() {
    let mut config = Config::parse(include_str!("../../examples/ciderd.toml")).unwrap();
    assert!(config.heartbeat.ca_certificate_file.is_none());
    config.heartbeat.ca_certificate_file = Some("relative.pem".into());
    assert!(config.validate().is_err());
    assert!(Sender::new(config.heartbeat.clone()).is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ca.pem");
    config.heartbeat.ca_certificate_file = Some(path.clone());
    std::fs::write(&path, b"not a certificate").unwrap();
    assert!(config.validate().is_ok());
    assert!(Sender::new(config.heartbeat.clone()).is_err());
    std::fs::write(&path, vec![b' '; 65537]).unwrap();
    assert!(Sender::new(config.heartbeat).is_err());
}

#[test]
fn authorization_rejects_newlines_and_is_marked_sensitive() {
    assert!(bearer_header("secret\r\nInjected: value").is_err());
    assert!(bearer_header("").is_err());
    assert!(bearer_header("valid-test-token").unwrap().is_sensitive());
}
