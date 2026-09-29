//! Exact replay of the seeded offset path: `EstimateZombieAttackAction` calls
//! `mt_srand($estimation->getSeed())` (low 32 bits) before shrinking the offsets, so replaying
//! PHP's MT19937 for all 2^32 seeds and keeping those consistent with the readings pins them.
//!
//! Almost every seed is rejected within ~30 draws, so the cost is the MT19937 initialisation
//! chain: [`SeedBatch`] runs it for [`LANES`] seeds side by side so it vectorises.

use crate::engine::{EstimConf, MAX_ROUNDS};
use crate::inference::RawObservation;
use crate::isa::{Isa, Kernel};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const N: usize = 624;
const M: usize = 397;
/// Output `j < N - M` only needs original words `j`, `j + 1` and `j + M`.
const LAZY_OUTPUTS: usize = N - M;
const INIT_MULTIPLIER: u32 = 1_812_433_253;

const LANES: usize = 64;
/// Outputs served from a [`SeedBatch`] before a seed spills into a scalar [`PhpMt`].
const BATCH_OUTPUTS: usize = 48;
/// Seeds per rayon task (multiple of [`LANES`]).
const CHUNK: u64 = 1 << 16;

#[inline(always)]
fn temper(mut y: u32) -> u32 {
    y ^= y >> 11;
    y ^= (y << 7) & 0x9d2c_5680;
    y ^= (y << 15) & 0xefc6_0000;
    y ^ (y >> 18)
}

/// `twist(m, u, v)` of PHP's `engine_mt19937.c`.
#[inline(always)]
fn twist(m: u32, u: u32, v: u32) -> u32 {
    let mixed = (u & 0x8000_0000) | (v & 0x7fff_ffff);
    let mag = if v & 1 != 0 { 0x9908_b0df } else { 0 };
    m ^ (mixed >> 1) ^ mag
}

#[inline(always)]
fn init_word(prev: u32, i: u32) -> u32 {
    INIT_MULTIPLIER
        .wrapping_mul(prev ^ (prev >> 30))
        .wrapping_add(i)
}

/// A source of PHP `mt_rand` outputs, with PHP's range and chance helpers.
pub trait MtSource {
    fn next_u32(&mut self) -> u32;

    /// `mt_rand($min, $max)` (`php_random_range32`: rejection sampling, no shortcut for min == max).
    #[inline(always)]
    fn range(&mut self, min: i64, max: i64) -> i64 {
        let umax = (max - min) as u32;
        let mut result = self.next_u32();
        if umax == u32::MAX {
            return min + i64::from(result);
        }
        let bound = umax + 1;
        if bound.is_power_of_two() {
            return min + i64::from(result & (bound - 1));
        }
        let limit = u32::MAX - (u32::MAX % bound) - 1;
        while result > limit {
            result = self.next_u32();
        }
        min + i64::from(result % bound)
    }

    /// `RandomGenerator::chance($c)`.
    #[inline(always)]
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

/// PHP's `mt_rand` engine (MT19937), lazily initialised until a full reload is needed.
pub struct PhpMt {
    seed: u32,
    state: [u32; N],
    initialised: usize,
    reloaded: bool,
    pos: usize,
}

impl PhpMt {
    #[must_use]
    pub fn new(seed: u32) -> Self {
        let mut mt = PhpMt {
            seed,
            state: [0; N],
            initialised: 0,
            reloaded: false,
            pos: 0,
        };
        mt.reseed(seed);
        mt
    }

    /// `mt_srand($seed)`; replaying the same seed keeps the words already derived.
    pub fn reseed(&mut self, seed: u32) {
        if seed != self.seed || self.initialised == 0 || self.reloaded {
            self.seed = seed;
            self.state[0] = seed;
            self.initialised = 1;
            self.reloaded = false;
        }
        self.pos = 0;
    }

    /// Makes output number `pos` the next one.
    fn seek(&mut self, pos: usize) {
        debug_assert!(!self.reloaded || pos >= self.pos);
        self.pos = pos;
    }

    fn ensure_initialised(&mut self, upto: usize) {
        while self.initialised <= upto {
            let i = self.initialised;
            self.state[i] = init_word(self.state[i - 1], i as u32);
            self.initialised += 1;
        }
    }

