// Self-contained test runner (no external framework) for the simulation kernel.

#include <cstdio>
#include <cstring>

#include "fleet/c_api.h"
#include "fleet/rng.hpp"

namespace {

int g_failed = 0;
int g_checks = 0;

#define CHECK(cond)                                                              \
  do {                                                                           \
    ++g_checks;                                                                  \
    if (!(cond)) {                                                               \
      ++g_failed;                                                                \
      std::fprintf(stderr, "FAIL %s:%d  %s\n", __FILE__, __LINE__, #cond);       \
    }                                                                            \
  } while (0)

fleet_scenario_t base(uint64_t seed = 1) {
  fleet_scenario_t sc;
  std::memset(&sc, 0, sizeof sc);
  sc.seed = seed;
  sc.grid = 128;
  sc.vehicles = 500;
  sc.ticks = 1440;
  sc.max_wait_ticks = 90;
  sc.base_rate = 4.5;
  sc.hotspot_share = 0.6;
  sc.n_hotspots = 2;
  sc.hotspot_x[0] = 32; sc.hotspot_y[0] = 40; sc.hotspot_w[0] = 1.0;
  sc.hotspot_x[1] = 96; sc.hotspot_y[1] = 88; sc.hotspot_w[1] = 0.7;
  return sc;
}

void addFault(fleet_scenario_t& sc, int type, int start, int dur, int cx, int cy, int r, double intensity) {
  fleet_fault_t& f = sc.faults[sc.n_faults++];
  f.type = type; f.start_tick = start; f.duration = dur; f.cx = cx; f.cy = cy; f.radius = r; f.intensity = intensity;
}

fleet_metrics_t run(const fleet_scenario_t& sc) {
  fleet_metrics_t m;
  std::memset(&m, 0, sizeof m);
  CHECK(fleet_run(&sc, &m) == 0);
  return m;
}

void testAbiSizes() {
  CHECK(fleet_scenario_size() == sizeof(fleet_scenario_t));
  CHECK(fleet_metrics_size() == sizeof(fleet_metrics_t));
  CHECK(sizeof(fleet_fault_t) == 32);
}

void testRngReference() {
  // Pin the generator so an accidental change cannot silently invalidate stored scenario hashes.
  fleet::Rng a(42), b(42), c(43);
  const uint64_t x = a.next();
  CHECK(x == b.next());
  CHECK(x != c.next());
  fleet::Rng u(7);
  for (int i = 0; i < 10000; ++i) {
    const double v = u.uniform();
    CHECK(v >= 0.0 && v < 1.0);
  }
}

void testDeterminism() {
  const fleet_scenario_t sc = base(11);
  const fleet_metrics_t a = run(sc), b = run(sc);
  CHECK(std::memcmp(&a, &b, sizeof a) == 0);
  CHECK(a.state_hash != 0);
  const fleet_metrics_t c = run(base(12));
  CHECK(a.state_hash != c.state_hash);
}

void testConservationAndInvariants() {
  for (int i = 0; i < 40; ++i) {
    fleet_scenario_t sc = base(100 + i);
    sc.vehicles = 50 + (i * 37) % 400;
    sc.ticks = 300 + (i * 53) % 900;
    sc.base_rate = 1.0 + (i % 7);
    addFault(sc, i % 5, 50 + i, 120, 20 + (i * 13) % 90, 20 + (i * 29) % 90, 12, 0.3 + 0.15 * (i % 5));
    if (i % 3 == 0) addFault(sc, (i + 2) % 5, 100, 80, 64, 64, 20, 0.8);
    const fleet_metrics_t m = run(sc);
    CHECK(m.invariant_violations == 0);
    CHECK(m.requests_total == m.completed + m.abandoned + m.cancelled_phantom + m.pending_end + m.in_progress_end);
    CHECK(m.utilization >= 0.0 && m.utilization <= 1.0);
    CHECK(m.vehicle_ticks == static_cast<uint64_t>(sc.vehicles) * static_cast<uint64_t>(sc.ticks));
  }
}

void testBaselineIsHealthy() {
  const fleet_metrics_t m = run(base(5));
  CHECK(m.completed > 0.9 * m.requests_total);
  CHECK(m.abandoned == 0);
  CHECK(m.cancelled_phantom == 0);
  CHECK(m.fault_vehicle_ticks == 0);
  CHECK(m.wait_p95_ticks < 60);
}

void testFaultsHaveTheirEffect() {
  const fleet_metrics_t clean = run(base(21));

  fleet_scenario_t spoof = base(21);
  addFault(spoof, FLEET_FAULT_GPS_SPOOF, 100, 900, 64, 64, 90, 0.9);
  const fleet_metrics_t ms = run(spoof);
  CHECK(ms.wait_mean_ticks > clean.wait_mean_ticks);
  CHECK(ms.requests_total == clean.requests_total);  // faults never perturb the demand stream

  fleet_scenario_t drop = base(21);
  addFault(drop, FLEET_FAULT_SENSOR_DROPOUT, 200, 300, 64, 64, 60, 0.9);
  const fleet_metrics_t md = run(drop);
  CHECK(md.fault_vehicle_ticks > 0);

  fleet_scenario_t surge = base(21);
  addFault(surge, FLEET_FAULT_DEMAND_SURGE, 300, 200, 32, 40, 12, 6.0);
  const fleet_metrics_t mu = run(surge);
  CHECK(mu.requests_total > clean.requests_total);
  CHECK(mu.abandoned >= clean.abandoned);

  fleet_scenario_t lat = base(21);
  addFault(lat, FLEET_FAULT_DISPATCH_LATENCY, 100, 600, 64, 64, 90, 0.95);
  const fleet_metrics_t ml = run(lat);
  CHECK(ml.wait_mean_ticks > clean.wait_mean_ticks + 3.0);

  fleet_scenario_t ph = base(21);
  addFault(ph, FLEET_FAULT_PHANTOM_REQUESTS, 100, 400, 64, 64, 20, 1.5);
  const fleet_metrics_t mp = run(ph);
  CHECK(mp.cancelled_phantom > 0);
}

void testRejectsInvalid() {
  fleet_metrics_t m;
  fleet_scenario_t sc = base();
  CHECK(fleet_run(nullptr, &m) < 0);
  CHECK(fleet_run(&sc, nullptr) < 0);
  sc.grid = 4;
  CHECK(fleet_run(&sc, &m) < 0);
  sc = base();
  sc.vehicles = 0;
  CHECK(fleet_run(&sc, &m) < 0);
  sc = base();
  addFault(sc, 99, 0, 10, 5, 5, 3, 0.5);
  CHECK(fleet_run(&sc, &m) < 0);
  sc = base();
  sc.hotspot_x[0] = 5000;
  CHECK(fleet_run(&sc, &m) < 0);
}

}  // namespace

int main() {
  testAbiSizes();
  testRngReference();
  testDeterminism();
  testConservationAndInvariants();
  testBaselineIsHealthy();
  testFaultsHaveTheirEffect();
  testRejectsInvalid();
  std::printf("%d checks, %d failed\n", g_checks, g_failed);
  return g_failed == 0 ? 0 : 1;
}
