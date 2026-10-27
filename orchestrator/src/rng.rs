//! Small deterministic PRNG (SplitMix64 seeding + xoshiro256**) for the telemetry generator.

pub fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// Order-independent uniform draw in [0, 1) keyed by three integers.
pub fn hash01(seed: u64, a: u64, b: u64) -> f64 {
    let mut s = seed ^ a.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ b.wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    (splitmix64(&mut s) >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
}

pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Self {
        let mut st = seed;
        Rng { s: [splitmix64(&mut st), splitmix64(&mut st), splitmix64(&mut st), splitmix64(&mut st)] }
    }

    pub fn next_u64(&mut self) -> u64 {
        let result = self.s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.s[1] << 17;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(45);
        result
    }

    pub fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// Uniform integer in [0, n).
    pub fn below(&mut self, n: u32) -> u32 {
        (((self.next_u64() >> 32) * n as u64) >> 32) as u32
    }

    /// Uniform integer in [lo, hi].
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u32) as i32
    }

    pub fn gaussian(&mut self) -> f64 {
        let u1 = self.uniform().max(1e-12);
        let u2 = self.uniform();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()
    }

    pub fn poisson(&mut self, lambda: f64) -> u32 {
        if lambda <= 0.0 {
            return 0;
        }
        let limit = (-lambda.min(500.0)).exp();
        let mut p = 1.0;
        let mut k = 0u32;
        loop {
            k += 1;
            p *= self.uniform();
            if p <= limit {
                return k - 1;
            }
        }
    }

    /// Number of failures before the first success, success probability `1 - stall`.
    pub fn geometric_stalls(&mut self, stall: f64) -> u32 {
        let mut n = 0;
        while self.uniform() < stall && n < 1000 {
            n += 1;
        }
        n
    }
}
