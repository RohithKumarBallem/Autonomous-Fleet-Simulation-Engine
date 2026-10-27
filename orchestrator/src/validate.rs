//! Pass/fail verdicts: does the platform meet its service and safety envelope under a scenario?

use crate::ffi::MetricsC;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Sla {
    /// 95th percentile rider wait (ticks, 1 tick = 10 s) must stay at or below this.
    pub max_p95_wait_ticks: i32,
    /// Abandoned requests as a fraction of all requests.
    pub max_abandon_rate: f64,
    /// Road-congestion events (cells over capacity while vehicles are moving).
    pub max_congestion_events: i64,
}

impl Default for Sla {
    fn default() -> Self {
        Sla { max_p95_wait_ticks: 60, max_abandon_rate: 0.01, max_congestion_events: 200 }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Verdict {
    pub pass: bool,
    pub violations: Vec<String>,
}

pub fn evaluate(m: &MetricsC, sla: &Sla) -> Verdict {
    let mut violations = Vec::new();
    if m.invariant_violations != 0 {
        violations.push(format!("invariant_violations={}", m.invariant_violations));
    }
    if m.wait_p95_ticks > sla.max_p95_wait_ticks {
        violations.push(format!("p95_wait={}>{}", m.wait_p95_ticks, sla.max_p95_wait_ticks));
    }
    let abandon_rate = m.abandoned as f64 / m.requests_total.max(1) as f64;
    if abandon_rate > sla.max_abandon_rate {
        violations.push(format!("abandon_rate={:.3}>{:.3}", abandon_rate, sla.max_abandon_rate));
    }
    if m.congestion_events > sla.max_congestion_events {
        violations.push(format!("congestion={}>{}", m.congestion_events, sla.max_congestion_events));
    }
    Verdict { pass: violations.is_empty(), violations }
}
