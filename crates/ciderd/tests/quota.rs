use ciderd::{
    config::Config,
    quota::{QuotaTarget, decode_reply, query},
};
use serde_json::json;
use std::{net::UdpSocket, time::Duration};
fn words(xs: &[u32]) -> Vec<u8> {
    xs.iter().flat_map(|x| x.to_be_bytes()).collect()
}
fn reply(fields: &[u32]) -> Vec<u8> {
    let mut out = words(&[42, 1, 0, 0, 0, 0]);
    out.extend(words(fields));
    out
}
fn target(port: u16) -> QuotaTarget {
    serde_json::from_value(
        json!({"server":"127.0.0.1","export_path":"/quota","uid":501,"port":port}),
    )
    .unwrap()
}
#[test]
fn exact_block_conversion_and_expired_grace() {
    let v = decode_reply(
        &reply(&[
            1,
            2_147_483_647,
            1,
            u32::MAX,
            0,
            u32::MAX,
            100,
            50,
            9,
            u32::MAX,
            0,
        ]),
        42,
    )
    .unwrap();
    assert_eq!(v["used_bytes"], "9223372030412324865");
    assert_eq!(v["block_hard_limit_bytes"], "9223372030412324865");
    assert_eq!(v["block_soft_limit_bytes"], "0");
    assert_eq!(v["block_grace_seconds_raw"], "4294967295");
    assert_eq!(v["block_grace_seconds_signed"], "-1");
    assert_eq!(v["active"], true);
}
#[test]
fn noquota_and_denial_have_no_usage_or_limits() {
    for (code, status) in [(2, "no_quota"), (3, "permission_denied")] {
        let v = decode_reply(&reply(&[code]), 42).unwrap();
        assert_eq!(v["status"], status);
        assert!(v.get("used_bytes").is_none());
    }
}
#[test]
fn malformed_responses_fail_closed() {
    let good = reply(&[1, 1024, 1, 2, 1, 1, 4, 3, 2, 0, 0]);
    for n in 0..good.len() {
        assert!(decode_reply(&good[..n], 42).is_err(), "truncation {n}");
    }
    assert!(decode_reply(&good, 43).is_err());
    for fields in [
        vec![4],
        vec![1, 0, 1, 2, 1, 1, 4, 3, 2, 0, 0],
        vec![1, u32::MAX, 1, 2, 1, 1, 4, 3, 2, 0, 0],
        vec![1, 1024, 2, 2, 1, 1, 4, 3, 2, 0, 0],
        vec![2, 123],
    ] {
        assert!(decode_reply(&reply(&fields), 42).is_err());
    }
}
#[test]
fn rpc_rejections_are_explicit() {
    assert_eq!(
        decode_reply(&words(&[42, 1, 0, 0, 0, 1]), 42).unwrap()["status"],
        "unsupported"
    );
    assert_eq!(
        decode_reply(&words(&[42, 1, 1, 1, 1]), 42).unwrap()["status"],
        "permission_denied"
    );
}
#[test]
fn config_bounds_targets_and_rejects_duplicate_subjects() {
    let base = include_str!("../examples/ciderd.toml");
    let q =
        "\n[[nfs_quotas.targets]]\nserver='127.0.0.1'\nexport_path='/quota'\nuid=501\nport=875\n";
    let conf = Config::parse(&format!("{base}{q}")).unwrap();
    assert_eq!(conf.nfs_quotas.targets.len(), 1);
    assert!(Config::parse(&format!("{base}{q}{q}")).is_err());
    assert!(Config::parse(&format!("{base}{}", q.replace("uid=501", "uid=4294967295"))).is_err());
    assert!(Config::parse(&format!("{base}{}", q.replace("port=875", "port=0"))).is_err());
    assert!(
        Config::parse(&format!(
            "{base}{}",
            q.replace("export_path='/quota'", "export_path='relative'")
        ))
        .is_err()
    );
}
#[test]
fn udp_query_uses_real_identity_and_target_uid_separately() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let t = target(sock.local_addr().unwrap().port());
    let h = std::thread::spawn(move || {
        let mut b = [0u8; 2048];
        let (n, p) = sock.recv_from(&mut b).unwrap();
        let b = &b[..n];
        let u = |offset| u32::from_be_bytes(b[offset..offset + 4].try_into().unwrap());
        assert_eq!(u(12), 100011);
        assert_eq!(u(16), 1);
        assert_eq!(u(20), 1);
        assert_eq!(u(24), 1);
        let host_len = u(36) as usize;
        let off = 40 + host_len.div_ceil(4) * 4;
        assert_eq!(u(off), ciderd::platform::rpc_identity().unwrap().uid);
        assert_eq!(u(n - 4), 501);
        let mut r = words(&[u(0), 1, 0, 0, 0, 0, 2]);
        sock.send_to(&r, p).unwrap();
        r.clear();
    });
    let v = query(&t, Duration::from_millis(300));
    h.join().unwrap();
    assert_eq!(v["status"], "no_quota");
}
#[test]
fn udp_deadline_does_not_become_zero_usage() {
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    let start = std::time::Instant::now();
    let v = query(
        &target(sock.local_addr().unwrap().port()),
        Duration::from_millis(40),
    );
    assert_eq!(v["status"], "timeout");
    assert!(v.get("used_bytes").is_none());
    assert!(start.elapsed() < Duration::from_secs(1));
}
#[test]
fn collector_publishes_failures_without_zero_limits() {
    for status in [
        "no_quota",
        "permission_denied",
        "timeout",
        "unavailable",
        "unsupported",
        "parse_error",
    ] {
        let c = ciderd::collectors::parse_nfs_quota(
            serde_json::to_string(&json!({"status":status}))
                .unwrap()
                .as_bytes(),
            "quota-id",
        )
        .unwrap();
        assert_eq!(c.samples.len(), 1);
        assert_eq!(c.samples[0].metrics[0].name, "storage.nfs.quota.status");
        assert_eq!(c.samples[0].metrics[0].value, Some(json!(status)));
        assert!(
            c.samples[0]
                .metrics
                .iter()
                .skip(1)
                .all(|m| m.value.is_none())
        );
    }
}
#[test]
fn collector_resource_contract_and_integer_metrics() {
    let t = target(875);
    let r = t.resource("node-a");
    r.validate().unwrap();
    assert_eq!(r.attributes["uid"], "501");
    assert_ne!(r.resource_id, t.resource("node-b").resource_id);
    let v = decode_reply(&reply(&[1, 1024, 1, 2048, 1024, 64, 20, 10, 2, 0, 0]), 42).unwrap();
    let c = ciderd::collectors::parse_nfs_quota(&serde_json::to_vec(&v).unwrap(), &r.resource_id)
        .unwrap();
    assert_eq!(c.samples[0].status, "ok");
    for m in &c.samples[0].metrics {
        m.validate().unwrap();
    }
    let m = c.samples[0]
        .metrics
        .iter()
        .find(|m| m.name == "storage.nfs.quota.used_bytes")
        .unwrap();
    assert_eq!(m.value, Some(json!("65536")));
    assert!(
        ciderd::collectors::parse_nfs_quota(br#"{"status":"available"}"#, &r.resource_id).is_err()
    );
}
#[test]
fn portmapper_preserves_rpc_failure_and_validates_registered_port() {
    use ciderd::quota::decode_portmapper;
    assert_eq!(
        decode_portmapper(&words(&[42, 1, 1, 1, 1]), 42).unwrap()["status"],
        "permission_denied"
    );
    assert_eq!(
        decode_portmapper(&words(&[42, 1, 0, 0, 0, 1]), 42).unwrap()["status"],
        "unsupported"
    );
    assert_eq!(
        decode_portmapper(&words(&[42, 1, 0, 0, 0, 0, 0]), 42).unwrap()["status"],
        "unsupported"
    );
    assert_eq!(
        decode_portmapper(&words(&[42, 1, 0, 0, 0, 0, 875]), 42).unwrap()["port"],
        875
    );
    assert!(decode_portmapper(&words(&[42, 1, 0, 0, 0, 0, 65536]), 42).is_err());
    assert!(decode_portmapper(&words(&[42, 1, 0, 0, 0, 0, 875, 0]), 42).is_err());
}
#[test]
fn recorded_real_rquotad_responses_match_independent_user_quota_accounting() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/rquota-live.json")).unwrap();
    for row in fixture["observations"].as_array().unwrap() {
        let hex = row["reply_hex"].as_str().unwrap();
        let bytes: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|n| u8::from_str_radix(&hex[n..n + 2], 16).unwrap())
            .collect();
        let decoded = decode_reply(&bytes, row["xid"].as_u64().unwrap() as u32).unwrap();
        for key in [
            "status",
            "active",
            "used_bytes",
            "block_size_bytes",
            "block_soft_limit_bytes",
            "block_hard_limit_bytes",
            "used_inodes",
            "inode_soft_limit",
            "inode_hard_limit",
            "block_grace_seconds_raw",
            "inode_grace_seconds_raw",
        ] {
            assert_eq!(decoded[key], row[key], "UID {} field {key}", row["uid"]);
        }
        let collected = ciderd::collectors::parse_nfs_quota(
            &serde_json::to_vec(&decoded).unwrap(),
            "fixture-quota",
        )
        .unwrap();
        assert_eq!(
            collected.samples[0].metrics[0].value,
            Some(row["status"].clone())
        );
    }
}
