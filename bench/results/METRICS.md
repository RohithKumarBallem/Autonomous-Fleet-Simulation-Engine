## Simulation throughput (460 scenarios, median of 3 runs)

| threads | seconds | scenarios/s | M vehicle-ticks/s | speed-up | efficiency |
|---:|---:|---:|---:|---:|---:|
| 1 | 6.34 (6.19-6.41) | 73 | 52 | 1.00x | 100% |
| 2 | 3.25 (3.23-3.29) | 141 | 102 | 1.95x | 97% |
| 4 | 1.88 (1.85-1.91) | 245 | 176 | 3.38x | 84% |
| 8 | 1.28 (1.25-1.29) | 359 | 259 | 4.95x | 62% |
| 12 | 1.12 (1.10-1.14) | 412 | 297 | 5.68x | 47% |

Suite digest identical across every thread count and every run: **True**

## Telemetry ingest (1,102,393 lines, 83 MB)

| threads | median seconds | records/s |
|---:|---:|---:|
| 1 | 0.504 | 2.19 M |
| 12 | 0.248 | 4.44 M |

## Anomaly detection vs injected ground truth

| feeds | injected | detected | matched | precision | recall |
|---|---:|---:|---:|---:|---:|
| all (6) | 144 | 126 | 120 | 0.95 | 0.83 |
| held-out only (5) | 120 | 105 | 99 | 0.94 | 0.82 |

| fault type | injected | detected | matched | recall | precision |
|---|---:|---:|---:|---:|---:|
| demand_surge | 34 | 35 | 32 | 0.94 | 0.91 |
| dispatch_latency | 37 | 21 | 21 | 0.57 | 1.00 |
| gps_spoof | 21 | 20 | 19 | 0.90 | 0.95 |
| phantom_requests | 22 | 21 | 21 | 0.95 | 1.00 |
| sensor_dropout | 30 | 29 | 27 | 0.90 | 0.93 |

## End-to-end validation cycle: sequential baseline vs Airflow pipeline (median, runs interleaved)

| scenarios | runs | baseline (min-max) | pipeline (min-max) | cycle-time change | speed-up | same results |
|---:|---:|---:|---:|---:|---:|:---:|
| 262 | 3 | 6.6 s (6.5-6.9) | 6.1 s (6.0-6.2) | -7.3% | 1.08x | yes |
| 482 | 5 | 8.8 s (8.7-9.3) | 5.9 s (5.8-8.1) | -32.4% | 1.48x | yes |
| 1,142 | 5 | 18.6 s (17.7-20.8) | 9.1 s (7.5-9.5) | -51.3% | 2.05x | yes |
| 2,242 | 3 | 35.7 s (35.2-36.7) | 10.0 s (9.9-11.9) | -72.1% | 3.58x | yes |
| 4,442 | 3 | 67.8 s (65.7-68.8) | 18.2 s (17.5-18.4) | -73.1% | 3.71x | yes |

## Kubernetes Indexed Job (kind, 3 nodes): same suite, different pod counts

| pods | scenarios | results identical to the local run | job wall time (indicative only) |
|---:|---:|:---:|---:|
| 1 | 461 | yes | 34 s |
| 2 | 461 | yes | 21 s |
| 4 | 461 | yes | 11 s |
| 8 | 461 | yes | 11 s |

Wall times above were taken while unrelated containers saturated the shared Docker VM, so they are not a scaling benchmark.

Airflow `KubernetesPodOperator` run: 4 shard pods, 146 scenarios, 100 replayed locally with 0 mismatches, 0 invariant violations.
