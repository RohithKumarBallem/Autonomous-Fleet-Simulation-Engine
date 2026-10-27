//! Telemetry wire format and a synthetic "field fleet" generator.
//!
//! There is no real vehicle data in this repository. The generator stands in for a
//! production telemetry feed: it emits per-vehicle heartbeats and ride-request events
//! with the defects real feeds have (duplicates, late/out-of-order delivery, corrupt
//! lines, random heartbeat loss) and injects *known* anomalies. Because the injected
//! anomalies are written to a ground-truth file, the ingest stage can be scored on
//! precision/recall instead of being taken on faith.

use crate::rng::{hash01, Rng};
use crate::spec::FaultKind;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub const GRID: i32 = 128;
pub const TICK_S: u32 = 10;
const LAT0: f64 = 37.70;
const LON0: f64 = -122.50;
const DLAT: f64 = 0.0009; // one 100 m cell
const DLON: f64 = 0.00113;

pub fn to_latlon(x: f64, y: f64) -> (f64, f64) {
    (LAT0 + y * DLAT, LON0 + x * DLON)
}

pub fn to_xy(lat: f64, lon: f64) -> (f64, f64) {
    ((lon - LON0) / DLON, (lat - LAT0) / DLAT)
}

/// One NDJSON line. `type` is `heartbeat` or `request`; the other fields depend on it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Line {
    #[serde(rename = "type")]
    pub kind: String,
    pub ts: u32,
    pub lat: f64,
    pub lon: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rid: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assign_delay: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancelled: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TruthEvent {
    pub kind: FaultKind,
    pub start_tick: i32,
    pub end_tick: i32,
    pub cx: i32,
    pub cy: i32,
    pub radius: i32,
    pub intensity: f64,
}

pub struct GenConfig {
    pub vehicles: u32,
    pub hours: f64,
    pub anomalies: usize,
    pub seed: u64,
    pub base_rate: f64,
}

#[derive(Serialize, Debug)]
pub struct GenSummary {
    pub lines_written: u64,
    pub heartbeats: u64,
    pub requests: u64,
    pub duplicates_injected: u64,
    pub corrupt_injected: u64,
    pub late_delivered: u64,
    pub ticks: i32,
    pub truth_events: usize,
}

const HOTSPOTS: [(i32, i32, f64); 3] = [(32, 40, 1.0), (96, 88, 0.8), (64, 64, 0.6)];
const HOTSPOT_SHARE: f64 = 0.6;

fn clamp(v: i32) -> i32 {
    v.clamp(0, GRID - 1)
}

fn sample_demand_point(rng: &mut Rng) -> (i32, i32) {
    if rng.uniform() < HOTSPOT_SHARE {
        let total: f64 = HOTSPOTS.iter().map(|h| h.2).sum();
        let mut pick = rng.uniform() * total;
        let mut h = HOTSPOTS.len() - 1;
        for (i, hs) in HOTSPOTS.iter().enumerate() {
            if pick < hs.2 {
                h = i;
                break;
            }
            pick -= hs.2;
        }
        let spread = GRID as f64 / 8.0;
        let ox = (rng.uniform() + rng.uniform() + rng.uniform() - 1.5) * spread;
        let oy = (rng.uniform() + rng.uniform() + rng.uniform() - 1.5) * spread;
        (clamp(HOTSPOTS[h].0 + ox.round() as i32), clamp(HOTSPOTS[h].1 + oy.round() as i32))
    } else {
        (rng.below(GRID as u32) as i32, rng.below(GRID as u32) as i32)
    }
}

fn in_region(e: &TruthEvent, x: i32, y: i32) -> bool {
    (x - e.cx).abs() + (y - e.cy).abs() <= e.radius
}

