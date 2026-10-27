// Discrete-time autonomous ride-hailing simulation with adversarial fault injection.
//
// World model
//   * city: grid x grid cells, 1 cell = 100 m, 1 tick = 10 s (a vehicle moves one cell per tick)
//   * demand: Poisson arrivals with a diurnal curve, partly clustered around hotspots
//   * dispatch: greedy nearest-idle-vehicle per request (FIFO), using a bucketed spatial index
//   * faults: GPS spoofing, sensor dropout, demand surge, dispatch latency, phantom requests
//
// Determinism contract: for a given (scenario, binary, CPU architecture) the result is
// bit-identical on every run and on every thread. Demand draws come from one RNG stream that
// faults never touch; every fault decision is a pure hash of (seed, fault, entity), so
// baseline and faulted runs are directly comparable.

#include <algorithm>
#include <climits>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <vector>

#include "fleet/c_api.h"
#include "fleet/rng.hpp"

namespace fleet {
namespace {

constexpr int kBucket = 8;           // spatial index bucket edge, in cells
constexpr int kCellCapacity = 12;    // vehicles per 100 m cell before a congestion event is logged
constexpr int kInvariantStride = 64; // full O(V) consistency scan every N ticks
constexpr double kPi = 3.14159265358979323846;

enum VehicleState : uint8_t { kIdle = 0, kToPickup = 1, kOnTrip = 2, kFault = 3 };

struct Vehicle {
  int16_t x, y;
  int16_t tx, ty;
  uint8_t state;
  uint8_t prev_state;  // state to resume after a sensor dropout
  int32_t req;         // assigned request id, -1 when none
  int32_t fault_until;
};

struct Request {
  int32_t t_created;
  int32_t t_pickup;
  int16_t px, py, dx, dy;
  int32_t vehicle;
  bool phantom;
};

inline uint64_t mix(uint64_t h, uint64_t v) {
  h ^= v + 0x9e3779b97f4a7c15ULL + (h << 6) + (h >> 2);
  h ^= h >> 33;
  h *= 0xff51afd7ed558ccdULL;
  h ^= h >> 33;
  return h;
}

inline int clampi(int v, int lo, int hi) { return v < lo ? lo : (v > hi ? hi : v); }

class Simulation {
 public:
  explicit Simulation(const fleet_scenario_t& sc)
      : sc_(sc),
        demand_(sc.seed),
        fault_rng_(sc.seed ^ 0xfa17fa17fa17fa17ULL),
        veh_(static_cast<size_t>(sc.vehicles)),
        ppx_(static_cast<size_t>(sc.vehicles)),
        ppy_(static_cast<size_t>(sc.vehicles)),
        occ_(static_cast<size_t>(sc.grid) * static_cast<size_t>(sc.grid), 0),
        wait_hist_(static_cast<size_t>(sc.max_wait_ticks) + 2, 0) {
    nb_ = (sc.grid + kBucket - 1) / kBucket;
    bucket_start_.assign(static_cast<size_t>(nb_) * nb_ + 1, 0);
    bucket_items_.reserve(static_cast<size_t>(sc.vehicles));
    pending_.reserve(1024);
    reqs_.reserve(static_cast<size_t>(sc.base_rate * sc.ticks * 1.3) + 64);

    Rng init(sc.seed ^ 0x1217121712171217ULL);
    for (auto& v : veh_) {
      v.x = static_cast<int16_t>(init.below(sc.grid));
      v.y = static_cast<int16_t>(init.below(sc.grid));
      v.tx = v.x;
      v.ty = v.y;
      v.state = kIdle;
      v.prev_state = kIdle;
      v.req = -1;
      v.fault_until = 0;
    }
    hash_ = mix(0x6a09e667f3bcc908ULL, sc.seed);
  }

