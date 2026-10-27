//! Merges shard outputs into one validation report.

use crate::runner::ScenarioResult;
use crate::spec::{hex, read_ndjson};
use anyhow::Result;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Serialize, Default, Debug)]
pub struct Group {
    pub scenarios: usize,
    pub passed: usize,
    pub failed: usize,
    pub pass_rate: f64,
    pub mean_p95_wait_ticks: f64,
    pub mean_abandon_rate: f64,
}

#[derive(Serialize, Debug)]
pub struct Report {
    pub scenarios: usize,
    pub passed: usize,
    pub failed: usize,
    pub pass_rate: f64,
    pub invariant_violations: i64,
    pub vehicle_ticks: u64,
    pub total_cpu_seconds: f64,
    /// SHA-256 over sorted `id:state_hash` pairs. Equal digests mean two runs of the suite
    /// produced bit-identical simulations, regardless of threads or shard layout.
    pub suite_digest: String,
    pub by_label: BTreeMap<String, Group>,
    pub by_fault: BTreeMap<String, Group>,
    pub failure_reasons: BTreeMap<String, usize>,
}

fn add(g: &mut Group, r: &ScenarioResult) {
    g.scenarios += 1;
    if r.pass {
        g.passed += 1;
    } else {
        g.failed += 1;
    }
    g.mean_p95_wait_ticks += r.metrics.wait_p95_ticks as f64;
    g.mean_abandon_rate += r.metrics.abandoned as f64 / r.metrics.requests_total.max(1) as f64;
}

fn finish(g: &mut Group) {
    let n = g.scenarios.max(1) as f64;
    g.pass_rate = g.passed as f64 / n;
    g.mean_p95_wait_ticks /= n;
    g.mean_abandon_rate /= n;
}

pub fn load_results(dir: &Path) -> Result<Vec<ScenarioResult>> {
    let mut all: Vec<ScenarioResult> = Vec::new();
    let mut files: Vec<_> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().map_or(false, |x| x == "ndjson"))
        .collect();
    files.sort();
    for f in files {
        all.extend(read_ndjson::<ScenarioResult>(&f)?);
    }
    all.sort_by(|a, b| a.id.cmp(&b.id));
    all.dedup_by(|a, b| a.id == b.id); // an at-least-once retry may re-run a scenario
    Ok(all)
}

pub fn digest(results: &[ScenarioResult]) -> String {
    let mut h = Sha256::new();
    for r in results {
        h.update(r.id.as_bytes());
        h.update(b":");
        h.update(r.metrics.state_hash.as_bytes());
        h.update(b"\n");
    }
    hex(&h.finalize())
}

pub fn aggregate(results: &[ScenarioResult]) -> Report {
    let mut rep = Report {
        scenarios: results.len(),
        passed: 0,
        failed: 0,
        pass_rate: 0.0,
        invariant_violations: 0,
        vehicle_ticks: 0,
        total_cpu_seconds: 0.0,
        suite_digest: digest(results),
        by_label: BTreeMap::new(),
        by_fault: BTreeMap::new(),
        failure_reasons: BTreeMap::new(),
    };
    for r in results {
        if r.pass {
            rep.passed += 1;
        } else {
            rep.failed += 1;
        }
        rep.invariant_violations += r.metrics.invariant_violations;
        rep.vehicle_ticks += r.metrics.vehicle_ticks;
        rep.total_cpu_seconds += r.elapsed_us as f64 / 1e6;
        add(rep.by_label.entry(r.label.clone()).or_default(), r);
        if r.faults.len() == 1 {
            add(rep.by_fault.entry(r.faults[0].clone()).or_default(), r);
        }
        for v in &r.violations {
            let key = v.split('=').next().unwrap_or(v).to_string();
            *rep.failure_reasons.entry(key).or_default() += 1;
        }
    }
    rep.pass_rate = rep.passed as f64 / rep.scenarios.max(1) as f64;
    rep.by_label.values_mut().for_each(finish);
    rep.by_fault.values_mut().for_each(finish);
    rep
}

pub fn markdown(rep: &Report) -> String {
    let mut s = String::new();
    s.push_str("# Fleet validation report\n\n");
    s.push_str(&format!(
        "- scenarios: **{}** (passed {}, failed {}, pass rate {:.1}%)\n- invariant violations: **{}**\n- simulated vehicle-ticks: {}\n- suite digest: `{}`\n\n",
        rep.scenarios, rep.passed, rep.failed, rep.pass_rate * 100.0, rep.invariant_violations, rep.vehicle_ticks, rep.suite_digest
    ));
    let table = |title: &str, m: &BTreeMap<String, Group>| {
        let mut t = format!("## {title}\n\n| group | scenarios | pass rate | mean p95 wait (ticks) | mean abandon rate |\n|---|---:|---:|---:|---:|\n");
        for (k, g) in m {
            t.push_str(&format!(
                "| {k} | {} | {:.1}% | {:.1} | {:.2}% |\n",
                g.scenarios,
                g.pass_rate * 100.0,
                g.mean_p95_wait_ticks,
                g.mean_abandon_rate * 100.0
            ));
        }
        t.push('\n');
        t
    };
    s.push_str(&table("By scenario type", &rep.by_label));
    s.push_str(&table("Single-fault scenarios by fault", &rep.by_fault));
    s.push_str("## Failure reasons\n\n");
    for (k, n) in &rep.failure_reasons {
        s.push_str(&format!("- `{k}`: {n}\n"));
    }
    s
}
