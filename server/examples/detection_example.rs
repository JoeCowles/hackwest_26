//! Deterministic synthetic examples for contract/browser verification; no I/O workload.
use orchard_server::{cider_wire::Decimal, detection::*, store};
use serde_json::{Value, json};

const BASE: i64 = 1_789_300_800_000; // Synthetic UTC wall clock.
const COUNTER: u128 = u64::MAX as u128 + 1_000_000;
fn observation(second: u64, counter: u128) -> Observation {
    Observation {
        collection_id: format!("synthetic-collection-{second}"),
        continuity: Continuity {
            boot_id: "synthetic-boot".into(),
            agent_generation: "1".into(),
            agent_session_id: "synthetic-session".into(),
            clock_id: "synthetic-monotonic".into(),
            counter_epoch: "synthetic-driver-epoch".into(),
            source_version: "synthetic-source-v1".into(),
            adapter_version: "synthetic-adapter-v1".into(),
            policy_version: POLICY_VERSION.into(),
        },
        finished_monotonic_ns: Decimal::from(u128::from(second) * 1_000_000_000),
        observed_at: store::timestamp(BASE + second as i64 * 1000),
        received_at_ms: BASE + second as i64 * 1000,
        counter: Some(counter.into()),
        unavailable_reason: None,
        age_at_receipt_seconds: 0.,
        stale_after_seconds: 15.,
    }
}
fn summary(state: &SourceState, second: u64) -> Value {
    let mut value = serde_json::to_value(state.summary(BASE + second as i64 * 1000, true)).unwrap();
    value["baseline"]["as_of"] = json!(store::timestamp(BASE + second as i64 * 1000));
    value
}
fn main() {
    let mut state = SourceState::new(SourceIdentity {
        source_id: "11111111-1111-5111-8111-111111111111".into(),
        node_id: "22222222-2222-4222-8222-222222222222".into(),
        object_id: "33333333-3333-5333-8333-333333333333".into(),
        resource_id: "synthetic-iokit-driver".into(),
        direction: "read".into(),
        metric: "storage.device.read_bytes_total".into(),
        collector: "iokit.block".into(),
        scope: "driver".into(),
        attributes: Default::default(),
    });
    state.observe(observation(0, COUNTER));
    let learning = summary(&state, 0);
    for n in 1..=120 {
        state.observe(observation(n * 5, COUNTER + u128::from(n) * 10_000_000));
    }
    let ready = summary(&state, 600);
    assert_eq!(
        ready["baseline"]["reference_rate_bytes_per_second"],
        2_000_000.
    );
    for n in 1..24 {
        assert!(
            state
                .observe(observation(
                    600 + n * 5,
                    COUNTER + 1_200_000_000 + u128::from(n) * 40_000_000
                ))
                .is_empty()
        );
    }
    let pending = summary(&state, 715);
    let opened = state
        .observe(observation(720, COUNTER + 2_160_000_000))
        .pop()
        .unwrap();
    let open = summary(&state, 720);
    assert_eq!(opened.status, "open");
    let mut resolved = opened.clone();
    for n in 1..=12 {
        resolved = state
            .observe(observation(
                720 + n * 5,
                COUNTER + 2_160_000_000 + u128::from(n) * 20_000_000,
            ))
            .pop()
            .unwrap();
    }
    assert_eq!(resolved.status, "resolved");
    state
        .rebaseline("planned_workload_change", BASE + 780_000)
        .unwrap();
    println!("{}",serde_json::to_string_pretty(&json!({"synthetic":true,"policy":Policy::default(),"learning_source":learning,
        "ready_source":ready,"pending_source":pending,"open_source":open,"open_finding":opened,"resolved_finding":resolved,
        "rebaseline_source":summary(&state,780)})).unwrap());
}
