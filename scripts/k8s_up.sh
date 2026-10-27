#!/usr/bin/env bash
# Creates a 3-node kind cluster, builds the image, loads it into the nodes and applies the manifests.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; export PATH="$HOME/.local/bin:$PATH"
HOST_DATA_DIR="$ROOT/out/k8s"; mkdir -p "$HOST_DATA_DIR/suite"; chmod -R 777 "$HOST_DATA_DIR"
CFG="$(mktemp)"; sed "s|\${HOST_DATA_DIR}|$HOST_DATA_DIR|g" "$ROOT/deploy/k8s/kind-config.yaml.tmpl" >"$CFG"
kind get clusters 2>/dev/null | grep -qx fleet || kind create cluster --config "$CFG"
docker build -t fleet-sim:latest "$ROOT"
kind load docker-image fleet-sim:latest --name fleet
kubectl --context kind-fleet apply -k "$ROOT/deploy/k8s"
kubectl --context kind-fleet get nodes