  void run(fleet_metrics_t* out) {
    for (int t = 0; t < sc_.ticks; ++t) {
      activateFaults(t);
      spawnDemand(t);
      dispatch(t);
      moveVehicles(t);
      if (t % kInvariantStride == 0) scanInvariants();
      hash_ = mix(hash_, (static_cast<uint64_t>(completed_) << 40) ^
                             (static_cast<uint64_t>(pending_.size()) << 20) ^ static_cast<uint64_t>(assigned_));
    }
    scanInvariants();
    finish(out);
  }

 private:
  const fleet_scenario_t& sc_;
  Rng demand_;
  Rng fault_rng_;
  std::vector<Vehicle> veh_;
  std::vector<int16_t> ppx_, ppy_;  // positions as perceived by the dispatcher
  std::vector<uint8_t> occ_;
  std::vector<Request> reqs_;
  std::vector<int32_t> pending_;
  std::vector<int32_t> bucket_start_, bucket_items_;
  std::vector<int64_t> wait_hist_;
  int nb_ = 0;

  int64_t completed_ = 0, abandoned_ = 0, phantom_ = 0, assigned_ = 0;
  int64_t congestion_ = 0, fault_vt_ = 0, violations_ = 0;
  int64_t deadhead_ = 0, loaded_ = 0, busy_vt_ = 0;
  int64_t wait_sum_ = 0, wait_n_ = 0, trip_sum_ = 0;
  int32_t max_occ_ = 0;
  uint64_t hash_ = 0;

  bool active(const fleet_fault_t& f, int t) const { return t >= f.start_tick && t < f.start_tick + f.duration; }
  static bool inRegion(const fleet_fault_t& f, int x, int y) {
    return std::abs(x - f.cx) + std::abs(y - f.cy) <= f.radius;
  }

  // ---- demand -----------------------------------------------------------------------------
  void addRequest(int t, int px, int py, int dx, int dy, bool phantom) {
    Request r;
    r.t_created = t;
    r.t_pickup = -1;
    r.px = static_cast<int16_t>(px);
    r.py = static_cast<int16_t>(py);
    r.dx = static_cast<int16_t>(dx);
    r.dy = static_cast<int16_t>(dy);
    r.vehicle = -1;
    r.phantom = phantom;
    pending_.push_back(static_cast<int32_t>(reqs_.size()));
    reqs_.push_back(r);
  }

  void sampleDropoff(Rng& rng, int px, int py, int* dx, int* dy) const {
    const int reach = std::max(4, sc_.grid / 3);
    for (int attempt = 0; attempt < 8; ++attempt) {
      *dx = clampi(px + rng.below(2 * reach + 1) - reach, 0, sc_.grid - 1);
      *dy = clampi(py + rng.below(2 * reach + 1) - reach, 0, sc_.grid - 1);
      if (std::abs(*dx - px) + std::abs(*dy - py) >= 5) return;
    }
  }

  void samplePickup(int* px, int* py) {
    if (sc_.n_hotspots > 0 && demand_.uniform() < sc_.hotspot_share) {
      double total = 0.0;
      for (int i = 0; i < sc_.n_hotspots; ++i) total += sc_.hotspot_w[i];
      double pick = demand_.uniform() * total;
      int h = 0;
      for (; h < sc_.n_hotspots - 1; ++h) {
        if (pick < sc_.hotspot_w[h]) break;
        pick -= sc_.hotspot_w[h];
      }
      const double spread = sc_.grid / 8.0;
      const double ox = (demand_.uniform() + demand_.uniform() + demand_.uniform() - 1.5) * spread;
      const double oy = (demand_.uniform() + demand_.uniform() + demand_.uniform() - 1.5) * spread;
      *px = clampi(sc_.hotspot_x[h] + static_cast<int>(std::lround(ox)), 0, sc_.grid - 1);
      *py = clampi(sc_.hotspot_y[h] + static_cast<int>(std::lround(oy)), 0, sc_.grid - 1);
    } else {
      *px = demand_.below(sc_.grid);
      *py = demand_.below(sc_.grid);
    }
  }

