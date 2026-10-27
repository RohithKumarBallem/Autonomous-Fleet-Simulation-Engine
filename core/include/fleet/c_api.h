/*
 * Stable C ABI of the fleet simulation kernel.
 *
 * The Rust orchestrator mirrors these structs with #[repr(C)]; both sides
 * assert fleet_*_size() at start-up so a layout drift fails loudly instead of
 * silently corrupting results.
 */
#ifndef FLEET_C_API_H
#define FLEET_C_API_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define FLEET_MAX_FAULTS 8
#define FLEET_MAX_HOTSPOTS 4

/* Adversarial fault classes the kernel can inject. */
enum fleet_fault_type {
  FLEET_FAULT_GPS_SPOOF = 0,        /* dispatcher sees displaced vehicle positions   */
  FLEET_FAULT_SENSOR_DROPOUT = 1,   /* vehicles in region freeze for `duration`      */
  FLEET_FAULT_DEMAND_SURGE = 2,     /* extra localised ride requests                 */
  FLEET_FAULT_DISPATCH_LATENCY = 3, /* dispatcher stalls for requests in region      */
  FLEET_FAULT_PHANTOM_REQUESTS = 4  /* spoofed requests that have no rider           */
};

/* A fault is active for ticks [start_tick, start_tick + duration) inside the
 * Manhattan diamond of `radius` cells around (cx, cy). */
typedef struct fleet_fault {
  int32_t type;
  int32_t start_tick;
  int32_t duration;
  int32_t cx;
  int32_t cy;
  int32_t radius;
  double intensity; /* meaning depends on type, see sim.cpp */
} fleet_fault_t;

typedef struct fleet_scenario {
  uint64_t seed;
  int32_t grid;           /* city is grid x grid cells (1 cell = 100 m)  */
  int32_t vehicles;       /* autonomous fleet size                       */
  int32_t ticks;          /* simulated horizon (1 tick = 10 s)           */
  int32_t max_wait_ticks; /* riders abandon an unassigned request after  */
  double base_rate;       /* mean ride requests per tick                 */
  double hotspot_share;   /* fraction of demand drawn near hotspots      */
  int32_t n_hotspots;
  int32_t n_faults;
  int32_t hotspot_x[FLEET_MAX_HOTSPOTS];
  int32_t hotspot_y[FLEET_MAX_HOTSPOTS];
  double hotspot_w[FLEET_MAX_HOTSPOTS];
  fleet_fault_t faults[FLEET_MAX_FAULTS];
} fleet_scenario_t;

typedef struct fleet_metrics {
  uint64_t state_hash;    /* digest of the whole run, used for replay checks */
  uint64_t vehicle_ticks; /* work counter: vehicles * ticks simulated        */
  int64_t requests_total;
  int64_t completed;
  int64_t abandoned;
  int64_t cancelled_phantom;
  int64_t pending_end;
  int64_t in_progress_end;
  int64_t congestion_events;   /* (cell, tick) pairs above cell capacity    */
  int64_t fault_vehicle_ticks; /* vehicle-ticks spent frozen by a fault     */
  int64_t invariant_violations;
  int64_t deadhead_cells;
  int64_t loaded_cells;
  int32_t max_occupancy;
  int32_t wait_p95_ticks;
  double wait_mean_ticks;
  double trip_mean_ticks;
  double utilization;
} fleet_metrics_t;

/* Runs one scenario to completion. Re-entrant and allocation-local, so it can
 * be called concurrently from many threads. Returns 0 on success, <0 when the
 * scenario is rejected as invalid. */
int fleet_run(const fleet_scenario_t* scenario, fleet_metrics_t* out);

size_t fleet_scenario_size(void);
size_t fleet_metrics_size(void);
const char* fleet_version(void);

#ifdef __cplusplus
}
#endif
#endif /* FLEET_C_API_H */
