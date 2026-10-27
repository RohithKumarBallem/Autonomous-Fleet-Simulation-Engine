//! Turns detected telemetry episodes into a reproducible scenario suite.
//!
//! Everything here is a pure function of the ingest output and the suite config: seeds and
//! start offsets come from SHA-256 of stable identifiers, never from a clock or an RNG, so
//! re-running the pipeline over the same telemetry yields byte-identical scenario ids.

use crate::ingest::{DetectedEvent, IngestOutput};
use crate::spec::{FaultSpec, ScenarioSpec};
use sha2::{Digest, Sha256};

pub struct SuiteConfig {
    pub seeds_per_event: usize,
    pub compound: usize,
    pub controls: usize,
    pub vehicles: i32,
    pub ticks: i32,
    pub max_wait_ticks: i32,
}

impl Default for SuiteConfig {
    fn default() -> Self {
        SuiteConfig { seeds_per_event: 20, compound: 24, controls: 20, vehicles: 500, ticks: 1440, max_wait_ticks: 90 }
    }
}

fn digest_u64(parts: &[&str]) -> u64 {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p.as_bytes());
        h.update([0u8]);
    }
    let out = h.finalize();
    u64::from_be_bytes(out[..8].try_into().unwrap())
}

fn fault_from(e: &DetectedEvent, lead_in: i32, offset: i32) -> FaultSpec {
    FaultSpec {
        kind: e.kind,
        start_tick: lead_in + offset,
        duration: (e.end_tick - e.start_tick).clamp(20, 400),
        cx: e.cx,
        cy: e.cy,
        radius: e.radius,
        intensity: e.intensity,
    }
}

pub fn build_suite(ingest: &IngestOutput, cfg: &SuiteConfig) -> Vec<ScenarioSpec> {
    let p = &ingest.profile;
    let template = |label: &str, source: Vec<String>, seed: u64, faults: Vec<FaultSpec>| {
        ScenarioSpec {
            id: String::new(),
            label: label.to_string(),
            source,
            seed,
            grid: p.grid,
            vehicles: cfg.vehicles,
            ticks: cfg.ticks,
            max_wait_ticks: cfg.max_wait_ticks,
            base_rate: p.base_rate,
            hotspot_share: p.hotspot_share,
            hotspots: p.hotspots.clone(),
            faults,
        }
        .finalize()
    };
    // Spread fault onsets across the horizon (after a warm-up) so the suite covers different
    // demand phases; the offset is a hash of the event id, hence reproducible.
    let lead_in = 240;
    let offset = |e: &DetectedEvent| (digest_u64(&["offset", &e.id]) % 600) as i32;

    let mut suite = Vec::new();
    for k in 0..cfg.controls {
        suite.push(template("control", vec![], digest_u64(&["control", &k.to_string()]), vec![]));
    }
    for e in &ingest.events {
        for k in 0..cfg.seeds_per_event {
            let seed = digest_u64(&["single", &e.id, &k.to_string()]);
            suite.push(template("single", vec![e.id.clone()], seed, vec![fault_from(e, lead_in, offset(e))]));
        }
    }
    let n = ingest.events.len();
    if n >= 2 {
        for pair in 0..cfg.compound {
            let i = pair % n;
            let mut j = (pair * 7 + 3) % n;
            if j == i {
                j = (i + 1) % n;
            }
            let (a, b) = (&ingest.events[i], &ingest.events[j]);
            let first = fault_from(a, lead_in, offset(a));
            let mut second = fault_from(b, lead_in, offset(a) + 30); // overlaps the first
            second.start_tick = first.start_tick + 30;
            let seed = digest_u64(&["compound", &a.id, &b.id]);
            suite.push(template("compound", vec![a.id.clone(), b.id.clone()], seed, vec![first, second]));
        }
    }
    suite.sort_by(|a, b| a.id.cmp(&b.id));
    suite.dedup_by(|a, b| a.id == b.id);
    suite
}