    /// `mt19937_reload`.
    fn reload(&mut self) {
        let s = &mut self.state;
        for i in 0..N - M {
            s[i] = twist(s[i + M], s[i], s[i + 1]);
        }
        for i in N - M..N - 1 {
            s[i] = twist(s[i + M - N], s[i], s[i + 1]);
        }
        s[N - 1] = twist(s[M - 1], s[N - 1], s[0]);
    }
}

impl MtSource for PhpMt {
    fn next_u32(&mut self) -> u32 {
        let j = self.pos;
        self.pos += 1;
        if !self.reloaded {
            if j < LAZY_OUTPUTS {
                self.ensure_initialised(j + M);
                return temper(twist(self.state[j + M], self.state[j], self.state[j + 1]));
            }
            self.ensure_initialised(N - 1);
            self.reload();
            self.reloaded = true;
        } else if j.is_multiple_of(N) {
            self.reload();
        }
        temper(self.state[j % N])
    }
}

/// Original MT19937 words of [`LANES`] consecutive seeds that their first [`BATCH_OUTPUTS`]
/// outputs read, lane-interleaved so the initialisation chain vectorises across seeds. Outputs
/// are tempered lazily: most seeds are rejected long before [`BATCH_OUTPUTS`] draws.
struct SeedBatch {
    head: [[u32; LANES]; BATCH_OUTPUTS + 1],
    tail: [[u32; LANES]; BATCH_OUTPUTS],
}

impl SeedBatch {
    fn new() -> Self {
        SeedBatch {
            head: [[0; LANES]; BATCH_OUTPUTS + 1],
            tail: [[0; LANES]; BATCH_OUTPUTS],
        }
    }

    /// Seeds `first..first + LANES` (wrapping). Inlined so it vectorises with the target features
    /// of [`Replay::run_range`].
    #[inline(always)]
    fn fill(&mut self, first: u32) {
        #[inline(always)]
        fn step(cur: &mut [u32; LANES], i: usize) {
            for w in cur.iter_mut() {
                *w = init_word(*w, i as u32);
            }
        }

        let mut cur: [u32; LANES] = std::array::from_fn(|l| first.wrapping_add(l as u32));
        self.head[0] = cur;
        for (i, word) in self.head.iter_mut().enumerate().skip(1) {
            step(&mut cur, i);
            *word = cur;
        }
        for i in BATCH_OUTPUTS + 1..M {
            step(&mut cur, i);
        }
        for (k, word) in self.tail.iter_mut().enumerate() {
            step(&mut cur, M + k);
            *word = cur;
        }
    }
}

/// One seed of a [`SeedBatch`]; spills into a scalar [`PhpMt`] past [`BATCH_OUTPUTS`] draws.
struct LaneMt<'a> {
    batch: &'a SeedBatch,
    lane: usize,
    seed: u32,
    pos: usize,
    spill: &'a mut PhpMt,
}

impl MtSource for LaneMt<'_> {
    #[inline(always)]
    fn next_u32(&mut self) -> u32 {
        let j = self.pos;
        self.pos += 1;
        if j < BATCH_OUTPUTS {
            let (b, l) = (self.batch, self.lane);
            temper(twist(b.tail[j][l], b.head[j][l], b.head[j + 1][l]))
        } else {
            if j == BATCH_OUTPUTS {
                self.spill.reseed(self.seed);
                self.spill.seek(j);
            }
            self.spill.next_u32()
        }
    }
}

/// One round of `calculate_offsets` with PHP's float arithmetic and draw order.
#[inline(always)]
pub fn php_step<R: MtSource>(
    om: f64,
    ox: f64,
    round: usize,
    min_spread: f64,
    rng: &mut R,
) -> (f64, f64) {
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
#[must_use]
pub fn php_path(seed: u32, om0: i64, ox0: i64, conf: &EstimConf) -> Vec<(f64, f64)> {
    let mut rng = PhpMt::new(seed);
    let min_spread = conf.min_spread();
    let mut off = (om0 as f64, ox0 as f64);
    let mut path = Vec::with_capacity(MAX_ROUNDS + 1);
    path.push(off);
    for round in 0..MAX_ROUNDS {
        off = php_step(off.0, off.1, round, min_spread, &mut rng);
        path.push(off);
    }
    path
}

/// Initial pair and hidden bounds a compatible seed allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Window {
    pub om0: i64,
    pub ox0: i64,
    pub tmin: (i64, i64),
    pub tmax: (i64, i64),
}