/// Schedules one anomaly per equal time slot so events never overlap in time.
/// Centres are drawn from the demand distribution: incidents happen where there is traffic.
fn plan_events(cfg: &GenConfig, ticks: i32, rng: &mut Rng) -> Vec<TruthEvent> {
    if cfg.anomalies == 0 {
        return vec![];
    }
    let slot = ticks / cfg.anomalies as i32;
    let mut events = Vec::new();
    for i in 0..cfg.anomalies {
        let kind = FaultKind::ALL[(rng.below(5)) as usize];
        let duration = rng.range(24, (slot - 40).clamp(24, 66));
        let start = i as i32 * slot + rng.range(20, (slot - duration - 10).max(21));
        let (cx, cy) = sample_demand_point(rng);
        let radius = rng.range(8, 14);
        let intensity = match kind {
            FaultKind::GpsSpoof | FaultKind::SensorDropout => 0.6 + 0.35 * rng.uniform(),
            FaultKind::DemandSurge => 3.0 + 4.0 * rng.uniform(),
            FaultKind::DispatchLatency => 0.88 + 0.09 * rng.uniform(),
            FaultKind::PhantomRequests => 1.5 + 2.0 * rng.uniform(),
        };
        events.push(TruthEvent {
            kind,
            start_tick: start,
            end_tick: start + duration,
            cx,
            cy,
            radius,
            intensity: (intensity * 1000.0).round() / 1000.0,
        });
    }
    events
}

struct Veh {
    x: i32,
    y: i32,
    tx: i32,
    ty: i32,
    pause: u32,
}

fn emit(w: &mut BufWriter<File>, s: &str, summary: &mut GenSummary) -> Result<()> {
    w.write_all(s.as_bytes())?;
    w.write_all(b"\n")?;
    summary.lines_written += 1;
    Ok(())
}

