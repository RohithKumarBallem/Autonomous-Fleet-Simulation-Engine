.PHONY: all build core rust test demo bench-scaling bench-cycle clean

all: build

build: core rust

core:
	$(MAKE) -C core

rust:
	cd orchestrator && cargo build --release

test: core
	$(MAKE) -C core test
	cd orchestrator && cargo test --release

# Generate telemetry -> detect events -> build scenarios -> simulate -> report -> replay-verify.
demo: rust
	scripts/demo.sh

bench-scaling: rust demo
	orchestrator/target/release/fleetctl bench-scaling --scenarios out/demo/scenarios.ndjson \
	  --limit 480 --out bench/results/scaling.json

bench-cycle: rust
	scripts/bench_cycle.sh

clean:
	$(MAKE) -C core clean
	cd orchestrator && cargo clean
	rm -rf out data bench/work