/// Number of seeds of a search that replay every reading within `window`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct WindowMatch {
    pub window: Window,
    pub seeds: u64,
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    tmin: (f64, f64),
    tmax: (f64, f64),
}

const EPS: f64 = 1e-6;

impl Bounds {
    /// Intersects with the `tmin`/`tmax` values for which the offsets display this observation.
    #[inline(always)]
    fn narrow(&mut self, om: f64, ox: f64, r: &RawObservation) -> bool {
        let inv_f = 1.0 / (1.0 - om * 0.01);
        let inv_g = 1.0 / (1.0 + ox * 0.01);
        self.tmin.0 = self.tmin.0.max(r.min_y.0 * inv_f - EPS);
        self.tmin.1 = self.tmin.1.min(r.min_y.1 * inv_f + EPS);
        self.tmax.0 = self.tmax.0.max(r.max_y.0 * inv_g - EPS);
        self.tmax.1 = self.tmax.1.min(r.max_y.1 * inv_g + EPS);
        self.tmin.0.ceil() <= self.tmin.1.floor() && self.tmax.0.ceil() <= self.tmax.1.floor()
    }

    #[inline(always)]
    fn window(&self, (om0, ox0): (i64, i64)) -> Window {
        Window {
            om0,
            ox0,
            tmin: (self.tmin.0.ceil() as i64, self.tmin.1.floor() as i64),
            tmax: (self.tmax.0.ceil() as i64, self.tmax.1.floor() as i64),
        }
    }
}

type Counts = HashMap<Window, u64>;

/// What the replay of one seed checks against, shared by all seeds of a search.
struct Replay {
    observed: [Option<RawObservation>; MAX_ROUNDS + 1],
    last: usize,
    min_spread: f64,
    /// Candidate initial pairs with their bounds after the seed-independent 0-round observation.
    starts: Vec<((i64, i64), Bounds)>,
    /// Lowest offsets still able to reach every later observation: offsets only shrink.
    floors: [(f64, f64); MAX_ROUNDS + 1],
}

impl Replay {
    #[inline(always)]
    fn run<R: MtSource>(&self, rng: &mut R, pair: (i64, i64), start: Bounds) -> Option<Window> {
        let mut bounds = start;
        let (mut om, mut ox) = (pair.0 as f64, pair.1 as f64);
        for round in 0..self.last {
            (om, ox) = php_step(om, ox, round, self.min_spread, rng);
            let (floor_m, floor_x) = self.floors[round + 1];
            if om < floor_m || ox < floor_x {
                return None;
            }
            if let Some(r) = &self.observed[round + 1]
                && !bounds.narrow(om, ox, r)
            {
                return None;
            }
        }
        Some(bounds.window(pair))
    }

    /// Replays seeds `lo..hi` (`u64` so the range can end at 2^32), compiled once per
    /// instruction set (SSE4.1+ also turns `floor`/`ceil` into single instructions).
    fn run_range(&self, lo: u64, hi: u64, isa: Isa, state: &mut ThreadState, found: &mut Counts) {
        isa.run(SeedRange {
            replay: self,
            lo,
            hi,
            state,
            found,
        });
    }

    #[inline(always)]
    fn run_range_impl(&self, lo: u64, hi: u64, state: &mut ThreadState, found: &mut Counts) {
        let ThreadState { batch, spill } = state;
        for base in (lo..hi).step_by(LANES) {
            batch.fill(base as u32);
            let batch = &*batch;
            let lanes = (hi - base).min(LANES as u64) as usize;
            for lane in 0..lanes {
                let seed = (base + lane as u64) as u32;
                for &(pair, start) in &self.starts {
                    let mut rng = LaneMt {
                        batch,
                        lane,
                        seed,
                        pos: 0,
                        spill: &mut *spill,
                    };
                    if let Some(window) = self.run(&mut rng, pair, start) {
                        *found.entry(window).or_default() += 1;
                    }
                }
            }
        }
    }
}

