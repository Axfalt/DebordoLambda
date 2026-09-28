//! Bayesian inversion of the watchtower: posterior of the real attack given every reading of the day.
//!
//! All readings of a day replay the same seeded offset path, the reading at `n` weighted citizens
//! being that path after `n` rounds. For each hidden target `(tmin, tmax, om0, ox0)` reachable from
//! the generator, a guided particle filter computes the probability that the offset process lands
//! on every observed range; the exact prior enumeration then turns these likelihoods into P(attack).

use crate::engine::*;
use crate::seed::SeedMatch;
use rand::RngExt;
use rand_mt::Mt64;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

/// One watchtower reading as displayed in game: `[pct%] min - max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub pct: u32,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GuetInput {
    /// Current town day.
    pub day: i64,
    /// `true` for the J+1 estimate (watchtower upgrade), which targets `day + 1`.
    pub future: bool,
    pub mode: AttackMode,
    pub readings: Vec<Reading>,
    /// J+1 readings taken the day before for this same attack (`future == false` only). They
    /// replay the same seeded path as today's readings, rounded to blocks.
    #[serde(default)]
    pub planner: Vec<Reading>,
    /// Red-soul factor of today's readings, also applied to the night attack.
    #[serde(default = "unit_factor")]
    pub soul_factor: f64,
    /// Red-soul factor when the planner readings were taken (defaults to `soul_factor`).
    #[serde(default)]
    pub planner_soul_factor: Option<f64>,
}

fn unit_factor() -> f64 {
    1.0
}

impl Default for GuetInput {
    fn default() -> Self {
        GuetInput {
            day: 0,
            future: false,
            mode: AttackMode::Normal,
            readings: Vec::new(),
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
        }
    }
}

impl GuetInput {
    pub fn estimated_day(&self) -> i64 {
        self.day + i64::from(self.future)
    }

    pub fn blocks(&self) -> i64 {
        if self.future {
            future_blocks(self.estimated_day())
        } else {
            1
        }
    }
}

/// A validated reading, keyed by the number of offset rounds it reveals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Observation {
    pub rounds: usize,
    pub pct: u32,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuetError {
    MissingDay,
    InvalidDay(i64),
    NoReadings,
    InvalidPercent(u32),
    InvertedRange(Reading),
    ConflictingReadings(Reading, Reading),
    /// Today's reading and yesterday's J+1 reading at this percentage cannot share one path.
    PlannerMismatch(u32),
    Inconsistent,
}

impl fmt::Display for GuetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GuetError::MissingDay => write!(f, "jour manquant : précisez `jour: N`."),
            GuetError::InvalidDay(day) => write!(f, "jour invalide ({day}), il doit être ≥ 1."),
            GuetError::NoReadings => write!(
                f,
                "aucun relevé reconnu (format attendu : `33% 2047 - 2749`)."
            ),
            GuetError::InvalidPercent(pct) => {
                write!(
                    f,
                    "pourcentage {pct}% impossible : la tour affiche n/24 arrondi."
                )
            }
            GuetError::InvertedRange(r) => {
                write!(f, "relevé {}% invalide : {} > {}.", r.pct, r.min, r.max)
            }
            GuetError::ConflictingReadings(a, b) => write!(
                f,
                "relevés contradictoires à {}% : {} - {} et {} - {}.",
                a.pct, a.min, a.max, b.min, b.max
            ),
            GuetError::PlannerMismatch(pct) => write!(
                f,
                "le relevé du jour et le relevé J+1 de la veille à {pct}% sont incompatibles."
            ),
            GuetError::Inconsistent => write!(
                f,
                "aucune attaque ne peut produire ces relevés. Vérifiez le jour, le mode, \
                 l'option J+1 et les valeurs (âmes rouges et événements ne sont pas modélisés)."
            ),
        }
    }
}

impl std::error::Error for GuetError {}

/// Validates today's readings, maps percentages to round counts and drops duplicates.
pub fn observations(input: &GuetInput) -> Result<Vec<Observation>, GuetError> {
    if input.day < 1 {
        return Err(GuetError::InvalidDay(input.day));
    }
    if input.readings.is_empty() {
        return Err(GuetError::NoReadings);
    }
    validate(&input.readings)
}

fn validate(readings: &[Reading]) -> Result<Vec<Observation>, GuetError> {
    let mut by_rounds: HashMap<usize, Reading> = HashMap::new();
    for reading in readings {
        if reading.min > reading.max {
            return Err(GuetError::InvertedRange(*reading));
        }
        let rounds =
            rounds_for_percent(reading.pct).ok_or(GuetError::InvalidPercent(reading.pct))?;
        match by_rounds.get(&rounds) {
            Some(prev) if prev.min != reading.min || prev.max != reading.max => {
                return Err(GuetError::ConflictingReadings(*prev, *reading));
            }
            Some(_) => {}
            None => {
                by_rounds.insert(rounds, *reading);
            }
        }
    }

    let mut obs: Vec<Observation> = by_rounds
        .into_iter()
        .map(|(rounds, r)| Observation {
            rounds,
            pct: r.pct,
            min: r.min,
            max: r.max,
        })
        .collect();
    obs.sort_by_key(|o| o.rounds);
    Ok(obs)
}

