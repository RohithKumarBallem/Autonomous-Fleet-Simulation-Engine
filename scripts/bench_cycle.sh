#!/usr/bin/env bash
# End-to-end validation-cycle benchmark.
#
#   baseline : one worker runs every stage by hand, one scenario at a time (--threads 1)
#   pipeline : the Airflow DAG, sharded, using every core
#
# Both consume the identical synthetic field feed (same seed) and must produce the identical
# suite digest; the script fails if they do not, so a speed-up can never come from doing less work.
#
#   SEEDS=20 REPS=3 scripts/bench_cycle.sh  ->  bench/results/cycle-seeds20.json
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
F="$ROOT/orchestrator/target/release/fleetctl"
SEEDS="${SEEDS:-20}"; REPS="${REPS:-3}"; DS="${DS:-2026-10-26}"; SEED="${DS//-/}"
OUT="$ROOT/bench/results/cycle-seeds${SEEDS}.json"; WORK="$ROOT/bench/work"
now() { python3 -c 'import time; print(time.time())'; }
mkdir -p "$WORK" "$ROOT/bench/results"

baseline() {
  local w="$WORK/baseline-$1"; rm -rf "$w"; mkdir -p "$w"
  local t0; t0=$(now)
  "$F" --threads 1 telemetry-gen --out "$w/telemetry.ndjson" --truth "$w/truth.json" --seed "$SEED" >/dev/null
  "$F" --threads 1 ingest --input "$w/telemetry.ndjson" --out "$w/ingest.json" >/dev/null
  "$F" --threads 1 scenarios --ingest "$w/ingest.json" --out "$w/scenarios.ndjson" --seeds "$SEEDS" >/dev/null
  "$F" --threads 1 run --scenarios "$w/scenarios.ndjson" --out-dir "$w/results" >/dev/null
  "$F" --threads 1 aggregate --results-dir "$w/results" --report "$w/report.json" >/dev/null
  "$F" --threads 1 verify --scenarios "$w/scenarios.ndjson" --results-dir "$w/results" --sample 100 >/dev/null
  python3 -c "print(round($(now) - $t0, 3))"
}

pipeline() {
  rm -rf "$ROOT/out/airflow"
  local t0; t0=$(now)
  FLEET_SEEDS_PER_EVENT="$SEEDS" FLEET_SHARDS=4 "$ROOT/scripts/run_pipeline.sh" "$DS" >"$WORK/pipeline-$1.log" 2>&1
  python3 -c "print(round($(now) - $t0, 3))"
}

digest() { python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['suite_digest'])" "$1"; }

base_t=(); pipe_t=()
for i in $(seq 1 "$REPS"); do
  b=$(baseline "$i"); base_t+=("$b"); echo "baseline  run $i: ${b}s" >&2
  p=$(pipeline "$i"); pipe_t+=("$p"); echo "pipeline  run $i: ${p}s" >&2
done
d_base=$(digest "$WORK/baseline-$REPS/report.json")
d_pipe=$(digest "$(ls -d "$ROOT"/out/airflow/*/ | head -1)report.json")
[ "$d_base" = "$d_pipe" ] || { echo "DIGEST MISMATCH: baseline $d_base vs pipeline $d_pipe" >&2; exit 1; }
n=$(python3 -c "import json; print(json.load(open('$WORK/baseline-$REPS/report.json'))['scenarios'])")

python3 - "$OUT" "$SEEDS" "$n" "$d_base" "${base_t[*]}" "${pipe_t[*]}" <<'PY'
import json, statistics, sys
out, seeds, n, digest, base, pipe = sys.argv[1:7]
base = [float(x) for x in base.split()]; pipe = [float(x) for x in pipe.split()]
mb, mp = statistics.median(base), statistics.median(pipe)
json.dump({
    "workload": {"seeds_per_event": int(seeds), "scenarios": int(n), "field_vehicles": 500, "field_hours": 6},
    "reps": len(base), "baseline_seconds": base, "pipeline_seconds": pipe,
    "baseline_median_s": mb, "pipeline_median_s": mp,
    "cycle_time_reduction_pct": round((1 - mp / mb) * 100, 1),
    "speedup_x": round(mb / mp, 2),
    "suite_digest_identical": True, "suite_digest": digest,
}, open(out, "w"), indent=2)
print(open(out).read())
PY
