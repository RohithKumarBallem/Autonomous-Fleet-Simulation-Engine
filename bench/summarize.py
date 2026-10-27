#!/usr/bin/env python3
"""Builds bench/results/METRICS.md from the raw JSON in bench/results. Nothing here is hand-typed."""
import glob, json, os, statistics

R = os.path.join(os.path.dirname(__file__), "results")
load = lambda name: json.load(open(os.path.join(R, name)))
out = []

# --- thread scaling ---
runs = [json.load(open(f)) for f in sorted(glob.glob(os.path.join(R, "scaling-run*.json")))]
if runs:
    n = runs[0]["scenarios"]
    threads = [row["threads"] for row in runs[0]["rows"]]
    base = statistics.median(r["rows"][0]["seconds"] for r in runs)
    out += [f"## Simulation throughput ({n} scenarios, median of {len(runs)} runs)\n",
            "| threads | seconds | scenarios/s | M vehicle-ticks/s | speed-up | efficiency |", "|---:|---:|---:|---:|---:|---:|"]
    for k, t in enumerate(threads):
        secs = [r["rows"][k]["seconds"] for r in runs]; m = statistics.median(secs)
        vt = statistics.median(r["rows"][k]["vehicle_ticks_per_second"] for r in runs) / 1e6
        out.append(f"| {t} | {m:.2f} ({min(secs):.2f}-{max(secs):.2f}) | {n / m:.0f} | {vt:.0f} | {base / m:.2f}x | {base / m / (t / threads[0]):.0%} |")
    same = all(r["digests_identical_across_thread_counts"] for r in runs) and len({row["suite_digest"] for r in runs for row in r["rows"]}) == 1
    out += ["", f"Suite digest identical across every thread count and every run: **{same}**\n"]

# --- ingest ---
if os.path.exists(os.path.join(R, "ingest.json")):
    i = load("ingest.json")
    out += [f"## Telemetry ingest ({i['lines']:,} lines, {i['bytes'] / 1e6:.0f} MB)\n", "| threads | median seconds | records/s |", "|---:|---:|---:|"]
    for th, v in i["runs"].items():
        out.append(f"| {th} | {v['median_s']:.3f} | {v['records_per_second'] / 1e6:.2f} M |")
    out.append("")

# --- detection ---
if os.path.exists(os.path.join(R, "detection.json")):
    d = load("detection.json")
    out += ["## Anomaly detection vs injected ground truth\n", "| feeds | injected | detected | matched | precision | recall |", "|---|---:|---:|---:|---:|---:|"]
    for label, k in (("all", "all_feeds"), ("held-out only", "held_out_feeds")):
        x = d[k]; out.append(f"| {label} ({x['feeds']}) | {x['truth_events']} | {x['detected_events']} | {x['matched']} | {x['precision']:.2f} | {x['recall']:.2f} |")
    out += ["", "| fault type | injected | detected | matched | recall | precision |", "|---|---:|---:|---:|---:|---:|"]
    for k, v in d["all_feeds"]["per_kind"].items():
        out.append(f"| {k} | {v['truth']} | {v['detected']} | {v['matched']} | {v['recall']:.2f} | {v['precision']:.2f} |")
    out.append("")

# --- validation cycle ---
cyc = sorted((json.load(open(f)) for f in glob.glob(os.path.join(R, "cycle-seeds*.json"))), key=lambda c: c["workload"]["scenarios"])
if cyc:
    out += ["## End-to-end validation cycle: sequential baseline vs Airflow pipeline (median, runs interleaved)\n",
            "| scenarios | runs | baseline (min-max) | pipeline (min-max) | cycle-time change | speed-up | same results |", "|---:|---:|---:|---:|---:|---:|:---:|"]
    for c in cyc:
        chg = f"{-c['cycle_time_reduction_pct']:+.1f}%" if c["cycle_time_reduction_pct"] < 0 else f"-{c['cycle_time_reduction_pct']:.1f}%"
        b, p = c["baseline_seconds"], c["pipeline_seconds"]
        out.append(f"| {c['workload']['scenarios']:,} | {c.get('reps', len(b))} | {c['baseline_median_s']:.1f} s ({min(b):.1f}-{max(b):.1f}) | {c['pipeline_median_s']:.1f} s ({min(p):.1f}-{max(p):.1f}) | {chg} | {c['speedup_x']:.2f}x | {'yes' if c['suite_digest_identical'] else 'NO'} |")
    out.append("")

# --- kubernetes ---
k8s = sorted(glob.glob(os.path.join(R, "k8s-pods*.json")), key=lambda p: json.load(open(p))["pods"])
if k8s:
    rows = [json.load(open(p)) for p in k8s]
    out += ["## Kubernetes Indexed Job (kind, 3 nodes): same suite, different pod counts\n",
            "| pods | scenarios | results identical to the local run | job wall time (indicative only) |", "|---:|---:|:---:|---:|"]
    for r in rows:
        out.append(f"| {r['pods']} | {r['scenarios']} | {'yes' if r['digest_identical'] else 'NO'} | {r['median_job_wall_seconds']:.0f} s |")
    out += ["", "Wall times above were taken while unrelated containers saturated the shared Docker VM, so they are not a scaling benchmark.", ""]
if os.path.exists(os.path.join(R, "k8s-airflow-run.json")):
    a = load("k8s-airflow-run.json")
    out += [f"Airflow `KubernetesPodOperator` run: {a['shard_pods']} shard pods, {a['scenarios']} scenarios, "
            f"{a['replay_verify']['replayed']} replayed locally with {a['replay_verify']['mismatches']} mismatches, "
            f"{a['invariant_violations']} invariant violations.", ""]

open(os.path.join(R, "METRICS.md"), "w").write("\n".join(out))
print("\n".join(out))