#[derive(Debug, Clone, Copy)]
pub struct InferenceOptions {
    /// Particles per hidden-target hypothesis.
    pub particles: usize,
    pub seed: u64,
    pub conf: EstimConf,
}

impl Default for InferenceOptions {
    fn default() -> Self {
        InferenceOptions {
            particles: 4096,
            seed: 0x6775_6574,
            conf: EstimConf::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TargetPosterior {
    pub target: HiddenTarget,
    pub probability: f64,
}

#[derive(Debug, Clone)]
pub struct Posterior {
    /// `(attack, probability)` sorted by attack, zero-probability values omitted.
    pub attack: Vec<(i64, f64)>,
    /// Hidden targets sorted by decreasing probability.
    pub targets: Vec<TargetPosterior>,
    pub observations: Vec<Observation>,
    /// Hidden targets evaluated with the particle filter.
    pub hypotheses: usize,
}

impl Posterior {
    /// Smallest attack whose cumulative probability reaches `p`.
    pub fn quantile(&self, p: f64) -> i64 {
        let mut cum = 0.0;
        for &(value, prob) in &self.attack {
            cum += prob;
            if cum >= p - 1e-12 {
                return value;
            }
        }
        self.attack.last().map_or(0, |&(v, _)| v)
    }

    pub fn median(&self) -> i64 {
        self.quantile(0.5)
    }

    /// Equal-tailed interval holding `mass` of the probability.
    pub fn central_interval(&self, mass: f64) -> (i64, i64) {
        let tail = (1.0 - mass) / 2.0;
        (self.quantile(tail), self.quantile(1.0 - tail))
    }

    pub fn mean(&self) -> f64 {
        self.attack.iter().map(|&(v, p)| v as f64 * p).sum()
    }

    pub fn support(&self) -> (i64, i64) {
        (
            self.attack.first().map_or(0, |&(v, _)| v),
            self.attack.last().map_or(0, |&(v, _)| v),
        )
    }

    /// Posterior of the initial `(om0, ox0)` offset pair, sorted by decreasing probability.
    pub fn offset_pairs(&self) -> Vec<((i64, i64), f64)> {
        let mut pairs: HashMap<(i64, i64), f64> = HashMap::new();
        for t in &self.targets {
            *pairs.entry((t.target.om0, t.target.ox0)).or_default() += t.probability;
        }
        let mut pairs: Vec<_> = pairs.into_iter().collect();
        pairs.sort_by(|a, b| b.1.total_cmp(&a.1));
        pairs
    }
}

/// Inclusive window of offsets (milli-percent) compatible with one displayed bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    lo: i64,
    hi: i64,
}

impl Window {
    fn contains(&self, v: i64) -> bool {
        self.lo <= v && v <= self.hi
    }
}

const PCT_MILLI: f64 = (100 * MILLI) as f64;

/// Tolerance on window bounds: exact rounding ties are accepted on both sides, since PHP's float
/// path may land either way there.
const WINDOW_EPS: f64 = 1e-6;

/// Offsets `m` (milli-percent) with `t - t*m/100000 ∈ [y.0, y.1]`.
fn min_side_window(t: i64, y: (f64, f64)) -> Window {
    let t = t as f64;
    Window {
        lo: ((PCT_MILLI * (t - y.1) / t - WINDOW_EPS).ceil() as i64).max(0),
        hi: (PCT_MILLI * (t - y.0) / t + WINDOW_EPS).floor() as i64,
    }
}

/// Offsets `m` (milli-percent) with `t + t*m/100000 ∈ [y.0, y.1]`.
fn max_side_window(t: i64, y: (f64, f64)) -> Window {
    let t = t as f64;
    Window {
        lo: ((PCT_MILLI * (y.0 - t) / t - WINDOW_EPS).ceil() as i64).max(0),
        hi: (PCT_MILLI * (y.1 - t) / t + WINDOW_EPS).floor() as i64,
    }
}

/// Draws `k` of one shrink step that bring `current` into `window` (clamped at 0 like the game).
#[derive(Debug, Clone, Copy)]
struct ValidDraws {
    below: (i64, i64),
    clamped: (i64, i64),
}

impl ValidDraws {
    fn new(current: i64, window: Window, lo: i64, hi: i64) -> Self {
        let below = (lo.max(current - window.hi), hi.min(current - window.lo));
        let clamped = if window.lo == 0 && window.hi >= 0 {
            (lo.max(current + 1), hi)
        } else {
            (1, 0)
        };
        ValidDraws { below, clamped }
    }

    fn len(range: (i64, i64)) -> i64 {
        (range.1 - range.0 + 1).max(0)
    }

    fn count(&self) -> i64 {
        Self::len(self.below) + Self::len(self.clamped)
    }

    fn sample(&self, rng: &mut Mt64) -> i64 {
        let n_below = Self::len(self.below);
        let r = rng.random_range(0..self.count());
        if r < n_below {
            self.below.0 + r
        } else {
            self.clamped.0 + (r - n_below)
        }
    }
}

/// One round conditioned on landing in `(wm, wx)`: returns the probability of doing so and a
/// state drawn from the conditional distribution (unchanged when the probability is 0).
fn guided_step(
    off: Offsets,
    round: usize,
    min_spread: i64,
    wm: Window,
    wx: Window,
    rng: &mut Mt64,
) -> (f64, Offsets) {
    let in_m = wm.contains(off.min);
    let in_x = wx.contains(off.max);
    if off.min + off.max <= min_spread {
        return (if in_m && in_x { 1.0 } else { 0.0 }, off);
    }

    let (lo, hi) = step_bounds(off, round);
    let n = (hi - lo + 1) as f64;
    let dm = ValidDraws::new(off.min, wm, lo, hi);
    let dx = ValidDraws::new(off.max, wx, lo, hi);
    let fm = dm.count() as f64 / n;
    let fx = dx.count() as f64 / n;
    let p_min_side = min_side_percent(off) as f64 / 100.0;

    let p_both = 0.25 * fm * fx;
    let p_min = if in_x { 0.75 * p_min_side * fm } else { 0.0 };
    let p_max = if in_m {
        0.75 * (1.0 - p_min_side) * fx
    } else {
        0.0
    };
    let total = p_both + p_min + p_max;
    if total <= 0.0 {
        return (0.0, off);
    }

    let u = rng.random::<f64>() * total;
    let next = if u < p_both {
        Offsets {
            min: (off.min - dm.sample(rng)).max(0),
            max: (off.max - dx.sample(rng)).max(0),
        }
    } else if u < p_both + p_min {
        Offsets {
            min: (off.min - dm.sample(rng)).max(0),
            max: off.max,
        }
    } else {
        Offsets {
            min: off.min,
            max: (off.max - dx.sample(rng)).max(0),
        }
    };
    (total, next)
}

/// Observation as ranges of the unscaled bounds `tmin - tmin*om/100` and `tmax + tmax*ox/100`,
/// before the red-soul factor, `round()` and block rounding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RawObservation {
    pub(crate) rounds: usize,
    pub(crate) min_y: (f64, f64),
    pub(crate) max_y: (f64, f64),
}

impl RawObservation {
    fn new(o: &Observation, blocks: i64, soul_factor: f64) -> Self {
        // Displayed min = floor(round(y * soul) / blocks) * blocks, max uses ceil.
        let (min, max) = (o.min as f64, o.max as f64);
        let b = blocks as f64;
        RawObservation {
            rounds: o.rounds,
            min_y: ((min - 0.5) / soul_factor, (min + b - 0.5) / soul_factor),
            max_y: ((max - b + 0.5) / soul_factor, (max + 0.5) / soul_factor),
        }
    }
}

/// Today's observations, intersected round by round with yesterday's J+1 readings if any.
fn raw_observations(
    input: &GuetInput,
    observations: &[Observation],
) -> Result<Vec<RawObservation>, GuetError> {
    let mut raw: Vec<RawObservation> = observations
        .iter()
        .map(|o| RawObservation::new(o, input.blocks(), input.soul_factor))
        .collect();
    if input.future {
        return Ok(raw);
    }

    let planner_blocks = future_blocks(input.day);
    let planner_soul = input.planner_soul_factor.unwrap_or(input.soul_factor);
    for p in validate(&input.planner)? {
        let extra = RawObservation::new(&p, planner_blocks, planner_soul);
        match raw.iter_mut().find(|r| r.rounds == p.rounds) {
            Some(r) => {
                r.min_y = (r.min_y.0.max(extra.min_y.0), r.min_y.1.min(extra.min_y.1));
                r.max_y = (r.max_y.0.max(extra.max_y.0), r.max_y.1.min(extra.max_y.1));
                if r.min_y.0 > r.min_y.1 + WINDOW_EPS || r.max_y.0 > r.max_y.1 + WINDOW_EPS {
                    return Err(GuetError::PlannerMismatch(p.pct));
                }
            }
            None => raw.push(extra),
        }
    }
    raw.sort_by_key(|r| r.rounds);
    Ok(raw)
}

fn splitmix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

fn target_seed(seed: u64, t: &HiddenTarget) -> u64 {
    let key = (t.tmin as u64) << 40 ^ (t.tmax as u64) << 16 ^ (t.om0 as u64) << 8 ^ t.ox0 as u64;
    splitmix64(seed ^ splitmix64(key))
}

/// Systematic resampling in place; zero-weight particles are never selected.
fn resample(
    states: &mut Vec<Offsets>,
    weights: &[f64],
    total: f64,
    rng: &mut Mt64,
    scratch: &mut Vec<Offsets>,
) {
    let n = states.len();
    let step = total / n as f64;
    let start = rng.random::<f64>() * step;
    scratch.clear();
    let mut cum = 0.0;
    let mut j = 0;
    for i in 0..n {
        let target = start + i as f64 * step;
        while j < n - 1 && cum + weights[j] <= target {
            cum += weights[j];
            j += 1;
        }
        scratch.push(states[j]);
    }
    std::mem::swap(states, scratch);
}

/// Log-likelihood of every observation given a hidden target, `None` when impossible.
fn log_likelihood(
    target: &HiddenTarget,
    raw: &[RawObservation],
    opts: &InferenceOptions,
) -> Option<f64> {
    let mut windows: [Option<(Window, Window)>; MAX_ROUNDS + 1] = [None; MAX_ROUNDS + 1];
    for r in raw {
        windows[r.rounds] = Some((
            min_side_window(target.tmin, r.min_y),
            max_side_window(target.tmax, r.max_y),
        ));
    }

    // Offsets only shrink: cheap rejection before running particles.
    let start = Offsets::initial(target);
    let (mut upper_m, mut upper_x) = (start.min, start.max);
    for (wm, wx) in windows.iter().flatten() {
        upper_m = upper_m.min(wm.hi);
        upper_x = upper_x.min(wx.hi);
        if upper_m < wm.lo || upper_x < wx.lo {
            return None;
        }
    }
    if let Some((wm, wx)) = windows[0]
        && !(wm.contains(start.min) && wx.contains(start.max))
    {
        return None;
    }

    let last = raw.iter().map(|r| r.rounds).max().unwrap_or(0);
    let min_spread = opts.conf.min_spread_milli();
    let particles = opts.particles.max(1);
    let mut rng = Mt64::new(target_seed(opts.seed, target));
    let mut states = vec![start; particles];
    let mut weights = vec![0.0; particles];
    let mut scratch = Vec::with_capacity(particles);
    let mut log_l = 0.0;

    for round in 0..last {
        match windows[round + 1] {
            None => {
                for s in states.iter_mut() {
                    *s = step_offsets(*s, round, min_spread, &mut rng);
                }
            }
            Some((wm, wx)) => {
                let mut total = 0.0;
                for (s, w) in states.iter_mut().zip(weights.iter_mut()) {
                    let (p, next) = guided_step(*s, round, min_spread, wm, wx, &mut rng);
                    *s = next;
                    *w = p;
                    total += p;
                }
                if total <= 0.0 {
                    return None;
                }
                log_l += (total / particles as f64).ln();
                resample(&mut states, &weights, total, &mut rng, &mut scratch);
            }
        }
    }
    Some(log_l)
}

/// Calls `f(value, target)` for every generator draw compatible with the readings' outer bounds.
fn for_each_draw(
    input: &GuetInput,
    conf: &EstimConf,
    (t_lo, t_hi): (i64, i64),
    mut f: impl FnMut(i64, HiddenTarget),
) {
    let day = input.estimated_day();
    let prior = ValuePrior::new(day, input.mode);
    let (off_lo, off_hi) = off_raw_range(day, conf);
    let shift_max = shift_raw_max(day, conf);
    let mut offsets_cache: HashMap<(i64, i64), Vec<(i64, i64)>> = HashMap::new();

    // tmin <= value <= tmax, so the value lies within the outer bounds as well.
    for value in t_lo.max(prior.min)..=t_hi.min(prior.max) {
        for shift_raw in 0..=shift_max {
            let (tmin, tmax) = shifted_targets(day, input.mode, conf, value, shift_raw);
            if tmin < t_lo || tmax > t_hi {
                continue;
            }
            let pairs = offsets_cache.entry((tmin, tmax)).or_insert_with(|| {
                (off_lo..=off_hi)
                    .map(|off_raw| initial_offsets(day, input.mode, conf, tmin, tmax, off_raw))
                    .collect()
            });
            for &(om0, ox0) in pairs.iter() {
                f(
                    value,
                    HiddenTarget {
                        tmin,
                        tmax,
                        om0,
                        ox0,
                    },
                );
            }
        }
    }
}

/// Validated observations, their raw ranges and the outer `[tmin, tmax]` bounds they allow.
type Prepared = (Vec<Observation>, Vec<RawObservation>, (i64, i64));

fn prepare(input: &GuetInput) -> Result<Prepared, GuetError> {
    let observations = observations(input)?;
    let raw = raw_observations(input, &observations)?;

    // Offsets are non-negative: every displayed min is <= tmin and every displayed max >= tmax.
    let t_lo = raw
        .iter()
        .map(|r| (r.min_y.0 - WINDOW_EPS).ceil() as i64)
        .max()
        .unwrap_or(0);
    let t_hi = raw
        .iter()
        .map(|r| (r.max_y.1 + WINDOW_EPS).floor() as i64)
        .min()
        .unwrap_or(0);
    if t_lo > t_hi {
        return Err(GuetError::Inconsistent);
    }
    Ok((observations, raw, (t_lo, t_hi)))
}

fn hidden_targets(
    input: &GuetInput,
    conf: &EstimConf,
    bounds: (i64, i64),
) -> HashSet<HiddenTarget> {
    let mut targets = HashSet::new();
    for_each_draw(input, conf, bounds, |_, t| {
        targets.insert(t);
    });
    targets
}

/// Combines the exact generator prior with a per-target relative likelihood.
fn build_posterior(
    input: &GuetInput,
    conf: &EstimConf,
    observations: Vec<Observation>,
    (t_lo, t_hi): (i64, i64),
    hypotheses: usize,
    likelihood: impl Fn(&HiddenTarget) -> Option<f64>,
) -> Result<Posterior, GuetError> {
    let prior = ValuePrior::new(input.estimated_day(), input.mode);
    let width = (t_hi - t_lo + 1) as usize;
    let mut attack = vec![0.0; width];
    let mut target_mass: HashMap<HiddenTarget, f64> = HashMap::new();
    for_each_draw(input, conf, (t_lo, t_hi), |value, t| {
        if let Some(l) = likelihood(&t) {
            let w = prior.weight(value) * l;
            *target_mass.entry(t).or_default() += w;
            if input.mode != AttackMode::Hard {
                attack[(value - t_lo) as usize] += w;
            }
        }
    });
    if input.mode == AttackMode::Hard {
        // Hard mode redraws the attack uniformly within [tmin, tmax].
        for (t, &w) in &target_mass {
            let share = w / (t.tmax - t.tmin + 1) as f64;
            for v in t.tmin..=t.tmax {
                attack[(v - t_lo) as usize] += share;
            }
        }
    }

    let total: f64 = attack.iter().sum();
    if total <= 0.0 {
        return Err(GuetError::Inconsistent);
    }
    let attack = attack
        .into_iter()
        .enumerate()
        .filter(|&(_, w)| w > 0.0)
        .map(|(i, w)| (t_lo + i as i64, w / total))
        .collect();

    let mass_total: f64 = target_mass.values().sum();
    let mut targets: Vec<TargetPosterior> = target_mass
        .into_iter()
        .map(|(target, w)| TargetPosterior {
            target,
            probability: w / mass_total,
        })
        .collect();
    targets.sort_by(|a, b| {
        b.probability
            .total_cmp(&a.probability)
            .then(a.target.cmp(&b.target))
    });

    Ok(Posterior {
        attack,
        targets,
        observations,
        hypotheses,
    })
}

/// Posterior treating the offset path as random (particle filter over the shrink process).
pub fn infer(input: &GuetInput, opts: &InferenceOptions) -> Result<Posterior, GuetError> {
    let (observations, raw, bounds) = prepare(input)?;
    let targets = hidden_targets(input, &opts.conf, bounds);
    let hypotheses = targets.len();

    let log_l: HashMap<HiddenTarget, f64> = targets
        .into_par_iter()
        .filter_map(|t| log_likelihood(&t, &raw, opts).map(|l| (t, l)))
        .collect();
    let max_log = log_l.values().copied().fold(f64::NEG_INFINITY, f64::max);
    if !max_log.is_finite() {
        return Err(GuetError::Inconsistent);
    }

    build_posterior(input, &opts.conf, observations, bounds, hypotheses, |t| {
        log_l.get(t).map(|l| (l - max_log).exp())
    })
}

#[derive(Debug, Clone)]
pub struct ExactPosterior {
    pub posterior: Posterior,
    /// Seeds (with their initial pair) that reproduce every reading.
    pub matches: Vec<SeedMatch>,
}

/// Posterior from the seeds that replay every reading exactly (see [`crate::seed`]).
///
/// `seeds` is normally the full `0..=u32::MAX` range; `progress` counts processed seeds.
pub fn infer_exact(
    input: &GuetInput,
    conf: &EstimConf,
    seeds: std::ops::RangeInclusive<u32>,
    progress: &std::sync::atomic::AtomicU64,
) -> Result<ExactPosterior, GuetError> {
    let (observations, raw, bounds) = prepare(input)?;
    let targets = hidden_targets(input, conf, bounds);
    let hypotheses = targets.len();
    let mut pairs: Vec<(i64, i64)> = targets.iter().map(|t| (t.om0, t.ox0)).collect();
    pairs.sort_unstable();
    pairs.dedup();

    let matches = crate::seed::search(&raw, &pairs, bounds, conf, seeds, progress);
    if matches.is_empty() {
        return Err(GuetError::Inconsistent);
    }
    let posterior = build_posterior(input, conf, observations, bounds, hypotheses, |t| {
        let n = matches
            .iter()
            .filter(|m| {
                (m.om0, m.ox0) == (t.om0, t.ox0)
                    && (m.tmin.0..=m.tmin.1).contains(&t.tmin)
                    && (m.tmax.0..=m.tmax.1).contains(&t.tmax)
            })
            .count();
        (n > 0).then_some(n as f64)
    })?;
    Ok(ExactPosterior { posterior, matches })
}

/// Readings a synthetic day would display at the given round counts (one per count).
pub fn readings_from_day(day: &SimulatedDay, rounds: &[usize], blocks: i64) -> Vec<Reading> {
    rounds
        .iter()
        .map(|&n| {
            let (min, max) = displayed_range(&day.target, day.path[n.min(MAX_ROUNDS)], blocks);
            Reading {
                pct: percent_for_rounds(n),
                min,
                max,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pastebin_input() -> GuetInput {
        let lines = [
            (33, 2047, 2749),
            (38, 2047, 2722),
            (42, 2064, 2722),
            (46, 2064, 2705),
            (50, 2076, 2694),
            (54, 2076, 2681),
            (58, 2089, 2650),
            (63, 2089, 2629),
            (67, 2089, 2607),
            (71, 2089, 2595),
            (75, 2089, 2556),
            (79, 2089, 2543),
            (83, 2089, 2519),
            (88, 2089, 2503),
            (92, 2089, 2477),
            (96, 2089, 2434),
            (100, 2089, 2361),
        ];
        GuetInput {
            day: 14,
            future: false,
            mode: AttackMode::Normal,
            readings: lines
                .iter()
                .map(|&(pct, min, max)| Reading { pct, min, max })
                .collect(),
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
        }
    }

    #[test]
    fn test_windows_match_displayed_rounding() {
        for t in [1500_i64, 2089, 2361, 7919] {
            for m in (0..30_000).step_by(7) {
                let target = HiddenTarget {
                    tmin: t,
                    tmax: t,
                    om0: 0,
                    ox0: 0,
                };
                let (lo, hi) = displayed_range(&target, Offsets { min: m, max: m }, 1);
                let y = |v: i64| (v as f64 - 0.5, v as f64 + 0.5);
                assert!(min_side_window(t, y(lo)).contains(m), "min t={t} m={m}");
                assert!(max_side_window(t, y(hi)).contains(m), "max t={t} m={m}");
                assert!(!min_side_window(t, y(lo + 2)).contains(m));
                assert!(!max_side_window(t, y(hi + 2)).contains(m));
            }
        }
    }

    #[test]
    fn test_guided_step_probability_matches_forward_frequency() {
        let mut rng = Mt64::new(7);
        let start = Offsets {
            min: 4_000,
            max: 18_000,
        };
        let round = 10;
        let wm = Window {
            lo: 3_200,
            hi: 4_000,
        };
        let wx = Window {
            lo: 17_000,
            hi: 17_600,
        };
        let trials = 400_000;
        let hits = (0..trials)
            .filter(|_| {
                let next = step_offsets(start, round, 0, &mut rng);
                wm.contains(next.min) && wx.contains(next.max)
            })
            .count();
        let (p, next) = guided_step(start, round, 0, wm, wx, &mut rng);
        let freq = hits as f64 / trials as f64;
        assert!((p - freq).abs() < 0.005, "guided {p} vs forward {freq}");
        assert!(wm.contains(next.min) && wx.contains(next.max));
    }

    #[test]
    fn test_guided_step_handles_clamp_to_zero() {
        let mut rng = Mt64::new(3);
        let start = Offsets {
            min: 100,
            max: 5_000,
        };
        let wm = Window { lo: 0, hi: 0 };
        let wx = Window {
            lo: 5_000,
            hi: 5_000,
        };
        let (p, next) = guided_step(start, 20, 0, wm, wx, &mut rng);
        assert!(p > 0.0);
        assert_eq!(next, Offsets { min: 0, max: 5_000 });
    }

    #[test]
    fn test_observations_dedupe_and_conflicts() {
        let mut input = pastebin_input();
        input.readings.push(Reading {
            pct: 100,
            min: 2089,
            max: 2361,
        });
        assert_eq!(observations(&input).unwrap().len(), 17);

        input.readings.push(Reading {
            pct: 100,
            min: 2089,
            max: 2360,
        });
        assert!(matches!(
            observations(&input),
            Err(GuetError::ConflictingReadings(..))
        ));

        let bad_pct = GuetInput {
            readings: vec![Reading {
                pct: 40,
                min: 1,
                max: 2,
            }],
            ..pastebin_input()
        };
        assert_eq!(observations(&bad_pct), Err(GuetError::InvalidPercent(40)));
    }

    #[test]
    fn test_pastebin_posterior_is_narrower_and_matches_plausible_pairs() {
        let input = pastebin_input();
        let opts = InferenceOptions {
            particles: 256,
            ..Default::default()
        };
        let post = infer(&input, &opts).unwrap();

        let total: f64 = post.attack.iter().map(|&(_, p)| p).sum();
        assert!((total - 1.0).abs() < 1e-9);
        let (lo, hi) = post.support();
        assert!(lo >= 2089 && hi <= 2361, "support {lo}-{hi}");

        let (q_lo, q_hi) = post.central_interval(0.95);
        assert!(q_hi - q_lo < 2361 - 2089);

        // The pastebin keeps (3,25)…(6,22) by hand.
        let pairs = post.offset_pairs();
        let mass: f64 = pairs
            .iter()
            .filter(|((om0, _), _)| (3..=6).contains(om0))
            .map(|(_, p)| p)
            .sum();
        assert!(mass > 0.95, "pairs {pairs:?}");
    }

    #[test]
    fn test_impossible_readings_are_rejected() {
        let mut input = pastebin_input();
        // Min going down while citizens are added cannot happen.
        input.readings[16] = Reading {
            pct: 100,
            min: 2000,
            max: 2361,
        };
        assert_eq!(
            infer(
                &input,
                &InferenceOptions {
                    particles: 64,
                    ..Default::default()
                }
            )
            .unwrap_err(),
            GuetError::Inconsistent
        );
    }

    #[test]
    fn test_synthetic_day_is_recovered() {
        let conf = EstimConf::default();
        let mut rng = Mt64::new(2024);
        let rounds: Vec<usize> = (8..=24).collect();
        let opts = InferenceOptions {
            particles: 128,
            ..Default::default()
        };
        for _ in 0..3 {
            let day = simulate_day(14, AttackMode::Normal, &conf, &mut rng);
            let input = GuetInput {
                day: 14,
                future: false,
                mode: AttackMode::Normal,
                readings: readings_from_day(&day, &rounds, 1),
                planner: Vec::new(),
                soul_factor: 1.0,
                planner_soul_factor: None,
            };
            let post = infer(&input, &opts).unwrap();
            let (lo, hi) = post.support();
            assert!(
                lo <= day.attack && day.attack <= hi,
                "attack {} outside {lo}-{hi}",
                day.attack
            );
        }
    }

    #[test]
    fn test_synthetic_future_day_is_recovered() {
        let conf = EstimConf::default();
        let mut rng = Mt64::new(99);
        let rounds: Vec<usize> = (0..=12).collect();
        let opts = InferenceOptions {
            particles: 128,
            ..Default::default()
        };
        let day = simulate_day(18, AttackMode::Normal, &conf, &mut rng);
        let input = GuetInput {
            day: 17,
            future: true,
            mode: AttackMode::Normal,
            readings: readings_from_day(&day, &rounds, future_blocks(18)),
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
        };
        let post = infer(&input, &opts).unwrap();
        let (lo, hi) = post.support();
        assert!(
            lo <= day.attack && day.attack <= hi,
            "attack {} outside {lo}-{hi}",
            day.attack
        );
    }

    #[test]
    fn test_planner_readings_sharpen_today() {
        let conf = EstimConf::default();
        let mut rng = Mt64::new(18);
        let opts = InferenceOptions {
            particles: 128,
            ..Default::default()
        };
        let sim = simulate_day(18, AttackMode::Normal, &conf, &mut rng);
        let today: Vec<usize> = (8..=24).collect();
        let planner: Vec<usize> = (0..=24).collect();
        let mut input = GuetInput {
            day: 18,
            future: false,
            mode: AttackMode::Normal,
            readings: readings_from_day(&sim, &today, 1),
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
        };
        let alone = infer(&input, &opts).unwrap();
        input.planner = readings_from_day(&sim, &planner, future_blocks(18));
        let both = infer(&input, &opts).unwrap();

        let (lo, hi) = both.support();
        assert!(lo <= sim.attack && sim.attack <= hi);
        let width = |p: &Posterior| p.support().1 - p.support().0;
        assert!(width(&both) <= width(&alone));
    }

    /// Real town, J14 (J13 planner + J14 readings); the gazette reported an attack of 2362.
    #[test]
    fn test_real_day_14_attack_is_inside_the_50_percent_interval() {
        let text = include_str!("../tests/data/j14_real_attack_2362.txt");
        let input = crate::parse::parse_text(text)
            .into_input(&crate::parse::InputOverrides::default())
            .unwrap();
        assert_eq!((input.day, input.future), (14, false));
        let post = infer(
            &input,
            &InferenceOptions {
                particles: 512,
                ..Default::default()
            },
        )
        .unwrap();

        let (lo, hi) = post.central_interval(0.5);
        assert!(lo <= 2362 && 2362 <= hi, "50% interval {lo}-{hi}");
        assert_eq!(post.offset_pairs()[0].0, (11, 17));
    }

    /// Exact search over a small window around the seed a full 2^32 sweep found.
    fn exact_on_fixture(text: &str, seeds: std::ops::RangeInclusive<u32>) -> ExactPosterior {
        let input = crate::parse::parse_text(text)
            .into_input(&crate::parse::InputOverrides::default())
            .unwrap();
        let progress = std::sync::atomic::AtomicU64::new(0);
        infer_exact(&input, &EstimConf::default(), seeds, &progress).unwrap()
    }

    #[test]
    fn test_real_day_14_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j14_real_attack_2362.txt");
        let exact = exact_on_fixture(text, 0x471c_0000..=0x471c_ffff);
        assert_eq!(exact.matches.len(), 1);
        let m = exact.matches[0];
        assert_eq!((m.seed, m.om0, m.ox0), (0x471c_9b8d, 11, 17));
        assert_eq!((m.tmin, m.tmax), ((2276, 2276), (2512, 2512)));
        assert_eq!(exact.posterior.support(), (2351, 2368));
    }

    #[test]
    fn test_real_day_15_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j15_real_attack_2587.txt");
        let exact = exact_on_fixture(text, 0x123f_0000..=0x123f_ffff);
        assert_eq!(exact.matches.len(), 1);
        let m = exact.matches[0];
        assert_eq!((m.seed, m.om0, m.ox0), (0x123f_81e5, 4, 24));
        assert_eq!((m.tmin, m.tmax), ((2518, 2518), (2777, 2777)));
        // Same range as the reference simulator; the gazette reported 2587.
        assert_eq!(exact.posterior.support(), (2582, 2597));
    }

    #[test]
    fn test_real_day_17_with_a_red_soul_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j17_real_attack_4115_red_soul.txt");
        let exact = exact_on_fixture(text, 0x9e76_0000..=0x9e76_ffff);
        assert_eq!(exact.matches.len(), 1);
        let m = exact.matches[0];
        assert_eq!((m.seed, m.om0, m.ox0), (0x9e76_c676, 11, 10));
        assert_eq!((m.tmin, m.tmax), ((3759, 3759), (4056, 4056)));
        // Stored attack before the ×1.04 red-soul factor; the gazette reported 4115 = round(3957 × 1.04).
        let (lo, hi) = exact.posterior.support();
        assert_eq!((lo, hi), (3949, 3971));
        let night = |v: i64| (v as f64 * 1.04).round() as i64;
        assert!(night(lo) <= 4115 && 4115 <= night(hi));
    }

    #[test]
    fn test_exact_search_recovers_a_synthetic_php_day() {
        let conf = EstimConf::default();
        let mut rng = Mt64::new(5);
        let sim = simulate_day(16, AttackMode::Normal, &conf, &mut rng);
        let seed = 0x00c0_ffee;
        let path = crate::seed::php_path(seed, sim.target.om0, sim.target.ox0, &conf);
        let display = |n: usize| {
            let (om, ox) = path[n];
            let (tmin, tmax) = (sim.target.tmin as f64, sim.target.tmax as f64);
            (
                (tmin - tmin * om / 100.0).round() as i64,
                (tmax + tmax * ox / 100.0).round() as i64,
            )
        };
        let readings = (8..=24)
            .map(|n| {
                let (min, max) = display(n);
                Reading {
                    pct: percent_for_rounds(n),
                    min,
                    max,
                }
            })
            .collect();
        let input = GuetInput {
            day: 16,
            future: false,
            mode: AttackMode::Normal,
            readings,
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
        };
        let progress = std::sync::atomic::AtomicU64::new(0);
        let exact = infer_exact(&input, &conf, seed - 20_000..=seed + 20_000, &progress).unwrap();
        assert!(exact.matches.iter().any(|m| m.seed == seed));
        let (lo, hi) = exact.posterior.support();
        assert!(
            lo <= sim.attack && sim.attack <= hi,
            "{} outside {lo}-{hi}",
            sim.attack
        );
    }

    #[test]
    fn test_planner_mismatch_is_reported() {
        let input = GuetInput {
            day: 18,
            future: false,
            mode: AttackMode::Normal,
            readings: vec![Reading {
                pct: 33,
                min: 4178,
                max: 5145,
            }],
            // Blocks of 20: 4200 covers raw 4200-4219, above today's 4178.
            planner: vec![Reading {
                pct: 33,
                min: 4200,
                max: 5160,
            }],
            soul_factor: 1.0,
            planner_soul_factor: None,
        };
        assert_eq!(
            infer(&input, &InferenceOptions::default()).unwrap_err(),
            GuetError::PlannerMismatch(33)
        );
    }

    /// Calibration check (slow; run with `cargo test --release -p guet_lib -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn calibration_of_credible_intervals() {
        let conf = EstimConf::default();
        let opts = InferenceOptions {
            particles: 512,
            ..Default::default()
        };
        // (day, mode, future, rounds, with yesterday's J+1 readings)
        let cases: [(i64, AttackMode, bool, Vec<usize>, bool); 5] = [
            (14, AttackMode::Normal, false, (8..=24).collect(), false),
            (35, AttackMode::Normal, false, (8..=24).collect(), false),
            (10, AttackMode::Hard, false, (8..=24).collect(), false),
            (17, AttackMode::Normal, true, (0..=12).collect(), false),
            (18, AttackMode::Normal, false, (8..=24).collect(), true),
        ];
        let per_case = 50;
        for (day, mode, future, rounds, with_planner) in cases {
            let mut rng = Mt64::new(day as u64 * 31 + future as u64);
            let estimated_day = day + i64::from(future);
            let blocks = if future {
                future_blocks(estimated_day)
            } else {
                1
            };
            let (mut in50, mut in80, mut in95) = (0, 0, 0);
            let (mut err_post, mut err_naive) = (0.0, 0.0);
            for _ in 0..per_case {
                let sim = simulate_day(estimated_day, mode, &conf, &mut rng);
                let readings = readings_from_day(&sim, &rounds, blocks);
                let last = *readings.last().unwrap();
                let planner = if with_planner {
                    let all: Vec<usize> = (0..=MAX_ROUNDS).collect();
                    readings_from_day(&sim, &all, future_blocks(day))
                } else {
                    Vec::new()
                };
                let input = GuetInput {
                    day,
                    future,
                    mode,
                    readings,
                    planner,
                    soul_factor: 1.0,
                    planner_soul_factor: None,
                };
                let post = infer(&input, &opts).unwrap();
                let inside = |(lo, hi): (i64, i64)| lo <= sim.attack && sim.attack <= hi;
                in50 += inside(post.central_interval(0.5)) as u32;
                in80 += inside(post.central_interval(0.8)) as u32;
                in95 += inside(post.central_interval(0.95)) as u32;
                err_post += (post.median() - sim.attack).abs() as f64;
                err_naive += ((last.min + last.max) as f64 / 2.0 - sim.attack as f64).abs();
            }
            let n = per_case as f64;
            println!(
                "day {day} {mode:?} future={future} planner={with_planner}: cover50={:.2} cover80={:.2} cover95={:.2} \
                 median err={:.1} vs last-midpoint err={:.1}",
                in50 as f64 / n,
                in80 as f64 / n,
                in95 as f64 / n,
                err_post / n,
                err_naive / n
            );
            assert!(in80 as f64 / n >= 0.65, "80% interval under-covers");
            assert!(in95 as f64 / n >= 0.85, "95% interval under-covers");
            assert!(err_post < err_naive);
        }
    }
}