  void sampleInRegion(const fleet_fault_t& f, int* px, int* py) {
    const int dx = fault_rng_.below(2 * f.radius + 1) - f.radius;
    const int span = f.radius - std::abs(dx);
    const int dy = fault_rng_.below(2 * span + 1) - span;
    *px = clampi(f.cx + dx, 0, sc_.grid - 1);
    *py = clampi(f.cy + dy, 0, sc_.grid - 1);
  }

  void spawnDemand(int t) {
    const double diurnal = 1.0 + 0.35 * std::sin(2.0 * kPi * static_cast<double>(t) / sc_.ticks);
    const int n = demand_.poisson(sc_.base_rate * diurnal);
    for (int i = 0; i < n; ++i) {
      int px, py, dx = 0, dy = 0;
      samplePickup(&px, &py);
      sampleDropoff(demand_, px, py, &dx, &dy);
      addRequest(t, px, py, dx, dy, false);
    }
    for (int i = 0; i < sc_.n_faults; ++i) {
      const fleet_fault_t& f = sc_.faults[i];
      if (!active(f, t)) continue;
      if (f.type != FLEET_FAULT_DEMAND_SURGE && f.type != FLEET_FAULT_PHANTOM_REQUESTS) continue;
      const int extra = fault_rng_.poisson(sc_.base_rate * f.intensity);
      for (int k = 0; k < extra; ++k) {
        int px, py, dx = 0, dy = 0;
        sampleInRegion(f, &px, &py);
        sampleDropoff(fault_rng_, px, py, &dx, &dy);
        addRequest(t, px, py, dx, dy, f.type == FLEET_FAULT_PHANTOM_REQUESTS);
      }
    }
  }

  // ---- faults -----------------------------------------------------------------------------
  void activateFaults(int t) {
    for (int i = 0; i < sc_.n_faults; ++i) {
      const fleet_fault_t& f = sc_.faults[i];
      if (f.type != FLEET_FAULT_SENSOR_DROPOUT || t != f.start_tick) continue;
      for (int id = 0; id < sc_.vehicles; ++id) {
        Vehicle& v = veh_[static_cast<size_t>(id)];
        if (v.state == kFault || !inRegion(f, v.x, v.y)) continue;
        if (hash01(sc_.seed, 2000 + static_cast<uint64_t>(i), static_cast<uint64_t>(id)) >= f.intensity) continue;
        v.prev_state = v.state;
        v.state = kFault;
        v.fault_until = f.start_tick + f.duration;
      }
    }
  }

  void perceivedPosition(int id, const Vehicle& v, int t, int* px, int* py) const {
    int x = v.x, y = v.y;
    for (int i = 0; i < sc_.n_faults; ++i) {
      const fleet_fault_t& f = sc_.faults[i];
      if (f.type != FLEET_FAULT_GPS_SPOOF || !active(f, t) || !inRegion(f, v.x, v.y)) continue;
      if (hash01(sc_.seed, 1000 + static_cast<uint64_t>(i), static_cast<uint64_t>(id)) >= f.intensity) continue;
      uint64_t s = sc_.seed ^ (static_cast<uint64_t>(id) << 20) ^ static_cast<uint64_t>(i);
      const int mag = 8 + static_cast<int>(24.0 * f.intensity);
      x += static_cast<int>(splitmix64(s) % static_cast<uint64_t>(2 * mag + 1)) - mag;
      y += static_cast<int>(splitmix64(s) % static_cast<uint64_t>(2 * mag + 1)) - mag;
      break;
    }
    *px = clampi(x, 0, sc_.grid - 1);
    *py = clampi(y, 0, sc_.grid - 1);
  }

  bool latencyBlocked(const Request& r, int req_id, int t) const {
    for (int i = 0; i < sc_.n_faults; ++i) {
      const fleet_fault_t& f = sc_.faults[i];
      if (f.type != FLEET_FAULT_DISPATCH_LATENCY || !active(f, t) || !inRegion(f, r.px, r.py)) continue;
      if (hash01(sc_.seed, 3000 + static_cast<uint64_t>(t), static_cast<uint64_t>(req_id)) < f.intensity) return true;
    }
    return false;
  }

