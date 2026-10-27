//! Parallel, shardable scenario execution on top of the C++ kernel.

use crate::ffi::{self, MetricsC};
use crate::spec::{hex, ScenarioSpec};
use crate::validate::{self, Sla};
use anyhow::{bail, Context, Result};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::time::Instant;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MetricsOut {
    pub state_hash: String,
    pub vehicle_ticks: u64,
    pub requests_total: i64,
    pub completed: i64,
    pub abandoned: i64,
    pub cancelled_phantom: i64,
    pub pending_end: i64,
    pub in_progress_end: i64,
    pub wait_mean_ticks: f64,
    pub wait_p95_ticks: i32,
    pub trip_mean_ticks: f64,
    pub utilization: f64,
    pub congestion_events: i64,
    pub max_occupancy: i32,
    pub fault_vehicle_ticks: i64,
    pub deadhead_cells: i64,
    pub loaded_cells: i64,
    pub invariant_violations: i64,
}

impl From<&MetricsC> for MetricsOut {
    fn from(m: &MetricsC) -> Self {
        MetricsOut {
            state_hash: hex(&m.state_hash.to_be_bytes()),
            vehicle_ticks: m.vehicle_ticks,
            requests_total: m.requests_total,
            completed: m.completed,
            abandoned: m.abandoned,
            cancelled_phantom: m.cancelled_phantom,
            pending_end: m.pending_end,
            in_progress_end: m.in_progress_end,
            wait_mean_ticks: m.wait_mean_ticks,
            wait_p95_ticks: m.wait_p95_ticks,
            trip_mean_ticks: m.trip_mean_ticks,
            utilization: m.utilization,
            congestion_events: m.congestion_events,
            max_occupancy: m.max_occupancy,
            fault_vehicle_ticks: m.fault_vehicle_ticks,
            deadhead_cells: m.deadhead_cells,
            loaded_cells: m.loaded_cells,
            invariant_violations: m.invariant_violations,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ScenarioResult {
    pub id: String,
    pub label: String,
    pub faults: Vec<String>,
    pub metrics: MetricsOut,
    pub pass: bool,
    pub violations: Vec<String>,
    pub elapsed_us: u64,
}

/// `index/count`: this worker owns scenarios whose position in id order is `index mod count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shard {
    pub index: usize,
    pub count: usize,
}

impl Shard {
    pub fn parse(s: &str) -> Result<Shard> {
        let (i, n) = s.split_once('/').with_context(|| format!("shard must look like 0/4, got {s}"))?;
        let (index, count): (usize, usize) = (i.parse()?, n.parse()?);
        if count == 0 || index >= count {
            bail!("invalid shard {s}");
        }
        Ok(Shard { index, count })
    }

    pub fn whole() -> Shard {
        Shard { index: 0, count: 1 }
    }
}

/// Ordering by id makes the partition independent of how the suite file was written.
pub fn select_shard(mut specs: Vec<ScenarioSpec>, shard: Shard) -> Vec<ScenarioSpec> {
    specs.sort_by(|a, b| a.id.cmp(&b.id));
    specs.into_iter().enumerate().filter(|(i, _)| i % shard.count == shard.index).map(|(_, s)| s).collect()
}

pub fn run_one(spec: &ScenarioSpec, sla: &Sla) -> Result<ScenarioResult> {
    let c = spec.to_c()?;
    let t0 = Instant::now();
    let m = ffi::run(&c).with_context(|| format!("scenario {}", spec.id))?;
    let elapsed_us = t0.elapsed().as_micros() as u64;
    let verdict = validate::evaluate(&m, sla);
    Ok(ScenarioResult {
        id: spec.id.clone(),
        label: spec.label.clone(),
        faults: spec.faults.iter().map(|f| f.kind.name().to_string()).collect(),
        metrics: MetricsOut::from(&m),
        pass: verdict.pass,
        violations: verdict.violations,
        elapsed_us,
    })
}

/// Runs every scenario on a dedicated pool of `threads` workers. Results keep input order.
pub fn run_all(specs: &[ScenarioSpec], sla: &Sla, threads: usize) -> Result<Vec<ScenarioResult>> {
    ffi::assert_abi()?;
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build()?;
    pool.install(|| specs.par_iter().map(|s| run_one(s, sla)).collect())
}
