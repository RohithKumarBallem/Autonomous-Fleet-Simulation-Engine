//! Telemetry ingest: clean a raw feed, detect anomalous episodes, and profile normal demand.
//!
//! Pipeline: parse (parallel) -> drop malformed -> sort + de-duplicate -> per-vehicle scan
//! for GPS discontinuities and heartbeat gaps -> tile/window aggregation -> rule-based
//! detectors -> connected components become `DetectedEvent`s.
//!
//! Detectors are deliberately simple, transparent rules (not a trained model); each event
//! carries the evidence count behind it so a reviewer can audit why it fired.

use crate::spec::{FaultKind, Hotspot};
use crate::telemetry::{to_xy, Line, GRID, TICK_S};
use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::Path;

pub const TILE: i32 = 16; // detection tile edge, in cells
pub const WINDOW: i32 = 30; // detection window, in ticks (5 minutes)
const TILES: i32 = GRID / TILE;
const HOP_CELLS: f64 = 6.0; // a position jump larger than this between adjacent heartbeats is a discontinuity
const HOTSPOT_RADIUS: f64 = 24.0; // Manhattan radius treated as "near" a hotspot centre
const GAP_TICKS: i32 = 3; // silence longer than this is a heartbeat gap

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct IngestStats {
    pub lines_total: u64,
    pub parsed: u64,
    pub malformed: u64,
    pub duplicates_dropped: u64,
    pub out_of_order: u64,
    pub heartbeats: u64,
    pub requests: u64,
    pub vehicles: u32,
    pub ticks: i32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Profile {
    pub grid: i32,
    pub vehicles: u32,
    pub ticks: i32,
    pub base_rate: f64,
    pub hotspot_share: f64,
    pub hotspots: Vec<Hotspot>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DetectedEvent {
    pub id: String,
    pub kind: FaultKind,
    pub start_tick: i32,
    pub end_tick: i32,
    pub cx: i32,
    pub cy: i32,
    pub radius: i32,
    pub intensity: f64,
    pub evidence: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct IngestOutput {
    pub stats: IngestStats,
    pub profile: Profile,
    pub events: Vec<DetectedEvent>,
}

enum Rec {
    Hb { ts: u32, vid: u32, x: f64, y: f64 },
    Rq { ts: u32, rid: u64, x: f64, y: f64, delay: u32, cancelled: bool },
}

fn parse(line: &[u8]) -> Option<Rec> {
    let l: Line = serde_json::from_slice(line).ok()?;
    if !l.lat.is_finite() || !l.lon.is_finite() {
        return None;
    }
    let (x, y) = to_xy(l.lat, l.lon);
    let lim = GRID as f64 + 2.0;
    if x < -2.0 || y < -2.0 || x > lim || y > lim {
        return None; // outside the service area: treat as a corrupt fix, not an anomaly
    }
    match l.kind.as_str() {
        "heartbeat" => Some(Rec::Hb { ts: l.ts, vid: l.vid?, x, y }),
        "request" => Some(Rec::Rq {
            ts: l.ts,
            rid: l.rid?,
            x,
            y,
            delay: l.assign_delay.unwrap_or(0),
            cancelled: l.cancelled.unwrap_or(false),
        }),
        _ => None,
    }
}

#[derive(Default, Clone)]
struct Cell {
    req_n: u32,
    req_cancel: u32,
    delay_sum: u64,
    req_sx: f64,
    req_sy: f64,
    tele: u32,
    tele_sx: f64,
    tele_sy: f64,
    tele_vids: Vec<u32>,
    tele_min: i32,
    tele_max: i32,
    gaps: u32,
    gap_sx: f64,
    gap_sy: f64,
    gap_vids: Vec<u32>,
    gap_min: i32,
    gap_max: i32,
}

fn cell_index(n_win: usize, win: usize, tx: i32, ty: i32) -> usize {
    debug_assert!(win < n_win);
    (win * (TILES * TILES) as usize) + (ty * TILES + tx) as usize
}

fn tile_of(v: f64) -> i32 {
    ((v / TILE as f64).floor() as i32).clamp(0, TILES - 1)
}

fn median(v: &mut Vec<u32>) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable();
    v[v.len() / 2] as f64
}

pub fn ingest(path: &Path) -> Result<IngestOutput> {
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let lines: Vec<&[u8]> = data.split(|b| *b == b'\n').filter(|l| !l.is_empty()).collect();
    let mut stats = IngestStats { lines_total: lines.len() as u64, ..Default::default() };

    let parsed: Vec<Option<Rec>> = lines.par_iter().map(|l| parse(l)).collect();

    let mut hbs: Vec<(u32, u32, f64, f64)> = Vec::new(); // (vid, ts, x, y)
    let mut rqs: Vec<(u64, u32, f64, f64, u32, bool)> = Vec::new();
    // "Out of order" means late by at least one whole tick; sub-tick timestamp jitter is normal.
    let mut newest_tick = 0u32;
    for rec in parsed {
        let ts = match &rec {
            None => {
                stats.malformed += 1;
                continue;
            }
            Some(Rec::Hb { ts, .. }) | Some(Rec::Rq { ts, .. }) => *ts,
        };
        let tick = ts / TICK_S;
        if tick < newest_tick {
            stats.out_of_order += 1;
        }
        newest_tick = newest_tick.max(tick);
        match rec {
            Some(Rec::Hb { ts, vid, x, y }) => hbs.push((vid, ts, x, y)),
            Some(Rec::Rq { ts, rid, x, y, delay, cancelled }) => rqs.push((rid, ts, x, y, delay, cancelled)),
            None => unreachable!(),
        }
    }
    stats.parsed = (hbs.len() + rqs.len()) as u64;

    hbs.par_sort_unstable_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));
    let before = hbs.len();
    hbs.dedup_by(|a, b| a.0 == b.0 && a.1 == b.1);
    rqs.par_sort_unstable_by_key(|r| r.0);
    let before_r = rqs.len();
    rqs.dedup_by_key(|r| r.0);
    stats.duplicates_dropped = ((before - hbs.len()) + (before_r - rqs.len())) as u64;
    stats.heartbeats = hbs.len() as u64;
    stats.requests = rqs.len() as u64;

    let max_tick = hbs
        .iter()
        .map(|h| (h.1 / TICK_S) as i32)
        .chain(rqs.iter().map(|r| (r.1 / TICK_S) as i32))
        .max()
        .unwrap_or(0);
    let ticks = max_tick + 1;
    stats.ticks = ticks;
    let n_win = (ticks / WINDOW + 1) as usize;
    let mut cells = vec![Cell { tele_min: i32::MAX, gap_min: i32::MAX, tele_max: -1, gap_max: -1, ..Default::default() }; n_win * (TILES * TILES) as usize];
    let mut vehicles = 0u32;

    // ---- per-vehicle scan (sorted by vehicle then time) ----
    let mut prev: Option<(u32, i32, f64, f64)> = None;
    for &(vid, ts, x, y) in &hbs {
        let tick = (ts / TICK_S) as i32;
        match prev {
            Some((pv, ptick, px, py)) if pv == vid => {
                let dt = tick - ptick;
                if dt > GAP_TICKS {
                    let (tx, ty) = (tile_of(px), tile_of(py));
                    let c = &mut cells[cell_index(n_win, (ptick / WINDOW) as usize, tx, ty)];
                    c.gaps += 1;
                    c.gap_sx += px;
                    c.gap_sy += py;
                    c.gap_vids.push(vid);
                    c.gap_min = c.gap_min.min(ptick);
                    c.gap_max = c.gap_max.max(tick);
                } else if (1..=2).contains(&dt) && ((x - px).powi(2) + (y - py).powi(2)).sqrt() > HOP_CELLS {
                    // Evidence goes to both endpoints: only one of them is the true position, but
                    // the true side accumulates hops from every affected vehicle while the spoofed
                    // side scatters across the map and never reaches the threshold.
                    for (ex, ey, et) in [(px, py, ptick), (x, y, tick)] {
                        let c = &mut cells[cell_index(n_win, (et / WINDOW) as usize, tile_of(ex), tile_of(ey))];
                        c.tele += 1;
                        c.tele_sx += ex;
                        c.tele_sy += ey;
                        c.tele_vids.push(vid);
                        c.tele_min = c.tele_min.min(ptick);
                        c.tele_max = c.tele_max.max(tick);
                    }
                }
            }
            _ => vehicles += 1,
        }
        prev = Some((vid, tick, x, y));
    }
    stats.vehicles = vehicles;

    // ---- request aggregation ----
    for &(_, ts, x, y, delay, cancelled) in &rqs {
        let tick = (ts / TICK_S) as i32;
        let c = &mut cells[cell_index(n_win, (tick / WINDOW) as usize, tile_of(x), tile_of(y))];
        c.req_n += 1;
        c.req_cancel += cancelled as u32;
        c.delay_sum += delay as u64;
        c.req_sx += x;
        c.req_sy += y;
    }

    // ---- baselines ----
    let n_tiles = (TILES * TILES) as usize;
    let mut tile_median = vec![0.0f64; n_tiles];
    for t in 0..n_tiles {
        let mut v: Vec<u32> = (0..n_win).map(|w| cells[w * n_tiles + t].req_n).collect();
        tile_median[t] = median(&mut v);
    }
    let density = vehicles as f64 / (GRID as f64 * GRID as f64);

    // ---- flag cells per detector ----
    let flag = |kind: FaultKind, c: &Cell, tile: usize| -> bool {
        let n = c.req_n as f64;
        let cancel_ratio = if c.req_n > 0 { c.req_cancel as f64 / n } else { 0.0 };
        match kind {
            FaultKind::GpsSpoof => c.tele >= 4,
            FaultKind::SensorDropout => c.gaps >= 2,
            FaultKind::DemandSurge => n >= 12.0 && n >= 3.0 * tile_median[tile].max(1.0) && cancel_ratio < 0.5,
            FaultKind::PhantomRequests => n >= 8.0 && cancel_ratio >= 0.6,
            // Baseline stalls average ~0.4 ticks per request, so even two requests with 12 ticks of
            // combined delay are overwhelming evidence; no minimum request count is needed beyond 2.
            FaultKind::DispatchLatency => n >= 2.0 && c.delay_sum >= 12 && c.delay_sum as f64 / n >= 4.0,
        }
    };

    let mut comps: Vec<(FaultKind, Vec<usize>)> = Vec::new();
    for kind in FaultKind::ALL {
        let flagged: Vec<usize> =
            (0..cells.len()).filter(|&i| flag(kind, &cells[i], i % n_tiles)).collect();
        let mut seen: HashMap<usize, bool> = flagged.iter().map(|&i| (i, false)).collect();
        for &seed in &flagged {
            if seen[&seed] {
                continue;
            }
            // Breadth-first over (window, tile) neighbours: +-2 windows, +-1 tile.
            let mut comp = vec![];
            let mut q = VecDeque::from([seed]);
            seen.insert(seed, true);
            while let Some(i) = q.pop_front() {
                comp.push(i);
                let (w, t) = ((i / n_tiles) as i32, (i % n_tiles) as i32);
                let (tx, ty) = (t % TILES, t / TILES);
                for dw in -2..=2 {
                    for dty in -1..=1 {
                        for dtx in -1..=1 {
                            let (nw, nx, ny) = (w + dw, tx + dtx, ty + dty);
                            if nw < 0 || nw >= n_win as i32 || nx < 0 || nx >= TILES || ny < 0 || ny >= TILES {
                                continue;
                            }
                            let j = cell_index(n_win, nw as usize, nx, ny);
                            if let Some(false) = seen.get(&j).copied() {
                                seen.insert(j, true);
                                q.push_back(j);
                            }
                        }
                    }
                }
            }
            comps.push((kind, comp));
        }
    }

    // Baseline demand rate: mean over windows untouched by a demand-inflating episode (surge or
    // phantom flood). A plain median is not robust here because such episodes can cover most windows.
    let mut tainted = vec![false; n_win];
    for (kind, comp) in &comps {
        if matches!(kind, FaultKind::DemandSurge | FaultKind::PhantomRequests) {
            for &i in comp {
                for w in (i / n_tiles).saturating_sub(1)..=(i / n_tiles + 1).min(n_win - 1) {
                    tainted[w] = true;
                }
            }
        }
    }
    // The final window is usually partial; exclude it too.
    let clean: Vec<f64> = (0..n_win.saturating_sub(1))
        .filter(|&w| !tainted[w])
        .map(|w| (0..n_tiles).map(|t| cells[w * n_tiles + t].req_n as f64).sum::<f64>())
        .collect();
    let base_rate = if clean.is_empty() {
        0.0
    } else {
        (clean.iter().sum::<f64>() / clean.len() as f64 / WINDOW as f64 * 1000.0).round() / 1000.0
    };

    let mut events: Vec<DetectedEvent> = Vec::new();
    for (kind, comp) in &comps {
        if let Some(ev) = summarise(*kind, comp, &cells, n_tiles, n_win, &tile_median, base_rate, density) {
            events.push(ev);
        }
    }
    events.sort_by_key(|e| (e.start_tick, e.kind));
    for (i, e) in events.iter_mut().enumerate() {
        e.id = format!("ev-{i:03}");
    }

    // ---- demand profile ----
    let profile = build_profile(&cells, n_tiles, &tile_median, &rqs, base_rate, vehicles, ticks);
    Ok(IngestOutput { stats, profile, events })
}