  // ---- dispatch ---------------------------------------------------------------------------
  int buildIndex(int t) {
    std::fill(bucket_start_.begin(), bucket_start_.end(), 0);
    int idle = 0;
    for (int id = 0; id < sc_.vehicles; ++id) {
      const Vehicle& v = veh_[static_cast<size_t>(id)];
      if (v.state != kIdle) continue;
      int px, py;
      perceivedPosition(id, v, t, &px, &py);
      ppx_[static_cast<size_t>(id)] = static_cast<int16_t>(px);
      ppy_[static_cast<size_t>(id)] = static_cast<int16_t>(py);
      ++bucket_start_[static_cast<size_t>((py / kBucket) * nb_ + (px / kBucket)) + 1];
      ++idle;
    }
    for (size_t b = 1; b < bucket_start_.size(); ++b) bucket_start_[b] += bucket_start_[b - 1];
    bucket_items_.assign(static_cast<size_t>(idle), 0);
    std::vector<int32_t>& cursor = cursor_;
    cursor.assign(bucket_start_.begin(), bucket_start_.end() - 1);
    for (int id = 0; id < sc_.vehicles; ++id) {
      if (veh_[static_cast<size_t>(id)].state != kIdle) continue;
      const size_t b = static_cast<size_t>((ppy_[static_cast<size_t>(id)] / kBucket) * nb_ + ppx_[static_cast<size_t>(id)] / kBucket);
      bucket_items_[static_cast<size_t>(cursor[b]++)] = id;
    }
    return idle;
  }

  int findNearest(int qx, int qy) const {
    const int bx = qx / kBucket, by = qy / kBucket;
    int best = -1, best_d = INT_MAX;
    for (int k = 0; k <= nb_; ++k) {
      const int x0 = bx - k, x1 = bx + k, y0 = by - k, y1 = by + k;
      for (int cy = std::max(y0, 0); cy <= std::min(y1, nb_ - 1); ++cy) {
        const bool edge_row = (cy == y0 || cy == y1);
        const int step = edge_row ? 1 : (x1 - x0);
        for (int cx = x0; cx <= x1; cx += step) {
          if (cx < 0 || cx >= nb_) continue;
          const size_t b = static_cast<size_t>(cy * nb_ + cx);
          for (int32_t i = bucket_start_[b]; i < bucket_start_[b + 1]; ++i) {
            const int id = bucket_items_[static_cast<size_t>(i)];
            if (veh_[static_cast<size_t>(id)].state != kIdle) continue;
            const int d = std::abs(ppx_[static_cast<size_t>(id)] - qx) + std::abs(ppy_[static_cast<size_t>(id)] - qy);
            if (d < best_d || (d == best_d && id < best)) {
              best_d = d;
              best = id;
            }
          }
        }
      }
      // Anything in an unscanned ring is at least k*kBucket + 1 cells away.
      if (best >= 0 && best_d <= k * kBucket) break;
    }
    return best;
  }

  void dispatch(int t) {
    int idle_left = buildIndex(t);
    size_t keep = 0;
    for (size_t i = 0; i < pending_.size(); ++i) {
      const int32_t rid = pending_[i];
      Request& r = reqs_[static_cast<size_t>(rid)];
      if (t - r.t_created > sc_.max_wait_ticks) {
        ++abandoned_;
        continue;
      }
      bool assigned = false;
      if (idle_left > 0 && !latencyBlocked(r, rid, t)) {
        const int id = findNearest(r.px, r.py);
        if (id >= 0) {
          Vehicle& v = veh_[static_cast<size_t>(id)];
          v.state = kToPickup;
          v.req = rid;
          v.tx = r.px;
          v.ty = r.py;
          r.vehicle = id;
          ++assigned_;
          --idle_left;
          assigned = true;
        }
      }
      if (!assigned) pending_[keep++] = rid;
    }
    pending_.resize(keep);
  }

