#!/usr/bin/env bash
# Whole pipeline on one machine, no Airflow or Kubernetes required.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
F="$ROOT/orchestrator/target/release/fleetctl"
D="$ROOT/out/demo"; rm -rf "$D"; mkdir -p "$D"

echo "1/6 generate synthetic field telemetry (6 h, 500 vehicles, 24 injected anomalies)"
"$F" telemetry-gen --out "$D/telemetry.ndjson" --truth "$D/truth.json" --seed 1
echo "2/6 ingest: clean the feed, detect anomalous episodes, profile demand"
"$F" ingest --input "$D/telemetry.ndjson" --out "$D/ingest.json"
echo "3/6 score detection against the injected ground truth"
"$F" score --ingest "$D/ingest.json" --truth "$D/truth.json" --out "$D/detection_score.json" >/dev/null
python3 - "$D/detection_score.json" <<'PY'
import json, sys
s = json.load(open(sys.argv[1]))
print(f"    precision {s['precision']:.2f}  recall {s['recall']:.2f}  ({s['matched']}/{s['truth_events']} injected events found)")
PY
echo "4/6 build the scenario suite"
"$F" scenarios --ingest "$D/ingest.json" --out "$D/scenarios.ndjson"
echo "5/6 simulate every scenario"
"$F" run --scenarios "$D/scenarios.ndjson" --out-dir "$D/results"
"$F" aggregate --results-dir "$D/results" --report "$D/report.json" --markdown "$D/report.md"
echo "6/6 replay a sample on 2 threads and require bit-identical results"
"$F" --threads 2 verify --scenarios "$D/scenarios.ndjson" --results-dir "$D/results" --sample 100
echo; cat "$D/report.md"
