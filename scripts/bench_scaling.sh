#!/usr/bin/env bash
# Thread-scaling benchmark of the C++ kernel through the Rust runner (3 runs; digests must never change).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; F="$ROOT/orchestrator/target/release/fleetctl"
[ -f "$ROOT/out/demo/scenarios.ndjson" ] || "$ROOT/scripts/demo.sh" >/dev/null
for i in 1 2 3; do
  "$F" bench-scaling --scenarios "$ROOT/out/demo/scenarios.ndjson" --limit 460 \
    --thread-counts "${THREADS:-1,2,4,8,12}" --out "$ROOT/bench/results/scaling-run$i.json" >/dev/null
done
