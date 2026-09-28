//! Exact replay of the seeded offset path.
//!
//! `EstimateZombieAttackAction` calls `mt_srand($estimation->getSeed())` before shrinking the
//! offsets, and PHP keeps only the low 32 bits of that seed. The whole path is therefore one of
//! 2^32 deterministic sequences: replaying PHP's MT19937 `mt_rand` for every seed and keeping those
//! consistent with the readings pins the offsets exactly, hence `tmin`/`tmax` and the attack.

use crate::engine::{EstimConf, MAX_ROUNDS};
use crate::inference::RawObservation;
use rayon::prelude::*;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

const N: usize = 624;
const M: usize = 397;
/// Outputs computable without a full reload: output `j` only needs original words `j`, `j+1`, `j+M`.
const LAZY_OUTPUTS: usize = N - M;

/// PHP's `mt_rand` engine (MT19937, `MT_RAND_MT19937` mode), lazily initialised so that rejecting
/// a seed after a few draws only costs the few hundred state words those draws depend on.
pub struct PhpMt {
    seed: u32,
    state: [u32; N],
    initialised: usize,
    pos: usize,
    full: Option<Box<rand_mt::Mt>>,
}

fn temper(mut y: u32) -> u32 {
    y ^= y >> 11;
    y ^= (y << 7) & 0x9d2c_5680;
    y ^= (y << 15) & 0xefc6_0000;
    y ^ (y >> 18)
}

impl PhpMt {
    pub fn new(seed: u32) -> Self {
        let mut mt = PhpMt {
            seed,
            state: [0; N],
            initialised: 0,
            pos: 0,
            full: None,
        };
        mt.reseed(seed);
        mt
    }

    /// `mt_srand($seed)`: PHP truncates the integer seed to 32 bits.
    pub fn reseed(&mut self, seed: u32) {
        // Replaying the same seed (next initial pair) keeps the words already derived.
        if seed != self.seed || self.initialised == 0 {
            self.seed = seed;
            self.state[0] = seed;
            self.initialised = 1;
        }
        self.pos = 0;
        self.full = None;
    }

    fn ensure_initialised(&mut self, upto: usize) {
        while self.initialised <= upto {
            let i = self.initialised;
            let prev = self.state[i - 1];
            self.state[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
            self.initialised += 1;
        }
    }

    pub fn next_u32(&mut self) -> u32 {
        let j = self.pos;
        self.pos += 1;
        if j < LAZY_OUTPUTS {
            self.ensure_initialised(j + M);
            let (u, v) = (self.state[j], self.state[j + 1]);
            let mixed = (u & 0x8000_0000) | (v & 0x7fff_ffff);
            let mag = if v & 1 != 0 { 0x9908_b0df } else { 0 };
            temper(self.state[j + M] ^ (mixed >> 1) ^ mag)
        } else {
            let seed = self.seed;
            let full = self.full.get_or_insert_with(|| {
                let mut mt = Box::new(rand_mt::Mt::new(seed));
                for _ in 0..j {
                    mt.next_u32();
                }
                mt
            });
            full.next_u32()
        }
    }

    /// `mt_rand($min, $max)` (`php_random_range32`: rejection sampling, no shortcut for min == max).
    pub fn range(&mut self, min: i64, max: i64) -> i64 {
        let umax = (max - min) as u32;
        let mut result = self.next_u32();
        if umax == u32::MAX {
            return min + result as i64;
        }
        let bound = umax + 1;
        if bound & (bound - 1) == 0 {
            return min + (result & (bound - 1)) as i64;
        }
        let limit = u32::MAX - (u32::MAX % bound) - 1;
        while result > limit {
            result = self.next_u32();
        }
        min + (result % bound) as i64
    }

    /// `RandomGenerator::chance($c)`.
    fn chance(&mut self, c: f64) -> bool {
        if c >= 1.0 {
            true
        } else if c <= 0.0 {
            false
        } else {
            (self.range(0, 99) as f64) < 100.0 * c
        }
    }
}

/// One round of `calculate_offsets` with PHP's float arithmetic and draw order.
pub fn php_step(om: f64, ox: f64, round: usize, min_spread: f64, rng: &mut PhpMt) -> (f64, f64) {
    if om + ox <= min_spread {
        return (om, ox);
    }
    let spendable = (om.max(0.0) + ox.max(0.0)) / (MAX_ROUNDS - round) as f64;
    let (lo, hi) = (
        (spendable * 250.0).floor() as i64,
        (spendable * 1000.0).floor() as i64,
    );
    let increase_min = rng.chance(om / (om + ox));
    let alter = rng.range(lo, hi) as f64 / 1000.0;
    if rng.chance(0.25) {
        let alter_max = rng.range(lo, hi) as f64 / 1000.0;
        ((om - alter).max(0.0), (ox - alter_max).max(0.0))
    } else if increase_min && om > 0.0 {
        ((om - alter).max(0.0), ox)
    } else {
        (om, (ox - alter).max(0.0))
    }
}

/// Offsets after 0..=24 rounds for a seed and initial pair.
pub fn php_path(seed: u32, om0: i64, ox0: i64, conf: &EstimConf) -> Vec<(f64, f64)> {
    let mut rng = PhpMt::new(seed);
    let min_spread = (conf.spread - conf.shift) as f64;
    let mut off = (om0 as f64, ox0 as f64);
    let mut path = vec![off];
    for round in 0..MAX_ROUNDS {
        off = php_step(off.0, off.1, round, min_spread, &mut rng);
        path.push(off);
    }
    path
}

/// A seed (and initial pair) reproducing every reading, with the hidden bounds it allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedMatch {
    pub seed: u32,
    pub om0: i64,
    pub ox0: i64,
    pub tmin: (i64, i64),
    pub tmax: (i64, i64),
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    tmin: (f64, f64),
    tmax: (f64, f64),
}