  // ---- vehicle dynamics -------------------------------------------------------------------
  void moveVehicles(int t) {
    for (int id = 0; id < sc_.vehicles; ++id) {
      Vehicle& v = veh_[static_cast<size_t>(id)];

      if (v.state == kFault) {
        if (t >= v.fault_until) {
          v.state = v.prev_state;
        } else {
          ++fault_vt_;
          if (v.prev_state == kToPickup && t - reqs_[static_cast<size_t>(v.req)].t_created > sc_.max_wait_ticks) {
            // The rider gives up on a frozen vehicle; the request is released.
            ++abandoned_;
            --assigned_;
            reqs_[static_cast<size_t>(v.req)].vehicle = -1;
            v.req = -1;
            v.prev_state = kIdle;
          }
          countOccupancy(v);
          continue;
        }
      }

      if (v.state == kToPickup || v.state == kOnTrip) {
        ++busy_vt_;
        if (v.x != v.tx) {
          v.x = static_cast<int16_t>(v.x + (v.tx > v.x ? 1 : -1));
          (v.state == kToPickup ? deadhead_ : loaded_)++;
        } else if (v.y != v.ty) {
          v.y = static_cast<int16_t>(v.y + (v.ty > v.y ? 1 : -1));
          (v.state == kToPickup ? deadhead_ : loaded_)++;
        }
        if (v.x == v.tx && v.y == v.ty) arrive(v, t);
      }
      if (v.x < 0 || v.y < 0 || v.x >= sc_.grid || v.y >= sc_.grid) ++violations_;
      if (v.state != kIdle) countOccupancy(v);  // parked vehicles are not road congestion
    }
    for (const Vehicle& v : veh_) occ_[static_cast<size_t>(v.y) * static_cast<size_t>(sc_.grid) + static_cast<size_t>(v.x)] = 0;
  }

  void countOccupancy(const Vehicle& v) {
    const size_t cell = static_cast<size_t>(v.y) * static_cast<size_t>(sc_.grid) + static_cast<size_t>(v.x);
    const int n = ++occ_[cell];
    if (n > max_occ_) max_occ_ = n;
    if (n == kCellCapacity + 1) ++congestion_;
  }

  void arrive(Vehicle& v, int t) {
    Request& r = reqs_[static_cast<size_t>(v.req)];
    if (v.state == kToPickup) {
      if (r.phantom) {
        ++phantom_;
        --assigned_;
        v.state = kIdle;
        v.req = -1;
        return;
      }
      const int wait = t - r.t_created;
      wait_sum_ += wait;
      ++wait_n_;
      ++wait_hist_[static_cast<size_t>(std::min(wait, sc_.max_wait_ticks + 1))];
      r.t_pickup = t;
      v.state = kOnTrip;
      v.tx = r.dx;
      v.ty = r.dy;
      // A drop-off equal to the pickup would complete next tick; handled by the next move step.
    } else {
      ++completed_;
      --assigned_;
      trip_sum_ += t - r.t_pickup;
      v.state = kIdle;
      v.req = -1;
    }
  }

  // ---- verification & output --------------------------------------------------------------
  void scanInvariants() {
    int64_t with_req = 0;
    for (int id = 0; id < sc_.vehicles; ++id) {
      const Vehicle& v = veh_[static_cast<size_t>(id)];
      if (v.req >= 0) {
        ++with_req;
        if (reqs_[static_cast<size_t>(v.req)].vehicle != id) ++violations_;
      }
      if (v.state == kIdle && v.req >= 0) ++violations_;
    }
    if (with_req != assigned_) ++violations_;
    const int64_t total = static_cast<int64_t>(reqs_.size());
    if (total != completed_ + abandoned_ + phantom_ + static_cast<int64_t>(pending_.size()) + assigned_) ++violations_;
  }

