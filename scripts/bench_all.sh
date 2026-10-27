#!/usr/bin/env bash
# Every benchmark that does not need a cluster, then the summary. Quiet machine recommended.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
"$ROOT/scripts/bench_scaling.sh"
"$ROOT/scripts/bench_detection.sh"
SWEEP="${SWEEP_HEADLINE:-20 50}" REPS="${REPS_HEADLINE:-5}" "$ROOT/scripts/bench_cycle_sweep.sh"
SWEEP="${SWEEP_OTHER:-10 100 200}" REPS="${REPS_OTHER:-3}" "$ROOT/scripts/bench_cycle_sweep.sh"
python3 "$ROOT/bench/summarize.py" > /dev/null
python3 "$ROOT/bench/plot.py" > /dev/null
echo "done: bench/results/METRICS.md"