const EPS: f64 = 1e-6;

impl Bounds {
    /// Intersects with the `tmin`/`tmax` values for which the offsets display this observation.
    fn narrow(&mut self, om: f64, ox: f64, r: &RawObservation) -> bool {
        let f = 1.0 - om / 100.0;
        let g = 1.0 + ox / 100.0;
        self.tmin.0 = self.tmin.0.max(r.min_y.0 / f - EPS);
        self.tmin.1 = self.tmin.1.min(r.min_y.1 / f + EPS);
        self.tmax.0 = self.tmax.0.max(r.max_y.0 / g - EPS);
        self.tmax.1 = self.tmax.1.min(r.max_y.1 / g + EPS);
        self.tmin.0.ceil() <= self.tmin.1.floor() && self.tmax.0.ceil() <= self.tmax.1.floor()
    }
}

fn replay(
    rng: &mut PhpMt,
    seed: u32,
    (om0, ox0): (i64, i64),
    observed: &[Option<RawObservation>; MAX_ROUNDS + 1],
    last: usize,
    min_spread: f64,
    start: Bounds,
) -> Option<SeedMatch> {
    rng.reseed(seed);
    let mut bounds = start;
    let (mut om, mut ox) = (om0 as f64, ox0 as f64);
    if let Some(r) = &observed[0]
        && !bounds.narrow(om, ox, r)
    {
        return None;
    }
    for round in 0..last {
        (om, ox) = php_step(om, ox, round, min_spread, rng);
        if let Some(r) = &observed[round + 1]
            && !bounds.narrow(om, ox, r)
        {
            return None;
        }
    }
    Some(SeedMatch {
        seed,
        om0,
        ox0,
        tmin: (bounds.tmin.0.ceil() as i64, bounds.tmin.1.floor() as i64),
        tmax: (bounds.tmax.0.ceil() as i64, bounds.tmax.1.floor() as i64),
    })
}

/// Replays every seed in `seeds` for each candidate initial pair and returns the consistent ones.
/// `progress` is incremented by the number of seeds processed (for reporting).
pub(crate) fn search(
    raw: &[RawObservation],
    pairs: &[(i64, i64)],
    t_bounds: (i64, i64),
    conf: &EstimConf,
    seeds: std::ops::RangeInclusive<u32>,
    progress: &AtomicU64,
) -> Vec<SeedMatch> {
    let mut observed: [Option<RawObservation>; MAX_ROUNDS + 1] = [None; MAX_ROUNDS + 1];
    for r in raw {
        observed[r.rounds] = Some(*r);
    }
    let last = raw.iter().map(|r| r.rounds).max().unwrap_or(0);
    let min_spread = (conf.spread - conf.shift) as f64;
    let start = Bounds {
        tmin: (t_bounds.0 as f64, t_bounds.1 as f64),
        tmax: (t_bounds.0 as f64, t_bounds.1 as f64),
    };

    let matches = Mutex::new(Vec::new());
    const CHUNK: u64 = 1 << 16;
    let (first, end) = (*seeds.start() as u64, *seeds.end() as u64 + 1);
    let chunks = (end - first).div_ceil(CHUNK);
    (0..chunks).into_par_iter().for_each_init(
        || PhpMt::new(0),
        |rng, c| {
            let lo = first + c * CHUNK;
            let hi = (lo + CHUNK).min(end);
            for seed in lo..hi {
                for &pair in pairs {
                    if let Some(m) =
                        replay(rng, seed as u32, pair, &observed, last, min_spread, start)
                    {
                        matches.lock().unwrap().push(m);
                    }
                }
            }
            progress.fetch_add(hi - lo, Ordering::Relaxed);
        },
    );
    let mut matches = matches.into_inner().unwrap();
    matches.sort_by_key(|m| (m.seed, m.om0));
    matches
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_generator_matches_reference_mt19937() {
        for seed in [0u32, 1, 42, 0xdead_beef, u32::MAX] {
            let mut lazy = PhpMt::new(seed);
            let mut reference = rand_mt::Mt::new(seed);
            for _ in 0..(LAZY_OUTPUTS + 50) {
                assert_eq!(lazy.next_u32(), reference.next_u32());
            }
        }
    }

    #[test]
    fn test_php_known_values() {
        // PHP >= 7.1: mt_srand(1); mt_rand() === 895547922 (mt_rand() is the output >> 1).
        assert_eq!(PhpMt::new(1).next_u32() >> 1, 895_547_922);
        // mt_rand(0, 0) still consumes a draw.
        let mut rng = PhpMt::new(5);
        assert_eq!(rng.range(3, 3), 3);
        assert_eq!(rng.pos, 1);
    }

    #[test]
    fn test_range_is_within_bounds() {
        let mut rng = PhpMt::new(7);
        for _ in 0..1000 {
            let v = rng.range(250, 1000);
            assert!((250..=1000).contains(&v));
        }
    }
}
