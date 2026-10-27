"""Telemetry -> reproducible scenarios -> sharded fleet validation -> report.

One DAG run is one validation cycle:

    acquire_telemetry -> ingest_and_detect -> quality_gate -> build_scenarios
        -> run_shard_0 .. run_shard_{N-1}   (parallel fan-out)
        -> aggregate_report -> replay_verify -> publish_gate
                                                 (+ score_detection when ground truth exists)

Execution mode is selected with FLEET_EXEC_MODE:
    local       shards run as `fleetctl run --shard i/N` processes (default; used for development)
    kubernetes  each shard is a pod created with KubernetesPodOperator

All configuration comes from environment variables so the same DAG file runs unchanged on a
laptop and on a cluster.
"""

from __future__ import annotations

import json
import os
from datetime import datetime, timedelta
from pathlib import Path

from airflow.decorators import dag, task
from airflow.exceptions import AirflowFailException
from airflow.operators.bash import BashOperator

REPO = Path(os.environ.get("FLEET_HOME", Path(__file__).resolve().parents[3]))
FLEETCTL = os.environ.get("FLEETCTL_BIN", str(REPO / "orchestrator" / "target" / "release" / "fleetctl"))
WORK_ROOT = os.environ.get("FLEET_WORK_DIR", str(REPO / "out" / "airflow"))
SHARDS = int(os.environ.get("FLEET_SHARDS", "4"))
EXEC_MODE = os.environ.get("FLEET_EXEC_MODE", "local")
SHARD_THREADS = os.environ.get("FLEET_SHARD_THREADS", "0")  # 0 = all cores
VEHICLES_FIELD = os.environ.get("FLEET_FIELD_VEHICLES", "500")
FIELD_HOURS = os.environ.get("FLEET_FIELD_HOURS", "6")
SEEDS_PER_EVENT = os.environ.get("FLEET_SEEDS_PER_EVENT", "20")
TELEMETRY_INPUT = os.environ.get("FLEET_TELEMETRY_INPUT", "")  # use an existing feed instead of generating
IMAGE = os.environ.get("FLEET_IMAGE", "fleet-sim:latest")
K8S_NAMESPACE = os.environ.get("FLEET_K8S_NAMESPACE", "fleet")
K8S_PVC = os.environ.get("FLEET_K8S_PVC", "fleet-data")
K8S_MOUNT = "/data"

MAX_MALFORMED_RATIO = 0.05

# `ts_nodash` keeps every run in its own directory, so retries of one run are idempotent
# (shard files are written atomically) and different runs never collide.
WORK = f"{WORK_ROOT}/{{{{ ts_nodash }}}}"
POD_WORK = f"{K8S_MOUNT}/{{{{ ts_nodash }}}}"


def _threads_flag() -> str:
    return f"--threads {SHARD_THREADS}" if SHARD_THREADS != "0" else ""


default_args = {
    "owner": "fleet-sim",
    "retries": 1,
    "retry_delay": timedelta(seconds=10),
}


