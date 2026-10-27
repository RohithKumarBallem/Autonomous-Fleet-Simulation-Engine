#!/usr/bin/env bash
# Scales one scenario suite across Kubernetes pods (Indexed Job) and checks the results never change.
#   POD_COUNTS="1 2 4 8" THREADS=1 REPS=3 scripts/bench_k8s.sh [scenarios.ndjson] [reference_report.json]
# Needs a running cluster (scripts/k8s_up.sh). Writes bench/results/k8s-pods<N>.json.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SUITE="${1:-$ROOT/out/demo/scenarios.ndjson}"; REF="${2:-$ROOT/out/demo/report.json}"
THREADS="${THREADS:-1}"; REPS="${REPS:-3}"
ref_digest=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['suite_digest'])" "$REF")
for pods in ${POD_COUNTS:-1 2 4 8}; do
  runs=()
  for r in $(seq 1 "$REPS"); do
    runs+=("$(SHARDS=$pods THREADS=$THREADS "$ROOT/scripts/k8s_run_suite.sh" "$SUITE" | tail -1)")
    echo "pods=$pods run=$r: ${runs[$((${#runs[@]} - 1))]}" >&2
  done
  python3 - "$ROOT/bench/results/k8s-pods$pods.json" "$ref_digest" "${runs[@]}" <<'PY'
import json, statistics, sys
out, ref, rows = sys.argv[1], sys.argv[2], [json.loads(x) for x in sys.argv[3:]]
walls = [r["job_wall_seconds"] for r in rows]
json.dump({"pods": rows[0]["pods"], "threads_per_pod": rows[0]["threads_per_pod"], "scenarios": rows[0]["scenarios"],
           "job_wall_seconds": walls, "median_job_wall_seconds": statistics.median(walls),
           "digest_identical": all(r["suite_digest"] == ref for r in rows), "suite_digest": ref}, open(out, "w"), indent=2)
PY
done
