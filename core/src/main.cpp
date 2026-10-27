// Standalone CLI for the simulation kernel: run one scenario and print metrics as JSON.
//
//   fleetsim --seed 7 --vehicles 400 --ticks 1440 \
//            --fault gps_spoof:300:200:64:64:20:0.5 --fault demand_surge:600:120:30:30:10:4
//
// fault spec: type:start_tick:duration:cx:cy:radius:intensity

#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <string>

#include "fleet/c_api.h"

namespace {

int faultType(const std::string& name) {
  if (name == "gps_spoof") return FLEET_FAULT_GPS_SPOOF;
  if (name == "sensor_dropout") return FLEET_FAULT_SENSOR_DROPOUT;
  if (name == "demand_surge") return FLEET_FAULT_DEMAND_SURGE;
  if (name == "dispatch_latency") return FLEET_FAULT_DISPATCH_LATENCY;
  if (name == "phantom_requests") return FLEET_FAULT_PHANTOM_REQUESTS;
  return -1;
}

bool parseFault(const char* spec, fleet_fault_t* f) {
  char name[32];
  int start, dur, cx, cy, radius;
  double intensity;
  if (std::sscanf(spec, "%31[^:]:%d:%d:%d:%d:%d:%lf", name, &start, &dur, &cx, &cy, &radius, &intensity) != 7) return false;
  const int type = faultType(name);
  if (type < 0) return false;
  f->type = type;
  f->start_tick = start;
  f->duration = dur;
  f->cx = cx;
  f->cy = cy;
  f->radius = radius;
  f->intensity = intensity;
  return true;
}

void usage() {
  std::fprintf(stderr,
               "usage: fleetsim [--seed N] [--grid N] [--vehicles N] [--ticks N] [--rate R] [--max-wait N]\n"
               "                [--fault type:start:duration:cx:cy:radius:intensity]...\n"
               "fault types: gps_spoof sensor_dropout demand_surge dispatch_latency phantom_requests\n");
}

}  // namespace

int main(int argc, char** argv) {
  fleet_scenario_t sc;
  std::memset(&sc, 0, sizeof sc);
  sc.seed = 1;
  sc.grid = 128;
  sc.vehicles = 500;
  sc.ticks = 1440;
  sc.max_wait_ticks = 90;
  sc.base_rate = 4.5;
  sc.hotspot_share = 0.6;
  sc.n_hotspots = 3;
  const int hx[3] = {32, 96, 64}, hy[3] = {40, 88, 64};
  const double hw[3] = {1.0, 0.8, 0.6};
  for (int i = 0; i < 3; ++i) {
    sc.hotspot_x[i] = hx[i];
    sc.hotspot_y[i] = hy[i];
    sc.hotspot_w[i] = hw[i];
  }

  for (int i = 1; i < argc; ++i) {
    const std::string a = argv[i];
    auto next = [&]() -> const char* { return i + 1 < argc ? argv[++i] : nullptr; };
    const char* v = nullptr;
    if (a == "--help" || a == "-h") { usage(); return 0; }
    if (!(v = next())) { usage(); return 2; }
    if (a == "--seed") sc.seed = std::strtoull(v, nullptr, 10);
    else if (a == "--grid") sc.grid = std::atoi(v);
    else if (a == "--vehicles") sc.vehicles = std::atoi(v);
    else if (a == "--ticks") sc.ticks = std::atoi(v);
    else if (a == "--rate") sc.base_rate = std::atof(v);
    else if (a == "--max-wait") sc.max_wait_ticks = std::atoi(v);
    else if (a == "--fault") {
      if (sc.n_faults >= FLEET_MAX_FAULTS || !parseFault(v, &sc.faults[sc.n_faults])) {
        std::fprintf(stderr, "bad --fault spec: %s\n", v);
        return 2;
      }
      ++sc.n_faults;
    } else { usage(); return 2; }
  }

  fleet_metrics_t m;
  if (fleet_run(&sc, &m) != 0) {
    std::fprintf(stderr, "scenario rejected as invalid\n");
    return 1;
  }
  std::printf(
      "{\"state_hash\":\"%016llx\",\"requests_total\":%lld,\"completed\":%lld,\"abandoned\":%lld,"
      "\"cancelled_phantom\":%lld,\"pending_end\":%lld,\"in_progress_end\":%lld,\"wait_mean_ticks\":%.4f,"
      "\"wait_p95_ticks\":%d,\"trip_mean_ticks\":%.4f,\"utilization\":%.4f,\"congestion_events\":%lld,"
      "\"max_occupancy\":%d,\"fault_vehicle_ticks\":%lld,\"deadhead_cells\":%lld,\"loaded_cells\":%lld,"
      "\"invariant_violations\":%lld}\n",
      static_cast<unsigned long long>(m.state_hash), static_cast<long long>(m.requests_total),
      static_cast<long long>(m.completed), static_cast<long long>(m.abandoned),
      static_cast<long long>(m.cancelled_phantom), static_cast<long long>(m.pending_end),
      static_cast<long long>(m.in_progress_end), m.wait_mean_ticks, m.wait_p95_ticks, m.trip_mean_ticks,
      m.utilization, static_cast<long long>(m.congestion_events), m.max_occupancy,
      static_cast<long long>(m.fault_vehicle_ticks), static_cast<long long>(m.deadhead_cells),
      static_cast<long long>(m.loaded_cells), static_cast<long long>(m.invariant_violations));
  return 0;
}