/// [`Replay::run_range_impl`] as a [`Kernel`].
struct SeedRange<'a> {
    replay: &'a Replay,
    lo: u64,
    hi: u64,
    state: &'a mut ThreadState,
    found: &'a mut Counts,
}

impl Kernel for SeedRange<'_> {
    type Output = ();

    #[inline(always)]
    fn run(self) {
        self.replay
            .run_range_impl(self.lo, self.hi, self.state, self.found);
    }
}

struct ThreadState {
    batch: SeedBatch,
    spill: PhpMt,
}

/// Suffix maxima of the smallest offsets each observation allows over the whole `[tmin, tmax]`
/// range, with a margin well above float noise so no valid path is dropped.
fn offset_floors(
    observed: &[Option<RawObservation>; MAX_ROUNDS + 1],
    (t_lo, t_hi): (i64, i64),
) -> [(f64, f64); MAX_ROUNDS + 1] {
    const MARGIN: f64 = 1e-6;
    let (t_lo, t_hi) = (t_lo as f64, t_hi as f64);
    let mut floors = [(f64::NEG_INFINITY, f64::NEG_INFINITY); MAX_ROUNDS + 1];
    let mut running = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (floor, obs) in floors.iter_mut().zip(observed).rev() {
        if let Some(r) = obs {
            // tmin*(1 - om/100) <= min_y.1 with tmin >= t_lo; tmax*(1 + ox/100) >= max_y.0 with
            // tmax <= t_hi.
            let om = 100.0 * (1.0 - r.min_y.1 / t_lo) - MARGIN;
            let ox = 100.0 * (r.max_y.0 / t_hi - 1.0) - MARGIN;
            running = (running.0.max(om), running.1.max(ox));
        }
        *floor = running;
    }
    floors
}

/// Slice `index` of the seed space `0..=u32::MAX` cut into `parts` contiguous slices, `None` when
/// `index >= parts`.
#[must_use]
pub fn seed_slice(index: u32, parts: u32) -> Option<RangeInclusive<u32>> {
    let (index, parts) = (u64::from(index), u64::from(parts.max(1)));
    if index >= parts {
        return None;
    }
    let total = u64::from(u32::MAX) + 1;
    let lo = u32::try_from(total * index / parts).ok()?;
    let hi = u32::try_from(total * (index + 1) / parts - 1).ok()?;
    Some(lo..=hi)
}