#[allow(clippy::too_many_arguments)]
fn summarise(
    kind: FaultKind,
    comp: &[usize],
    cells: &[Cell],
    n_tiles: usize,
    n_win: usize,
    tile_median: &[f64],
    base_rate: f64,
    density: f64,
) -> Option<DetectedEvent> {
    let _ = n_win;
    let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
    let (mut evidence, mut vids): (u64, Vec<u32>) = (0, vec![]);
    let (mut wmin, mut wmax) = (usize::MAX, 0usize);
    let (mut tmin, mut tmax) = (i32::MAX, -1i32);
    let (mut excess, mut cancels, mut reqs, mut delays) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for &i in comp {
        let c = &cells[i];
        let w = i / n_tiles;
        wmin = wmin.min(w);
        wmax = wmax.max(w);
        match kind {
            FaultKind::GpsSpoof => {
                sx += c.tele_sx;
                sy += c.tele_sy;
                n += c.tele as f64;
                evidence += c.tele as u64;
                vids.extend_from_slice(&c.tele_vids);
                if c.tele > 0 {
                    tmin = tmin.min(c.tele_min);
                    tmax = tmax.max(c.tele_max);
                }
            }
            FaultKind::SensorDropout => {
                sx += c.gap_sx;
                sy += c.gap_sy;
                n += c.gaps as f64;
                evidence += c.gaps as u64;
                vids.extend_from_slice(&c.gap_vids);
                if c.gaps > 0 {
                    tmin = tmin.min(c.gap_min);
                    tmax = tmax.max(c.gap_max);
                }
            }
            _ => {
                sx += c.req_sx;
                sy += c.req_sy;
                n += c.req_n as f64;
                evidence += c.req_n as u64;
                excess += c.req_n as f64 - tile_median[i % n_tiles];
                cancels += c.req_cancel as f64;
                reqs += c.req_n as f64;
                delays += c.delay_sum as f64;
            }
        }
    }
    if n == 0.0 {
        return None;
    }
    let (cx, cy) = (sx / n, sy / n);
    let mut extent = 0.0f64;
    for &i in comp {
        let t = (i % n_tiles) as i32;
        let (tcx, tcy) = ((t % TILES) as f64 * TILE as f64 + TILE as f64 / 2.0, (t / TILES) as f64 * TILE as f64 + TILE as f64 / 2.0);
        extent = extent.max((tcx - cx).abs() + (tcy - cy).abs());
    }
    let radius = (8.0 + 0.5 * extent).round().clamp(8.0, 20.0) as i32;

    let (start, end) = match kind {
        FaultKind::GpsSpoof | FaultKind::SensorDropout if tmax >= 0 => (tmin, tmax),
        _ => (wmin as i32 * WINDOW, (wmax as i32 + 1) * WINDOW),
    };
    let duration = (end - start).max(1) as f64;
    let intensity = match kind {
        FaultKind::GpsSpoof | FaultKind::SensorDropout => {
            vids.sort_unstable();
            vids.dedup();
            let present = (density * 2.0 * (radius * radius) as f64).max(1.0);
            (vids.len() as f64 / present).clamp(0.3, 0.95)
        }
        FaultKind::DemandSurge => (excess / duration / base_rate.max(0.1)).clamp(1.0, 12.0),
        FaultKind::PhantomRequests => {
            ((cancels - 0.03 * reqs) / duration / base_rate.max(0.1)).clamp(0.5, 8.0)
        }
        FaultKind::DispatchLatency => {
            let d = delays / reqs.max(1.0);
            (d / (1.0 + d)).clamp(0.5, 0.97)
        }
    };
    Some(DetectedEvent {
        id: String::new(),
        kind,
        start_tick: start,
        end_tick: end,
        cx: (cx.round() as i32).clamp(0, GRID - 1),
        cy: (cy.round() as i32).clamp(0, GRID - 1),
        radius,
        intensity: (intensity * 1000.0).round() / 1000.0,
        evidence,
    })
}

