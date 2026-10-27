#!/usr/bin/env bash
# Runs one full validation cycle through Airflow (all tasks, in dependency order).
#   FLEET_EXEC_MODE=local|kubernetes   FLEET_SHARDS=4   FLEET_SEEDS_PER_EVENT=20
set -euo pipefail
source "$(dirname "$0")/airflow_env.sh"
# One-time metadata DB setup stays out of the run itself.
[ -f "$AIRFLOW_HOME/airflow.db" ] || "$AIRFLOW_BIN" db migrate >/dev/null 2>&1
"$AIRFLOW_BIN" dags test fleet_telemetry_to_scenarios "${1:-$(date +%Y-%m-%d)}"