/// Replays every seed in `seeds` for each candidate initial pair and counts the consistent ones
/// per [`Window`], sorted. `progress` counts processed seeds; setting `cancel` stops the search
/// within a chunk per thread and returns `None`.
pub(crate) fn search(
    raw: &[RawObservation],
    pairs: &[(i64, i64)],
    t_bounds: (i64, i64),
    conf: &EstimConf,
    seeds: RangeInclusive<u32>,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Option<Vec<WindowMatch>> {
    let mut observed = [None; MAX_ROUNDS + 1];
    for r in raw {
        observed[r.rounds] = Some(*r);
    }
    let (lo, hi) = (t_bounds.0 as f64, t_bounds.1 as f64);
    let outer = Bounds {
        tmin: (lo, hi),
        tmax: (lo, hi),
    };
    let starts: Vec<_> = pairs
        .iter()
        .filter_map(|&(om0, ox0)| {
            let mut bounds = outer;
            match &observed[0] {
                Some(r) if !bounds.narrow(om0 as f64, ox0 as f64, r) => None,
                _ => Some(((om0, ox0), bounds)),
            }
        })
        .collect();
    let (first, end) = (u64::from(*seeds.start()), u64::from(*seeds.end()) + 1);
    let last = raw.iter().map(|r| r.rounds).max().unwrap_or(0);

    let counts = if last == 0 {
        // Only 0 % readings: the seed never comes into play, every seed matches.
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        progress.fetch_add(end - first, Ordering::Relaxed);
        starts
            .iter()
            .map(|&(pair, bounds)| (bounds.window(pair), end - first))
            .collect()
    } else {
        let replay = Replay {
            floors: offset_floors(&observed, t_bounds),
            observed,
            last,
            min_spread: conf.min_spread(),
            starts,
        };
        let isa = Isa::detect();
        (0..(end - first).div_ceil(CHUNK))
            .into_par_iter()
            .map_init(
                // Boxed: rayon keeps the init value on the stack of every split level.
                || {
                    Box::new(ThreadState {
                        batch: SeedBatch::new(),
                        spill: PhpMt::new(0),
                    })
                },
                |state, c| {
                    let mut found = Counts::new();
                    if cancel.load(Ordering::Relaxed) {
                        return found;
                    }
                    let lo = first + c * CHUNK;
                    let hi = (lo + CHUNK).min(end);
                    replay.run_range(lo, hi, isa, state, &mut found);
                    progress.fetch_add(hi - lo, Ordering::Relaxed);
                    found
                },
            )
            .reduce(Counts::new, |mut acc, found| {
                for (window, n) in found {
                    *acc.entry(window).or_default() += n;
                }
                acc
            })
    };
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let mut matches: Vec<WindowMatch> = counts
        .into_iter()
        .map(|(window, seeds)| WindowMatch { window, seeds })
        .collect();
    matches.sort_unstable();
    Some(matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lazy_generator_matches_reference_mt19937() {
        for seed in [0u32, 1, 42, 0xdead_beef, u32::MAX] {
            let mut lazy = PhpMt::new(seed);
            let mut reference = rand_mt::Mt::new(seed);
            // Crosses the lazy/reload boundary and a second reload.
            for _ in 0..(2 * N + 50) {
                assert_eq!(lazy.next_u32(), reference.next_u32());
            }
        }
    }

    #[test]
    fn test_batch_lanes_match_scalar_generator() {
        let mut batch = SeedBatch::new();
        let mut spill = PhpMt::new(0);
        for first in [0u32, 0xdead_beef, u32::MAX - 5] {
            batch.fill(first);
            for lane in 0..LANES {
                let seed = first.wrapping_add(lane as u32);
                let mut reference = PhpMt::new(seed);
                let mut rng = LaneMt {
                    batch: &batch,
                    lane,
                    seed,
                    pos: 0,
                    spill: &mut spill,
                };
                // Crosses the batch/spill boundary.
                for _ in 0..(BATCH_OUTPUTS + 40) {
                    assert_eq!(rng.next_u32(), reference.next_u32(), "seed {seed:#x}");
                }
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

    #[test]
    fn test_seed_slices_cover_the_seed_space_once() {
        for parts in [1, 3, 32, 1000] {
            let slices: Vec<_> = (0..parts).map_while(|i| seed_slice(i, parts)).collect();
            assert_eq!(slices.len(), parts as usize);
            assert_eq!(*slices[0].start(), 0);
            assert_eq!(*slices.last().unwrap().end(), u32::MAX);
            for pair in slices.windows(2) {
                assert_eq!(u64::from(*pair[0].end()) + 1, u64::from(*pair[1].start()));
            }
            assert_eq!(seed_slice(parts, parts), None);
        }
        assert_eq!(seed_slice(0, 0), Some(0..=u32::MAX));
    }

    #[test]
    fn test_php_path_only_shrinks_and_nests_readings() {
        let conf = EstimConf::default();
        let target = crate::engine::HiddenTarget {
            tmin: 2200,
            tmax: 2420,
            om0: 7,
            ox0: 21,
        };
        for seed in 0..200 {
            let path = php_path(seed, target.om0, target.ox0, &conf);
            assert_eq!(path.len(), MAX_ROUNDS + 1);
            for pair in path.windows(2) {
                let ((om0, ox0), (om1, ox1)) = (pair[0], pair[1]);
                assert!(om1 <= om0 && ox1 <= ox0 && om1 >= 0.0 && ox1 >= 0.0);
                let (a0, b0) = crate::engine::displayed_range(&target, om0, ox0, 1, 1.0);
                let (a1, b1) = crate::engine::displayed_range(&target, om1, ox1, 1, 1.0);
                assert!(a0 <= a1 && b1 <= b0);
            }
        }
    }
}
