#!/usr/bin/env bash
# Runs a scenario suite as a Kubernetes Indexed Job and aggregates the shard results.
#   SHARDS=4 THREADS=1 scripts/k8s_run_suite.sh path/to/scenarios.ndjson
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; export PATH="$HOME/.local/bin:$PATH"
SUITE="${1:?usage: k8s_run_suite.sh scenarios.ndjson}"; SHARDS="${SHARDS:-4}"; THREADS="${THREADS:-2}"
K="kubectl --context kind-fleet -n fleet"; DIR="$ROOT/out/k8s/suite"
rm -rf "$DIR/results"; mkdir -p "$DIR/results"; chmod -R 777 "$DIR"; cp "$SUITE" "$DIR/scenarios.ndjson"
$K delete job fleet-validate --ignore-not-found --wait=true >/dev/null
now() { python3 -c 'import time; print(time.time())'; }
t0=$(now)
sed -e "s/completions: 4 .*/completions: $SHARDS/" -e "s/parallelism: 4/parallelism: $SHARDS/" \
    -e "s/value: \"4\"/value: \"$SHARDS\"/" -e "s/- \"2\"/- \"$THREADS\"/" \
    "$ROOT/deploy/k8s/job-validate.yaml" | $K apply -f - >/dev/null
$K wait --for=condition=complete job/fleet-validate --timeout=900s >/dev/null
t1=$(now)
"$ROOT/orchestrator/target/release/fleetctl" aggregate --results-dir "$DIR/results" --report "$DIR/report.json" >/dev/null
python3 - "$DIR/report.json" "$SHARDS" "$THREADS" "$t0" "$t1" <<'PY'
import json, sys
r = json.load(open(sys.argv[1])); s, th, t0, t1 = int(sys.argv[2]), int(sys.argv[3]), float(sys.argv[4]), float(sys.argv[5])
print(json.dumps({"pods": s, "threads_per_pod": th, "scenarios": r["scenarios"], "job_wall_seconds": round(t1 - t0, 2),
                  "suite_digest": r["suite_digest"]}))
PY
