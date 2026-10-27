// Deterministic PRNG: SplitMix64 seeding + xoshiro256**.
// No <random> distributions are used because their output is not specified
// across standard libraries, which would break cross-platform replay.
#pragma once

#include <cmath>
#include <cstdint>

namespace fleet {

inline uint64_t splitmix64(uint64_t& state) {
  uint64_t z = (state += 0x9e3779b97f4a7c15ULL);
  z = (z ^ (z >> 30)) * 0xbf58476d1ce4e5b9ULL;
  z = (z ^ (z >> 27)) * 0x94d049bb133111ebULL;
  return z ^ (z >> 31);
}

class Rng {
 public:
  explicit Rng(uint64_t seed) {
    for (auto& word : s_) word = splitmix64(seed);
  }

  uint64_t next() {
    const uint64_t result = rotl(s_[1] * 5, 7) * 9;
    const uint64_t t = s_[1] << 17;
    s_[2] ^= s_[0];
    s_[3] ^= s_[1];
    s_[1] ^= s_[2];
    s_[0] ^= s_[3];
    s_[2] ^= t;
    s_[3] = rotl(s_[3], 45);
    return result;
  }

  // Uniform in [0, 1) with 53 bits of entropy.
  double uniform() { return static_cast<double>(next() >> 11) * 0x1.0p-53; }

  // Uniform integer in [0, n).
  int32_t below(int32_t n) {
    return static_cast<int32_t>((static_cast<uint64_t>(next() >> 32) * static_cast<uint64_t>(n)) >> 32);
  }

  // Knuth's method; adequate for the small per-tick rates used here.
  int32_t poisson(double lambda) {
    if (lambda <= 0.0) return 0;
    if (lambda > 500.0) lambda = 500.0;  // exp(-lambda) would underflow; real scenarios stay far below this
    const double limit = std::exp(-lambda);
    double p = 1.0;
    int32_t k = 0;
    do {
      ++k;
      p *= uniform();
    } while (p > limit);
    return k - 1;
  }

 private:
  static uint64_t rotl(uint64_t x, int k) { return (x << k) | (x >> (64 - k)); }
  uint64_t s_[4];
};

// Order-independent uniform draw keyed by (seed, a, b). Fault decisions use
// this instead of a shared stream so they never perturb the demand stream.
inline double hash01(uint64_t seed, uint64_t a, uint64_t b) {
  uint64_t s = seed ^ (a * 0x9e3779b97f4a7c15ULL) ^ (b * 0xc2b2ae3d27d4eb4fULL);
  return static_cast<double>(splitmix64(s) >> 11) * 0x1.0p-53;
}

}  // namespace fleet
