//! Scenario specification: the unit of reproducibility.
//!
//! A scenario is a plain JSON document. Its `id` is the SHA-256 of the canonical
//! JSON of every other field, so identical inputs always get identical ids and a
//! result can always be traced back to exactly what was simulated.

use crate::ffi::{FaultC, ScenarioC, MAX_FAULTS, MAX_HOTSPOTS};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FaultKind {
    GpsSpoof,
    SensorDropout,
    DemandSurge,
    DispatchLatency,
    PhantomRequests,
}

impl FaultKind {
    pub const ALL: [FaultKind; 5] = [
        FaultKind::GpsSpoof,
        FaultKind::SensorDropout,
        FaultKind::DemandSurge,
        FaultKind::DispatchLatency,
        FaultKind::PhantomRequests,
    ];

    pub fn code(self) -> i32 {
        self as i32
    }

    pub fn name(self) -> &'static str {
        match self {
            FaultKind::GpsSpoof => "gps_spoof",
            FaultKind::SensorDropout => "sensor_dropout",
            FaultKind::DemandSurge => "demand_surge",
            FaultKind::DispatchLatency => "dispatch_latency",
            FaultKind::PhantomRequests => "phantom_requests",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FaultSpec {
    pub kind: FaultKind,
    pub start_tick: i32,
    pub duration: i32,
    pub cx: i32,
    pub cy: i32,
    pub radius: i32,
    pub intensity: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hotspot {
    pub x: i32,
    pub y: i32,
    pub weight: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ScenarioSpec {
    #[serde(default)]
    pub id: String,
    /// `control`, `single` or `compound`.
    pub label: String,
    /// Ids of the detected telemetry events this scenario was derived from.
    pub source: Vec<String>,
    pub seed: u64,
    pub grid: i32,
    pub vehicles: i32,
    pub ticks: i32,
    pub max_wait_ticks: i32,
    pub base_rate: f64,
    pub hotspot_share: f64,
    pub hotspots: Vec<Hotspot>,
    pub faults: Vec<FaultSpec>,
}

impl ScenarioSpec {
    /// Content hash of everything except the id itself (first 16 hex chars).
    pub fn content_id(&self) -> String {
        let mut clone = self.clone();
        clone.id = String::new();
        let json = serde_json::to_vec(&clone).expect("scenario serialises");
        hex(&Sha256::digest(&json))[..16].to_string()
    }

    pub fn finalize(mut self) -> Self {
        self.id = self.content_id();
        self
    }

    pub fn to_c(&self) -> Result<ScenarioC> {
        if self.faults.len() > MAX_FAULTS || self.hotspots.len() > MAX_HOTSPOTS {
            bail!("scenario {} exceeds fault/hotspot capacity", self.id);
        }
        let mut c = ScenarioC {
            seed: self.seed,
            grid: self.grid,
            vehicles: self.vehicles,
            ticks: self.ticks,
            max_wait_ticks: self.max_wait_ticks,
            base_rate: self.base_rate,
            hotspot_share: self.hotspot_share,
            n_hotspots: self.hotspots.len() as i32,
            n_faults: self.faults.len() as i32,
            ..Default::default()
        };
        for (i, h) in self.hotspots.iter().enumerate() {
            c.hotspot_x[i] = h.x;
            c.hotspot_y[i] = h.y;
            c.hotspot_w[i] = h.weight;
        }
        for (i, f) in self.faults.iter().enumerate() {
            c.faults[i] = FaultC {
                kind: f.kind.code(),
                start_tick: f.start_tick,
                duration: f.duration,
                cx: f.cx,
                cy: f.cy,
                radius: f.radius,
                intensity: f.intensity,
            };
        }
        Ok(c)
    }
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(b >> 4) as usize] as char);
        s.push(DIGITS[(b & 15) as usize] as char);
    }
    s
}

pub fn read_ndjson<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Result<Vec<T>> {
    use anyhow::Context;
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .enumerate()
        .map(|(i, l)| serde_json::from_str(l).with_context(|| format!("{}:{}", path.display(), i + 1)))
        .collect()
}
