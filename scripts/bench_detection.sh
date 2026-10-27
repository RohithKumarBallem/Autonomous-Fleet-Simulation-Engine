#!/usr/bin/env bash
# Telemetry ingest throughput and anomaly-detection quality over several independent synthetic feeds.
# Seed 1 is the feed the detector rules were tuned on; seeds 2..N are held out.
#   -> bench/results/detection.json, bench/results/ingest.json
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; F="$ROOT/orchestrator/target/release/fleetctl"
W="$ROOT/bench/work/detection"; rm -rf "$W"; mkdir -p "$W" "$ROOT/bench/results"
SEEDS="${SEEDS:-1 2 3 4 5 6}"
for s in $SEEDS; do
  "$F" telemetry-gen --out "$W/t$s.ndjson" --truth "$W/truth$s.json" --seed "$s" >/dev/null
  "$F" ingest --input "$W/t$s.ndjson" --out "$W/ingest$s.json" >/dev/null
  "$F" score --ingest "$W/ingest$s.json" --truth "$W/truth$s.json" --out "$W/score$s.json" >/dev/null
done
first=${SEEDS%% *}
python3 - "$W" "$ROOT/bench/results/detection.json" $SEEDS <<'PY'
import json, sys
w, out, seeds = sys.argv[1], sys.argv[2], sys.argv[3:]
tuned = seeds[0]
def agg(which):
    t = d = m = 0; kinds = {}
    for s in which:
        sc = json.load(open(f"{w}/score{s}.json"))
        t += sc["truth_events"]; d += sc["detected_events"]; m += sc["matched"]
        for k, v in sc["per_kind"].items():
            x = kinds.setdefault(k, [0, 0, 0]); x[0] += v["truth"]; x[1] += v["detected"]; x[2] += v["matched"]
    return {"feeds": len(which), "truth_events": t, "detected_events": d, "matched": m,
            "precision": round(m / max(d, 1), 3), "recall": round(m / max(t, 1), 3),
            "per_kind": {k: {"truth": v[0], "detected": v[1], "matched": v[2],
                             "recall": round(v[2] / max(v[0], 1), 2), "precision": round(v[2] / max(v[1], 1), 2)}
                         for k, v in sorted(kinds.items())}}
stats = json.load(open(f"{w}/ingest{tuned}.json"))["stats"]
json.dump({"all_feeds": agg(seeds), "held_out_feeds": agg(seeds[1:]), "feed_stats": stats}, open(out, "w"), indent=2)
PY
# Ingest throughput on the first feed: 3 runs each at 1 thread and at all cores.
python3 - "$ROOT/bench/results/ingest.json" "$F" "$W/t$first.ndjson" "$W/ing_tmp.json" <<'PY'
import json, subprocess, statistics, sys, os
out, f, feed, tmp = sys.argv[1:5]
res = {"lines": sum(1 for _ in open(feed, "rb")), "bytes": os.path.getsize(feed), "runs": {}}
for th in (1, os.cpu_count()):
    secs = []
    for _ in range(3):
        o = subprocess.run([f, "--threads", str(th), "ingest", "--input", feed, "--out", tmp], capture_output=True, text=True, check=True)
        secs.append(json.loads(o.stdout)["seconds"])
    med = statistics.median(secs)
    res["runs"][str(th)] = {"seconds": secs, "median_s": med, "records_per_second": res["lines"] / med}
json.dump(res, open(out, "w"), indent=2)
PY
echo "wrote bench/results/detection.json and ingest.json"