fn build_profile(
    cells: &[Cell],
    n_tiles: usize,
    tile_median: &[f64],
    rqs: &[(u64, u32, f64, f64, u32, bool)],
    base_rate: f64,
    vehicles: u32,
    ticks: i32,
) -> Profile {
    // Hotspots: highest-baseline tiles, suppressing tiles within two tile-steps of an earlier pick.
    let mut order: Vec<usize> = (0..n_tiles).collect();
    order.sort_by(|&a, &b| tile_median[b].partial_cmp(&tile_median[a]).unwrap().then(a.cmp(&b)));
    let mut picks: Vec<(i32, i32)> = vec![];
    for &t in &order {
        let (tx, ty) = (t as i32 % TILES, t as i32 / TILES);
        if picks.iter().all(|&(px, py)| (px - tx).abs() + (py - ty).abs() > 2) {
            picks.push((tx, ty));
        }
        if picks.len() == 3 {
            break;
        }
    }
    let n_win = cells.len() / n_tiles;
    let mut hotspots = vec![];
    for &(tx, ty) in &picks {
        let (mut sx, mut sy, mut n) = (0.0, 0.0, 0.0);
        for w in 0..n_win {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let (nx, ny) = (tx + dx, ty + dy);
                    if nx < 0 || ny < 0 || nx >= TILES || ny >= TILES {
                        continue;
                    }
                    let c = &cells[w * n_tiles + (ny * TILES + nx) as usize];
                    sx += c.req_sx;
                    sy += c.req_sy;
                    n += c.req_n as f64;
                }
            }
        }
        if n > 0.0 {
            hotspots.push(Hotspot { x: (sx / n).round() as i32, y: (sy / n).round() as i32, weight: n });
        }
    }
    let max_w = hotspots.iter().map(|h| h.weight).fold(0.0, f64::max).max(1.0);
    for h in &mut hotspots {
        h.weight = (h.weight / max_w * 1000.0).round() / 1000.0;
    }
    let near = rqs
        .iter()
        .filter(|r| hotspots.iter().any(|h| ((r.2 - h.x as f64).abs() + (r.3 - h.y as f64).abs()) <= HOTSPOT_RADIUS))
        .count() as f64;
    let frac = near / rqs.len().max(1) as f64;
    let uniform = (hotspots.len() as f64 * 2.0 * HOTSPOT_RADIUS * HOTSPOT_RADIUS / (GRID as f64 * GRID as f64)).min(0.9);
    let share = ((frac - uniform) / (1.0 - uniform)).clamp(0.3, 0.9);
    Profile {
        grid: GRID,
        vehicles,
        ticks,
        base_rate,
        hotspot_share: (share * 1000.0).round() / 1000.0,
        hotspots,
    }
}

