//! Session-relative monotonic clock segments; old samples retain their origin.
use std::{
    sync::Mutex,
    time::{Duration, Instant, SystemTime},
};

pub struct Clock {
    session: String,
    inner: Mutex<Segment>,
}
struct Segment {
    index: u64,
    origin: Instant,
    last_instant: Instant,
    last_wall: SystemTime,
}
#[derive(Clone)]
pub struct SampleTime {
    pub clock_id: String,
    pub started_at: String,
    pub started_monotonic_ns: u128,
    origin: Instant,
}
impl Clock {
    pub fn new(session: &str) -> Self {
        let origin = Instant::now();
        Self {
            session: session.into(),
            inner: Mutex::new(Segment {
                index: 0,
                origin,
                last_instant: origin,
                last_wall: SystemTime::now(),
            }),
        }
    }
    pub fn start(&self) -> SampleTime {
        let mut s = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let instant = Instant::now();
        let wall = SystemTime::now();
        let monotonic = instant.duration_since(s.last_instant);
        let discontinuous = wall
            .duration_since(s.last_wall)
            .map(|elapsed| elapsed.abs_diff(monotonic) > Duration::from_secs(2))
            .unwrap_or(true);
        if discontinuous {
            s.index = s.index.saturating_add(1);
            s.origin = instant;
        }
        s.last_instant = instant;
        s.last_wall = wall;
        SampleTime {
            clock_id: format!("{}/monotonic/{}", self.session, s.index),
            started_at: crate::model::now(),
            started_monotonic_ns: instant.duration_since(s.origin).as_nanos(),
            origin: s.origin,
        }
    }
    pub fn read(&self) -> (String, u128) {
        let stamp = self.start();
        (stamp.clock_id, stamp.started_monotonic_ns)
    }
}
impl SampleTime {
    pub fn finished_ns(&self) -> u128 {
        self.origin.elapsed().as_nanos()
    }
}