  void finish(fleet_metrics_t* out) {
    int32_t p95 = 0;
    if (wait_n_ > 0) {
      const int64_t target = (wait_n_ * 95 + 99) / 100;
      int64_t cum = 0;
      for (size_t w = 0; w < wait_hist_.size(); ++w) {
        cum += wait_hist_[w];
        if (cum >= target) {
          p95 = static_cast<int32_t>(w);
          break;
        }
      }
    }
    for (const Vehicle& v : veh_) {
      hash_ = mix(hash_, (static_cast<uint64_t>(static_cast<uint16_t>(v.x)) << 48) ^
                             (static_cast<uint64_t>(static_cast<uint16_t>(v.y)) << 32) ^
                             (static_cast<uint64_t>(v.state) << 24) ^ static_cast<uint64_t>(static_cast<uint32_t>(v.req)));
    }
    hash_ = mix(hash_, static_cast<uint64_t>(wait_sum_));
    hash_ = mix(hash_, static_cast<uint64_t>(abandoned_));

    int64_t in_progress = 0;
    for (const Vehicle& v : veh_) in_progress += v.req >= 0 ? 1 : 0;

    out->state_hash = hash_;
    out->vehicle_ticks = static_cast<uint64_t>(sc_.vehicles) * static_cast<uint64_t>(sc_.ticks);
    out->requests_total = static_cast<int64_t>(reqs_.size());
    out->completed = completed_;
    out->abandoned = abandoned_;
    out->cancelled_phantom = phantom_;
    out->pending_end = static_cast<int64_t>(pending_.size());
    out->in_progress_end = in_progress;
    out->congestion_events = congestion_;
    out->fault_vehicle_ticks = fault_vt_;
    out->invariant_violations = violations_;
    out->deadhead_cells = deadhead_;
    out->loaded_cells = loaded_;
    out->max_occupancy = max_occ_;
    out->wait_p95_ticks = p95;
    out->wait_mean_ticks = wait_n_ ? static_cast<double>(wait_sum_) / static_cast<double>(wait_n_) : 0.0;
    out->trip_mean_ticks = completed_ ? static_cast<double>(trip_sum_) / static_cast<double>(completed_) : 0.0;
    out->utilization = static_cast<double>(busy_vt_) / static_cast<double>(out->vehicle_ticks);
  }

  std::vector<int32_t> cursor_;
};

bool valid(const fleet_scenario_t& s) {
  if (s.grid < 8 || s.grid > 1024) return false;
  if (s.vehicles < 1 || s.vehicles > 200000) return false;
  if (s.ticks < 1 || s.ticks > 1000000) return false;
  if (s.max_wait_ticks < 1 || s.max_wait_ticks > 100000) return false;
  if (!(s.base_rate >= 0.0) || s.base_rate > 500.0) return false;
  if (s.n_hotspots < 0 || s.n_hotspots > FLEET_MAX_HOTSPOTS) return false;
  if (s.n_faults < 0 || s.n_faults > FLEET_MAX_FAULTS) return false;
  for (int i = 0; i < s.n_hotspots; ++i) {
    if (s.hotspot_x[i] < 0 || s.hotspot_x[i] >= s.grid || s.hotspot_y[i] < 0 || s.hotspot_y[i] >= s.grid) return false;
    if (!(s.hotspot_w[i] > 0.0)) return false;
  }
  for (int i = 0; i < s.n_faults; ++i) {
    const fleet_fault_t& f = s.faults[i];
    if (f.type < FLEET_FAULT_GPS_SPOOF || f.type > FLEET_FAULT_PHANTOM_REQUESTS) return false;
    if (f.start_tick < 0 || f.duration < 1 || f.radius < 1) return false;
    if (!(f.intensity >= 0.0) || f.intensity > 50.0) return false;
  }
  return true;
}

}  // namespace
}  // namespace fleet

extern "C" {

int fleet_run(const fleet_scenario_t* scenario, fleet_metrics_t* out) {
  if (scenario == nullptr || out == nullptr || !fleet::valid(*scenario)) return -1;
  fleet::Simulation sim(*scenario);
  sim.run(out);
  return 0;
}

size_t fleet_scenario_size(void) { return sizeof(fleet_scenario_t); }
size_t fleet_metrics_size(void) { return sizeof(fleet_metrics_t); }
const char* fleet_version(void) { return "fleet-core 0.1.0"; }

}  // extern "C"