// ---- scoring against generator ground truth ----

#[derive(Serialize, Deserialize, Debug, Default)]
pub struct KindScore {
    pub truth: usize,
    pub detected: usize,
    pub matched: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct DetectionScore {
    pub truth_events: usize,
    pub detected_events: usize,
    pub matched: usize,
    pub precision: f64,
    pub recall: f64,
    pub mean_start_error_ticks: f64,
    pub mean_center_error_cells: f64,
    pub per_kind: BTreeMap<String, KindScore>,
}

pub fn score(detected: &[DetectedEvent], truth: &[crate::telemetry::TruthEvent]) -> DetectionScore {
    let mut used = vec![false; detected.len()];
    let mut per_kind: BTreeMap<String, KindScore> = BTreeMap::new();
    let (mut matched, mut start_err, mut center_err) = (0usize, 0.0, 0.0);
    for t in truth {
        per_kind.entry(t.kind.name().to_string()).or_default().truth += 1;
    }
    for d in detected {
        per_kind.entry(d.kind.name().to_string()).or_default().detected += 1;
    }
    for t in truth {
        let mut best: Option<(usize, i32)> = None;
        for (i, d) in detected.iter().enumerate() {
            if used[i] || d.kind != t.kind {
                continue;
            }
            let overlap = d.start_tick <= t.end_tick + 15 && d.end_tick >= t.start_tick - 15;
            let dist = (d.cx - t.cx).abs() + (d.cy - t.cy).abs();
            if overlap && dist <= 28 && best.map_or(true, |(_, bd)| dist < bd) {
                best = Some((i, dist));
            }
        }
        if let Some((i, dist)) = best {
            used[i] = true;
            matched += 1;
            start_err += (detected[i].start_tick - t.start_tick).abs() as f64;
            center_err += dist as f64;
            per_kind.entry(t.kind.name().to_string()).or_default().matched += 1;
        }
    }
    DetectionScore {
        truth_events: truth.len(),
        detected_events: detected.len(),
        matched,
        precision: if detected.is_empty() { 0.0 } else { matched as f64 / detected.len() as f64 },
        recall: if truth.is_empty() { 0.0 } else { matched as f64 / truth.len() as f64 },
        mean_start_error_ticks: if matched > 0 { start_err / matched as f64 } else { 0.0 },
        mean_center_error_cells: if matched > 0 { center_err / matched as f64 } else { 0.0 },
        per_kind,
    }
}