@dag(
    dag_id="fleet_telemetry_to_scenarios",
    description="Field telemetry -> reproducible adversarial scenarios -> distributed validation",
    start_date=datetime(2026, 1, 1),
    schedule=None,  # triggered per telemetry batch; use "@hourly" for a rolling feed
    catchup=False,
    max_active_runs=1,
    default_args=default_args,
    tags=["fleet-sim", "validation"],
)
def fleet_pipeline():
    if TELEMETRY_INPUT:
        acquire = BashOperator(
            task_id="acquire_telemetry",
            bash_command=f"mkdir -p {WORK} && ln -sf {TELEMETRY_INPUT} {WORK}/telemetry.ndjson",
        )
    else:
        # Stand-in for pulling a batch from the production feed (object store / Kafka sink).
        acquire = BashOperator(
            task_id="acquire_telemetry",
            bash_command=(
                f"mkdir -p {WORK} && {FLEETCTL} telemetry-gen --out {WORK}/telemetry.ndjson "
                f"--truth {WORK}/truth.json --vehicles {VEHICLES_FIELD} --hours {FIELD_HOURS} "
                "--seed {{ ds_nodash }}"
            ),
        )

    ingest = BashOperator(
        task_id="ingest_and_detect",
        bash_command=(
            f"{FLEETCTL} ingest --input {WORK}/telemetry.ndjson --out {WORK}/ingest.json | tee {WORK}/ingest.stdout"
        ),
    )

    @task
    def quality_gate(work: str) -> dict:
        """Stop the run if the feed is too damaged to trust, rather than validating on bad data."""
        ingest_doc = json.loads(Path(work, "ingest.json").read_text())
        stats = ingest_doc["stats"]
        ratio = stats["malformed"] / max(stats["lines_total"], 1)
        if ratio > MAX_MALFORMED_RATIO:
            raise AirflowFailException(f"malformed ratio {ratio:.3f} exceeds {MAX_MALFORMED_RATIO}")
        if not ingest_doc["events"]:
            raise AirflowFailException("no anomalous episodes detected; nothing to validate")
        return {"events": len(ingest_doc["events"]), "malformed_ratio": round(ratio, 5), "records": stats["lines_total"]}

    gate = quality_gate.override(task_id="quality_gate")(WORK)

    build = BashOperator(
        task_id="build_scenarios",
        bash_command=(
            f"{FLEETCTL} scenarios --ingest {WORK}/ingest.json --out {WORK}/scenarios.ndjson --seeds {SEEDS_PER_EVENT}"
        ),
    )

    shard_tasks = []
    for i in range(SHARDS):
        if EXEC_MODE == "kubernetes":
            from airflow.providers.cncf.kubernetes.operators.pod import KubernetesPodOperator
            from kubernetes.client import models as k8s

            shard_tasks.append(
                KubernetesPodOperator(
                    task_id=f"run_shard_{i}",
                    name=f"fleet-shard-{i}",
                    namespace=K8S_NAMESPACE,
                    image=IMAGE,
                    image_pull_policy="IfNotPresent",
                    cmds=["fleetctl"],
                    arguments=(
                        (["--threads", SHARD_THREADS] if SHARD_THREADS != "0" else [])
                        + ["run", "--scenarios", f"{POD_WORK}/scenarios.ndjson", "--out-dir", f"{POD_WORK}/results",
                           "--shard", f"{i}/{SHARDS}"]
                    ),
                    volumes=[k8s.V1Volume(name="data", persistent_volume_claim=k8s.V1PersistentVolumeClaimVolumeSource(claim_name=K8S_PVC))],
                    volume_mounts=[k8s.V1VolumeMount(name="data", mount_path=K8S_MOUNT)],
                    container_resources=k8s.V1ResourceRequirements(requests={"cpu": "1", "memory": "256Mi"}),
                    get_logs=True,
                    on_finish_action="delete_pod",
                )
            )
        else:
            shard_tasks.append(
                BashOperator(
                    task_id=f"run_shard_{i}",
                    bash_command=(
                        f"{FLEETCTL} {_threads_flag()} run --scenarios {WORK}/scenarios.ndjson "
                        f"--out-dir {WORK}/results --shard {i}/{SHARDS}"
                    ),
                )
            )

    aggregate = BashOperator(
        task_id="aggregate_report",
        bash_command=(
            f"{FLEETCTL} aggregate --results-dir {WORK}/results --report {WORK}/report.json --markdown {WORK}/report.md"
        ),
    )

    verify = BashOperator(
        task_id="replay_verify",
        # Re-run a sample on a different thread count; any divergence fails the run.
        bash_command=(
            f"{FLEETCTL} --threads 3 verify --scenarios {WORK}/scenarios.ndjson --results-dir {WORK}/results --sample 100"
        ),
    )

    @task
    def publish_gate(work: str, expected_shards: int) -> dict:
        """Hard-fail on engine faults. SLA failures are findings, not pipeline errors."""
        report = json.loads(Path(work, "report.json").read_text())
        shard_files = list(Path(work, "results").glob("shard-*.ndjson"))
        if len(shard_files) != expected_shards:
            raise AirflowFailException(f"expected {expected_shards} shard files, found {len(shard_files)}")
        if report["invariant_violations"] != 0:
            raise AirflowFailException(f"{report['invariant_violations']} simulation invariant violations")
        return {
            "scenarios": report["scenarios"],
            "pass_rate": round(report["pass_rate"], 4),
            "failed": report["failed"],
            "suite_digest": report["suite_digest"],
        }

    publish = publish_gate.override(task_id="publish_gate")(WORK, SHARDS)

    score = BashOperator(
        task_id="score_detection",
        bash_command=(
            f"test -f {WORK}/truth.json && {FLEETCTL} score --ingest {WORK}/ingest.json "
            f"--truth {WORK}/truth.json --out {WORK}/detection_score.json || echo 'no ground truth for this feed; skipped'"
        ),
    )

    acquire >> ingest >> gate >> build >> shard_tasks >> aggregate >> verify >> publish
    ingest >> score


fleet_pipeline()
