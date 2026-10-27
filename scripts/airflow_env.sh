# Source me: points Airflow at this repo's DAGs and a local SQLite metadata DB.
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export AIRFLOW_HOME="$ROOT/pipelines/airflow/airflow_home"
export AIRFLOW__CORE__DAGS_FOLDER="$ROOT/pipelines/airflow/dags"
export AIRFLOW__CORE__LOAD_EXAMPLES=False
export AIRFLOW__CORE__EXECUTOR=SequentialExecutor
export AIRFLOW_BIN="${AIRFLOW_BIN:-$ROOT/pipelines/airflow/.venv/bin/airflow}"
mkdir -p "$AIRFLOW_HOME"
