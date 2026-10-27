#!/usr/bin/env bash
# Creates pipelines/airflow/.venv with Airflow pinned to the official constraints.
set -euo pipefail
cd "$(dirname "$0")/../pipelines/airflow"
AIRFLOW_VERSION=2.10.5
PY=$(python3 -c 'import sys; print(f"{sys.version_info.major}.{sys.version_info.minor}")')
python3 -m venv .venv
.venv/bin/pip install -q --upgrade pip
.venv/bin/pip install -q "apache-airflow[cncf.kubernetes]==${AIRFLOW_VERSION}" \
  --constraint "https://raw.githubusercontent.com/apache/airflow/constraints-${AIRFLOW_VERSION}/constraints-${PY}.txt"
echo "Airflow ${AIRFLOW_VERSION} ready in pipelines/airflow/.venv"