pub fn generate(cfg: &GenConfig, out: &Path, truth_out: &Path) -> Result<GenSummary> {
    let ticks = (cfg.hours * 3600.0 / TICK_S as f64) as i32;
    let mut rng = Rng::new(cfg.seed);
    let events = plan_events(cfg, ticks, &mut rng);
    let mut w = BufWriter::with_capacity(1 << 20, File::create(out)?);

    let mut fleet: Vec<Veh> = (0..cfg.vehicles)
        .map(|_| {
            let (x, y) = (rng.below(GRID as u32) as i32, rng.below(GRID as u32) as i32);
            Veh { x, y, tx: x, ty: y, pause: rng.below(10) }
        })
        .collect();

    let mut summary = GenSummary {
        lines_written: 0,
        heartbeats: 0,
        requests: 0,
        duplicates_injected: 0,
        corrupt_injected: 0,
        late_delivered: 0,
        ticks,
        truth_events: events.len(),
    };
    let mut rid: u64 = 0;
    let mut late: Vec<(i32, String)> = Vec::new();


    for t in 0..ticks {
        let active: Vec<(usize, &TruthEvent)> =
            events.iter().enumerate().filter(|(_, e)| t >= e.start_tick && t < e.end_tick).collect();
        let mut batch: Vec<String> = Vec::with_capacity(cfg.vehicles as usize + 16);

        // --- vehicles ---
        for (vid, v) in fleet.iter_mut().enumerate() {
            let vid = vid as u32;
            let dropped = active.iter().any(|(i, e)| {
                e.kind == FaultKind::SensorDropout
                    && in_region(e, v.x, v.y)
                    && hash01(cfg.seed, 2000 + *i as u64, vid as u64) < e.intensity
            });
            if !dropped {
                if v.pause > 0 {
                    v.pause -= 1;
                } else if v.x != v.tx {
                    v.x += (v.tx - v.x).signum();
                } else if v.y != v.ty {
                    v.y += (v.ty - v.y).signum();
                } else {
                    v.pause = rng.below(15);
                    let (nx, ny) = if rng.uniform() < 0.5 {
                        sample_demand_point(&mut rng)
                    } else {
                        (clamp(v.x + rng.range(-30, 30)), clamp(v.y + rng.range(-30, 30)))
                    };
                    v.tx = nx;
                    v.ty = ny;
                }
            }
            if dropped || rng.uniform() < 0.003 {
                continue; // no heartbeat: silent sensor or ordinary packet loss
            }
            let (mut fx, mut fy) = (v.x as f64, v.y as f64);
            for (i, e) in &active {
                if e.kind == FaultKind::GpsSpoof
                    && in_region(e, v.x, v.y)
                    && hash01(cfg.seed, 1000 + *i as u64, vid as u64) < e.intensity
                {
                    // Constant per-vehicle bias for the whole event, like a spoofed GNSS fix.
                    let mag = 8.0 + 24.0 * e.intensity;
                    fx += (hash01(cfg.seed, 5000 + *i as u64, vid as u64) * 2.0 - 1.0) * mag;
                    fy += (hash01(cfg.seed, 6000 + *i as u64, vid as u64) * 2.0 - 1.0) * mag;
                    break;
                }
            }
            fx += rng.gaussian() * 0.05;
            fy += rng.gaussian() * 0.05;
            let (lat, lon) = to_latlon(fx, fy);
            let line = Line {
                kind: "heartbeat".into(),
                ts: t as u32 * TICK_S + rng.below(3),
                lat: (lat * 1e6).round() / 1e6,
                lon: (lon * 1e6).round() / 1e6,
                vid: Some(vid),
                rid: None,
                assign_delay: None,
                cancelled: None,
            };
            summary.heartbeats += 1;
            batch.push(serde_json::to_string(&line)?);
        }

        // --- ride requests ---
        let diurnal = 1.0 + 0.35 * (2.0 * std::f64::consts::PI * t as f64 / ticks as f64).sin();
        let mut reqs: Vec<(i32, i32, bool, Option<usize>)> = Vec::new(); // x, y, cancelled, event
        for _ in 0..rng.poisson(cfg.base_rate * diurnal) {
            let (x, y) = sample_demand_point(&mut rng);
            reqs.push((x, y, rng.uniform() < 0.03, None));
        }
        for (i, e) in &active {
            if e.kind == FaultKind::DemandSurge || e.kind == FaultKind::PhantomRequests {
                for _ in 0..rng.poisson(cfg.base_rate * e.intensity) {
                    let dx = rng.range(-e.radius, e.radius);
                    let span = e.radius - dx.abs();
                    let dy = rng.range(-span, span);
                    let phantom = e.kind == FaultKind::PhantomRequests;
                    reqs.push((clamp(e.cx + dx), clamp(e.cy + dy), phantom || rng.uniform() < 0.05, Some(*i)));
                }
            }
        }
        for (x, y, cancelled, _) in reqs {
            let stall = active
                .iter()
                .find(|(_, e)| e.kind == FaultKind::DispatchLatency && in_region(e, x, y))
                .map(|(_, e)| e.intensity)
                .unwrap_or(0.3);
            let (lat, lon) = to_latlon(x as f64, y as f64);
            rid += 1;
            let line = Line {
                kind: "request".into(),
                ts: t as u32 * TICK_S + rng.below(TICK_S),
                lat: (lat * 1e6).round() / 1e6,
                lon: (lon * 1e6).round() / 1e6,
                vid: None,
                rid: Some(rid),
                assign_delay: Some(rng.geometric_stalls(stall)),
                cancelled: Some(cancelled),
            };
            summary.requests += 1;
            batch.push(serde_json::to_string(&line)?);
        }

        // --- delivery defects ---
        let (due, rest): (Vec<_>, Vec<_>) = late.drain(..).partition(|(release, _)| *release <= t);
        late = rest;
        for (_, s) in due {
            emit(&mut w, &s, &mut summary)?;
            summary.late_delivered += 1;
        }
        for s in batch {
            let r = rng.uniform();
            if r < 0.02 {
                late.push((t + 1 + rng.below(3) as i32, s)); // delivered 1-3 ticks late
            } else if r < 0.03 {
                emit(&mut w, &s, &mut summary)?;
                emit(&mut w, &s, &mut summary)?; // duplicate
                summary.duplicates_injected += 1;
            } else if r < 0.031 {
                let cut = (s.len() / 2).max(1);
                emit(&mut w, &s[..cut], &mut summary)?; // truncated line
                summary.corrupt_injected += 1;
            } else {
                emit(&mut w, &s, &mut summary)?;
            }
        }
    }
    for (_, s) in late.drain(..) {
        emit(&mut w, &s, &mut summary)?;
        summary.late_delivered += 1;
    }
    w.flush()?;

    serde_json::to_writer_pretty(File::create(truth_out)?, &events)?;
    Ok(summary)
}
