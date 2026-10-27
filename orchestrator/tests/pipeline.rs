//! Integration tests: Rust orchestrator + C++ kernel working together.

use fleetctl::extract::{build_suite, SuiteConfig};
use fleetctl::ffi;
use fleetctl::ingest::{self, IngestOutput};
use fleetctl::report;
use fleetctl::runner::{self, Shard};
use fleetctl::spec::ScenarioSpec;
use fleetctl::telemetry::{self, GenConfig, TruthEvent};
use fleetctl::validate::Sla;
use std::path::PathBuf;

fn workdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("fleetctl-test-{}-{name}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn feed(name: &str, vehicles: u32, hours: f64, anomalies: usize, seed: u64) -> (IngestOutput, Vec<TruthEvent>) {
    let d = workdir(name);
    let (t, truth) = (d.join("t.ndjson"), d.join("truth.json"));
    telemetry::generate(&GenConfig { vehicles, hours, anomalies, seed, base_rate: 4.5 }, &t, &truth).unwrap();
    let out = ingest::ingest(&t).unwrap();
    let truth: Vec<TruthEvent> = serde_json::from_slice(&std::fs::read(&truth).unwrap()).unwrap();
    std::fs::remove_dir_all(&d).ok();
    (out, truth)
}

fn small_suite(name: &str) -> Vec<ScenarioSpec> {
    let (ing, _) = feed(name, 500, 6.0, 24, 7);
    let cfg = SuiteConfig { seeds_per_event: 2, compound: 4, controls: 3, ..Default::default() };
    build_suite(&ing, &cfg)
}

#[test]
fn abi_layout_matches_the_cpp_kernel() {
    ffi::assert_abi().unwrap();
    assert!(ffi::core_version().starts_with("fleet-core"));
}

#[test]
fn kernel_rejects_invalid_scenarios() {
    let suite = small_suite("invalid");
    let mut bad = suite[0].clone();
    bad.vehicles = 0;
    assert!(ffi::run(&bad.to_c().unwrap()).is_err());
}

#[test]
fn ingest_removes_exactly_the_injected_defects() {
    let d = workdir("defects");
    let (t, truth) = (d.join("t.ndjson"), d.join("truth.json"));
    let s = telemetry::generate(&GenConfig { vehicles: 200, hours: 2.0, anomalies: 4, seed: 3, base_rate: 4.5 }, &t, &truth).unwrap();
    let out = ingest::ingest(&t).unwrap();
    assert_eq!(out.stats.duplicates_dropped, s.duplicates_injected);
    // Corrupt lines are always rejected; a few extra rejections are spoofed fixes outside the service area.
    assert!(out.stats.malformed >= s.corrupt_injected);
    assert!(out.stats.malformed < s.corrupt_injected + s.corrupt_injected / 10 + 20);
    std::fs::remove_dir_all(&d).ok();
}

#[test]
fn detector_meets_a_precision_and_recall_floor() {
    let (ing, truth) = feed("detector", 500, 6.0, 24, 1);
    let sc = ingest::score(&ing.events, &truth);
    assert!(sc.precision >= 0.8, "precision {:.2}", sc.precision);
    assert!(sc.recall >= 0.6, "recall {:.2}", sc.recall);
    assert!((ing.profile.base_rate - 4.5).abs() < 0.6, "base rate {}", ing.profile.base_rate);
}

#[test]
fn scenario_ids_are_reproducible_and_content_addressed() {
    let (a, _) = feed("ids-a", 500, 6.0, 24, 5);
    let (b, _) = feed("ids-b", 500, 6.0, 24, 5);
    let cfg = SuiteConfig { seeds_per_event: 2, compound: 4, controls: 3, ..Default::default() };
    let (sa, sb) = (build_suite(&a, &cfg), build_suite(&b, &cfg));
    assert_eq!(sa.iter().map(|s| &s.id).collect::<Vec<_>>(), sb.iter().map(|s| &s.id).collect::<Vec<_>>());
    let mut changed = sa[0].clone();
    changed.base_rate += 0.001;
    assert_ne!(changed.content_id(), sa[0].id);
    for s in &sa {
        assert_eq!(s.id, s.content_id());
    }
}

#[test]
fn shards_partition_the_suite_exactly() {
    let suite = small_suite("shards");
    for count in [1usize, 3, 7] {
        let mut seen = std::collections::BTreeSet::new();
        for index in 0..count {
            for s in runner::select_shard(suite.clone(), Shard { index, count }) {
                assert!(seen.insert(s.id), "scenario appears in two shards");
            }
        }
        assert_eq!(seen.len(), suite.len());
    }
}

#[test]
fn results_are_identical_across_thread_counts_and_shard_layouts() {
    let suite = small_suite("determinism");
    let sla = Sla::default();
    let mut whole = runner::run_all(&suite, &sla, 1).unwrap();
    whole.sort_by(|a, b| a.id.cmp(&b.id));
    let reference = report::digest(&whole);

    let mut parallel = runner::run_all(&suite, &sla, 6).unwrap();
    parallel.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(report::digest(&parallel), reference, "thread count changed the result");

    let mut sharded = Vec::new();
    for index in 0..5 {
        let part = runner::select_shard(suite.clone(), Shard { index, count: 5 });
        sharded.extend(runner::run_all(&part, &sla, 2).unwrap());
    }
    sharded.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(report::digest(&sharded), reference, "shard layout changed the result");

    assert!(whole.iter().all(|r| r.metrics.invariant_violations == 0));
    let controls: Vec<_> = whole.iter().filter(|r| r.label == "control").collect();
    assert!(!controls.is_empty() && controls.iter().all(|r| r.pass), "healthy baseline must pass the SLA");
}
