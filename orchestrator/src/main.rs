use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use fleetctl::extract::{build_suite, SuiteConfig};
use fleetctl::ingest::{self, IngestOutput};
use fleetctl::report;
use fleetctl::runner::{self, ScenarioResult, Shard};
use fleetctl::spec::{read_ndjson, ScenarioSpec};
use fleetctl::telemetry::{self, GenConfig, TruthEvent};
use fleetctl::validate::Sla;
use fleetctl::ffi;
use serde_json::json;
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(name = "fleetctl", version, about = "Autonomous Fleet Simulation Engine: orchestrator")]
struct Cli {
    /// Worker threads for parsing and simulation (default: all cores).
    #[arg(long, global = true)]
    threads: Option<usize>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print kernel version and verify the Rust/C++ ABI.
    Info,
    /// Generate synthetic field telemetry (with injected anomalies) and a ground-truth file.
    TelemetryGen {
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        truth: PathBuf,
        #[arg(long, default_value_t = 500)]
        vehicles: u32,
        #[arg(long, default_value_t = 6.0)]
        hours: f64,
        #[arg(long, default_value_t = 24)]
        anomalies: usize,
        #[arg(long, default_value_t = 1)]
        seed: u64,
        #[arg(long, default_value_t = 4.5)]
        rate: f64,
    },
    /// Clean a telemetry feed, detect anomalous episodes, and profile normal demand.
    Ingest {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Score detected events against the generator's ground truth.
    Score {
        #[arg(long)]
        ingest: PathBuf,
        #[arg(long)]
        truth: PathBuf,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// Build the reproducible scenario suite from ingest output.
    Scenarios {
        #[arg(long)]
        ingest: PathBuf,
        #[arg(long)]
        out: PathBuf,
        #[arg(long, default_value_t = 20)]
        seeds: usize,
        #[arg(long, default_value_t = 24)]
        compound: usize,
        #[arg(long, default_value_t = 20)]
        controls: usize,
        #[arg(long, default_value_t = 500)]
        vehicles: i32,
        #[arg(long, default_value_t = 1440)]
        ticks: i32,
    },
    /// Run a scenario suite (or one shard of it) and write per-scenario results.
    Run {
        #[arg(long)]
        scenarios: PathBuf,
        #[arg(long)]
        out_dir: PathBuf,
        /// `index/count`, e.g. 2/8.
        #[arg(long)]
        shard: Option<String>,
        /// Take the shard from JOB_COMPLETION_INDEX and FLEET_SHARDS (Kubernetes Indexed Jobs).
        #[arg(long)]
        shard_from_env: bool,
    },
    /// Merge shard results into one validation report.
    Aggregate {
        #[arg(long)]
        results_dir: PathBuf,
        #[arg(long)]
        report: PathBuf,
        #[arg(long)]
        markdown: Option<PathBuf>,
    },
    /// Re-run a sample of scenarios and check the results are bit-identical.
    Verify {
        #[arg(long)]
        scenarios: PathBuf,
        #[arg(long)]
        results_dir: PathBuf,
        #[arg(long, default_value_t = 100)]
        sample: usize,
    },
    /// Measure throughput at several thread counts and check the results never change.
    BenchScaling {
        #[arg(long)]
        scenarios: PathBuf,
        #[arg(long, default_value_t = 240)]
        limit: usize,
        #[arg(long, value_delimiter = ',', default_value = "1,2,4,8,12")]
        thread_counts: Vec<usize>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn default_threads() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn write_ndjson<T: serde::Serialize>(path: &Path, items: &[T]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // Write-then-rename so a retried Kubernetes pod can never leave a half-written shard behind.
    let tmp = path.with_extension("tmp");
    let mut w = BufWriter::new(File::create(&tmp)?);
    for it in items {
        serde_json::to_writer(&mut w, it)?;
        w.write_all(b"\n")?;
    }
    w.flush()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    Ok(serde_json::from_reader(std::io::BufReader::new(f))?)
}

fn write_json<T: serde::Serialize>(path: &Path, v: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(v)?)?;
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let threads = cli.threads.unwrap_or_else(default_threads);
    rayon::ThreadPoolBuilder::new().num_threads(threads).build_global().ok();

    match cli.cmd {
        Cmd::Info => {
            ffi::assert_abi()?;
            println!("{}", json!({"core": ffi::core_version(), "abi": "ok", "threads": threads}));
        }

        Cmd::TelemetryGen { out, truth, vehicles, hours, anomalies, seed, rate } => {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            let t0 = Instant::now();
            let s = telemetry::generate(&GenConfig { vehicles, hours, anomalies, seed, base_rate: rate }, &out, &truth)?;
            println!("{}", json!({"summary": s, "seconds": t0.elapsed().as_secs_f64()}));
        }

        Cmd::Ingest { input, out } => {
            let t0 = Instant::now();
            let res = ingest::ingest(&input)?;
            let secs = t0.elapsed().as_secs_f64();
            write_json(&out, &res)?;
            println!(
                "{}",
                json!({
                    "stats": res.stats,
                    "events_detected": res.events.len(),
                    "seconds": secs,
                    "records_per_second": res.stats.lines_total as f64 / secs,
                })
            );
        }

        Cmd::Score { ingest, truth, out } => {
            let det: IngestOutput = read_json(&ingest)?;
            let truth: Vec<TruthEvent> = read_json(&truth)?;
            let score = ingest::score(&det.events, &truth);
            if let Some(o) = out {
                write_json(&o, &score)?;
            }
            println!("{}", serde_json::to_string(&score)?);
        }

        Cmd::Scenarios { ingest, out, seeds, compound, controls, vehicles, ticks } => {
            let det: IngestOutput = read_json(&ingest)?;
            let cfg = SuiteConfig { seeds_per_event: seeds, compound, controls, vehicles, ticks, ..Default::default() };
            let suite = build_suite(&det, &cfg);
            write_ndjson(&out, &suite)?;
            println!("{}", json!({"scenarios": suite.len(), "events": det.events.len()}));
        }

        Cmd::Run { scenarios, out_dir, shard, shard_from_env } => {
            let shard = if shard_from_env {
                let index: usize = std::env::var("JOB_COMPLETION_INDEX").context("JOB_COMPLETION_INDEX not set")?.parse()?;
                let count: usize = std::env::var("FLEET_SHARDS").context("FLEET_SHARDS not set")?.parse()?;
                if index >= count {
                    bail!("JOB_COMPLETION_INDEX {index} >= FLEET_SHARDS {count}");
                }
                Shard { index, count }
            } else {
                shard.as_deref().map(Shard::parse).transpose()?.unwrap_or_else(Shard::whole)
            };
            let all: Vec<ScenarioSpec> = read_ndjson(&scenarios)?;
            let mine = runner::select_shard(all, shard);
            let t0 = Instant::now();
            let results = runner::run_all(&mine, &Sla::default(), threads)?;
            let secs = t0.elapsed().as_secs_f64();
            let file = out_dir.join(format!("shard-{:04}-of-{:04}.ndjson", shard.index, shard.count));
            write_ndjson(&file, &results)?;
            let vt: u64 = results.iter().map(|r| r.metrics.vehicle_ticks).sum();
            println!(
                "{}",
                json!({
                    "shard": format!("{}/{}", shard.index, shard.count),
                    "scenarios": results.len(),
                    "seconds": secs,
                    "scenarios_per_second": results.len() as f64 / secs,
                    "vehicle_ticks_per_second": vt as f64 / secs,
                    "out": file,
                })
            );
        }

        Cmd::Aggregate { results_dir, report: report_path, markdown } => {
            let results = report::load_results(&results_dir)?;
            let rep = report::aggregate(&results);
            write_json(&report_path, &rep)?;
            if let Some(md) = markdown {
                fs::write(md, report::markdown(&rep))?;
            }
            println!(
                "{}",
                json!({"scenarios": rep.scenarios, "passed": rep.passed, "failed": rep.failed,
                       "invariant_violations": rep.invariant_violations, "suite_digest": rep.suite_digest})
            );
        }

        Cmd::Verify { scenarios, results_dir, sample } => {
            let specs: Vec<ScenarioSpec> = read_ndjson(&scenarios)?;
            let results = report::load_results(&results_dir)?;
            let by_id: std::collections::HashMap<&str, &ScenarioResult> = results.iter().map(|r| (r.id.as_str(), r)).collect();
            let step = (specs.len() / sample.max(1)).max(1);
            let picked: Vec<ScenarioSpec> = specs.into_iter().step_by(step).take(sample).collect();
            let replay = runner::run_all(&picked, &Sla::default(), threads)?;
            let (mut mismatches, mut missing) = (0, 0);
            for r in &replay {
                match by_id.get(r.id.as_str()) {
                    None => missing += 1,
                    Some(orig) if orig.metrics.state_hash != r.metrics.state_hash => mismatches += 1,
                    _ => {}
                }
            }
            println!("{}", json!({"replayed": replay.len(), "missing": missing, "mismatches": mismatches, "threads": threads}));
            if mismatches > 0 {
                bail!("{mismatches} replayed scenarios diverged from their recorded state hash");
            }
        }

        Cmd::BenchScaling { scenarios, limit, thread_counts, out } => {
            let mut specs: Vec<ScenarioSpec> = read_ndjson(&scenarios)?;
            specs.sort_by(|a, b| a.id.cmp(&b.id));
            specs.truncate(limit);
            let sla = Sla::default();
            let mut rows = vec![];
            let mut base_secs = 0.0;
            let mut digests = std::collections::BTreeSet::new();
            // Warm-up so the first measurement does not pay for page faults and cache misses.
            runner::run_all(&specs[..specs.len().min(24)], &sla, 1)?;
            for &t in &thread_counts {
                let t0 = Instant::now();
                let results = runner::run_all(&specs, &sla, t)?;
                let secs = t0.elapsed().as_secs_f64();
                if rows.is_empty() {
                    base_secs = secs;
                }
                let vt: u64 = results.iter().map(|r| r.metrics.vehicle_ticks).sum();
                let d = report::digest(&results);
                digests.insert(d.clone());
                let speedup = base_secs / secs;
                rows.push(json!({
                    "threads": t,
                    "seconds": secs,
                    "scenarios_per_second": results.len() as f64 / secs,
                    "vehicle_ticks_per_second": vt as f64 / secs,
                    "speedup_vs_first": speedup,
                    "parallel_efficiency": speedup / (t as f64 / thread_counts[0] as f64),
                    "suite_digest": d,
                }));
                eprintln!("threads={t:>2}  {secs:7.2}s  {:.0} scenarios/s", results.len() as f64 / secs);
            }
            let out_json = json!({
                "scenarios": specs.len(),
                "digests_identical_across_thread_counts": digests.len() == 1,
                "rows": rows,
            });
            if let Some(o) = out {
                write_json(&o, &out_json)?;
            }
            println!("{}", serde_json::to_string(&out_json)?);
            if digests.len() != 1 {
                bail!("results changed with the thread count: nondeterminism detected");
            }
        }
    }
    Ok(())
}
