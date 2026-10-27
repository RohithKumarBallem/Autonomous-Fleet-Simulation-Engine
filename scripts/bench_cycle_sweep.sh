#!/usr/bin/env bash
# Runs bench_cycle.sh for several suite sizes. The headline workload is SEEDS=20 (the DAG default).
set -euo pipefail
for s in ${SWEEP:-20 10 50 100}; do
  echo "=== suite size: $s seeds per event ===" >&2
  SEEDS=$s "$(dirname "$0")/bench_cycle.sh" > /dev/null
done
