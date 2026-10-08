//! Exact inversion of the watchtower: every reading of a day replays the same seeded offset path,
//! so [`estimate`] finds the seeds reproducing them all (see [`crate::seed`]) and the attack
//! range of the hidden targets they allow.

use crate::engine::{
    AttackMode, DayModel, EstimConf, HiddenTarget, MAX_ROUNDS, factors_differ, future_blocks,
    rounds_for_percent,
};
use crate::seed::{Window, WindowMatch};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::ops::RangeInclusive;
use std::sync::atomic::{AtomicBool, AtomicU64};

/// One watchtower reading as displayed in game: `[pct%] min - max`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reading {
    pub pct: u32,
    pub min: i64,
    pub max: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EstimationInput {
    /// Current town day.
    pub day: i64,
    /// J+1 estimate (watchtower upgrade), targeting `day + 1`.
    pub future: bool,
    pub mode: AttackMode,
    pub readings: Vec<Reading>,
    /// J+1 readings taken the day before for this same attack (`future == false` only).
    #[serde(default)]
    pub planner: Vec<Reading>,
    /// Red-soul factor of today's readings, also applied to the night attack.
    #[serde(default = "unit_factor")]
    pub soul_factor: f64,
    /// Red-soul factor when the planner readings were taken (defaults to `soul_factor`).
    #[serde(default)]
    pub planner_soul_factor: Option<f64>,
    /// Red-soul factor of the night attack when it differs from the readings' (a J+1 estimate
    /// displayed with yesterday's penalty, tonight's attack using today's).
    #[serde(default)]
    pub attack_soul_factor: Option<f64>,
}

fn unit_factor() -> f64 {
    1.0
}

impl Default for EstimationInput {
    fn default() -> Self {
        EstimationInput {
            day: 0,
            future: false,
            mode: AttackMode::Normal,
            readings: Vec::new(),
            planner: Vec::new(),
            soul_factor: 1.0,
            planner_soul_factor: None,
            attack_soul_factor: None,
        }
    }
}

impl EstimationInput {
    #[must_use]
    pub fn estimated_day(&self) -> i64 {
        self.day + i64::from(self.future)
    }

    #[must_use]
    pub fn blocks(&self) -> i64 {
        if self.future {
            future_blocks(self.estimated_day())
        } else {
            1
        }
    }

    #[must_use]
    pub fn has_red_souls(&self) -> bool {
        factors_differ(self.soul_factor, 1.0) || factors_differ(self.night_soul_factor(), 1.0)
    }

    /// Red-soul factor of the night attack (`NightlyHandler`: `round(zombies * soulFactor)`).
    #[must_use]
    pub fn night_soul_factor(&self) -> f64 {
        self.attack_soul_factor.unwrap_or(self.soul_factor)
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
pub enum EstimationError {
    MissingDay,
    InvalidDay(i64),
    NoReadings,
    InvalidPercent(u32),
    InvertedRange(Reading),
    ConflictingReadings(Reading, Reading),
    /// Today's reading and yesterday's J+1 reading at this percentage cannot share one path.
    PlannerMismatch(u32),
    Inconsistent,
    Cancelled,
}

impl fmt::Display for EstimationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EstimationError::MissingDay => write!(f, "jour manquant : précisez `jour: N`."),
            EstimationError::InvalidDay(day) => {
                write!(f, "jour invalide ({day}), il doit être ≥ 1.")
            }
            EstimationError::NoReadings => write!(
                f,
                "aucun relevé reconnu (format attendu : `33% 2047 - 2749`)."
            ),
            EstimationError::InvalidPercent(pct) => {
                write!(
                    f,
                    "pourcentage {pct}% impossible : la tour affiche n/24 arrondi."
                )
            }
            EstimationError::InvertedRange(r) => {
                write!(f, "relevé {}% invalide : {} > {}.", r.pct, r.min, r.max)
            }
            EstimationError::ConflictingReadings(a, b) => write!(
                f,
                "relevés contradictoires à {}% : {} - {} et {} - {}.",
                a.pct, a.min, a.max, b.min, b.max
            ),
            EstimationError::PlannerMismatch(pct) => write!(
                f,
                "le relevé du jour et le relevé J+1 de la veille à {pct}% sont incompatibles."
            ),
            EstimationError::Inconsistent => write!(
                f,
                "aucune seed ne reproduit ces relevés. Vérifiez le jour, le mode, l'option J+1 \
                 et les âmes rouges (les événements ne sont pas modélisés)."
            ),
            EstimationError::Cancelled => write!(f, "recherche annulée."),
        }
    }
}

impl std::error::Error for EstimationError {}

/// Validates today's readings, maps percentages to round counts and drops duplicates.
///
/// # Errors
///
/// [`EstimationError::InvalidDay`], [`EstimationError::NoReadings`], or a per-reading error
/// ([`EstimationError::InvertedRange`], [`EstimationError::InvalidPercent`],
/// [`EstimationError::ConflictingReadings`]).
pub fn observations(input: &EstimationInput) -> Result<Vec<Observation>, EstimationError> {
    if input.day < 1 {
        return Err(EstimationError::InvalidDay(input.day));
    }
    if input.readings.is_empty() {
        return Err(EstimationError::NoReadings);
    }
    validate(&input.readings)
}

fn validate(readings: &[Reading]) -> Result<Vec<Observation>, EstimationError> {
    let mut by_rounds = [None::<Reading>; MAX_ROUNDS + 1];
    for &reading in readings {
        if reading.min > reading.max {
            return Err(EstimationError::InvertedRange(reading));
        }
        let rounds =
            rounds_for_percent(reading.pct).ok_or(EstimationError::InvalidPercent(reading.pct))?;
        match by_rounds[rounds] {
            Some(prev) if (prev.min, prev.max) != (reading.min, reading.max) => {
                return Err(EstimationError::ConflictingReadings(prev, reading));
            }
            Some(_) => {}
            None => by_rounds[rounds] = Some(reading),
        }
    }
    Ok(by_rounds
        .iter()
        .enumerate()
        .filter_map(|(rounds, r)| {
            r.map(|r| Observation {
                rounds,
                pct: r.pct,
                min: r.min,
                max: r.max,
            })
        })
        .collect())
}

/// Tolerance on the unscaled bounds: PHP's float path may round exact ties either way.
const BOUND_EPS: f64 = 1e-6;

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
    input: &EstimationInput,
    observations: &[Observation],
) -> Result<Vec<RawObservation>, EstimationError> {
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
                if r.min_y.0 > r.min_y.1 + BOUND_EPS || r.max_y.0 > r.max_y.1 + BOUND_EPS {
                    return Err(EstimationError::PlannerMismatch(p.pct));
                }
            }
            None => raw.push(extra),
        }
    }
    raw.sort_by_key(|r| r.rounds);
    Ok(raw)
}

/// Calls `f(value, target)` for every generator draw whose targets lie in `[t_lo, t_hi]`, once
/// per run of consecutive shifts giving the same targets.
fn for_each_draw(
    input: &EstimationInput,
    conf: &EstimConf,
    (t_lo, t_hi): (i64, i64),
    mut f: impl FnMut(i64, HiddenTarget),
) {
    let model = DayModel::new(input.estimated_day(), input.mode, conf);
    let (value_lo, value_hi) = model.range;

    // tmin <= value <= tmax.
    for value in t_lo.max(value_lo)..=t_hi.min(value_hi) {
        let mut emit = |(tmin, tmax): (i64, i64)| {
            if tmin < t_lo || tmax > t_hi {
                return;
            }
            for (om0, ox0) in model.offset_pairs(tmin, tmax) {
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
        };
        let mut run = model.shifted_targets(value, 0);
        for shift_raw in 1..=model.shift_raw_max {
            let targets = model.shifted_targets(value, shift_raw);
            if targets != run {
                emit(run);
                run = targets;
            }
        }
        emit(run);
    }
}

/// Raw observations and the outer `[tmin, tmax]` bounds they allow.
fn prepare(input: &EstimationInput) -> Result<(Vec<RawObservation>, (i64, i64)), EstimationError> {
    let raw = raw_observations(input, &observations(input)?)?;

    // Offsets are non-negative: every displayed min is <= tmin and every displayed max >= tmax.
    let t_lo = raw
        .iter()
        .map(|r| (r.min_y.0 - BOUND_EPS).ceil() as i64)
        .max()
        .unwrap_or(0);
    let t_hi = raw
        .iter()
        .map(|r| (r.max_y.1 + BOUND_EPS).floor() as i64)
        .min()
        .unwrap_or(0);
    if t_lo > t_hi {
        return Err(EstimationError::Inconsistent);
    }
    Ok((raw, (t_lo, t_hi)))
}

/// Initial offset pairs the generator can produce within the outer bounds.
fn candidate_pairs(
    input: &EstimationInput,
    conf: &EstimConf,
    bounds: (i64, i64),
) -> Vec<(i64, i64)> {
    let mut pairs = BTreeSet::new();
    for_each_draw(input, conf, bounds, |_, t| {
        pairs.insert((t.om0, t.ox0));
    });
    pairs.into_iter().collect()
}

/// `(om0, ox0)` -> `[(tmin, tmax, attack_lo, attack_hi)]` for every hidden target in `bounds`.
type DrawTable = HashMap<(i64, i64), Vec<(i64, i64, i64, i64)>>;

/// Enumerates the generator once for all windows.
fn draw_table(input: &EstimationInput, conf: &EstimConf, bounds: (i64, i64)) -> DrawTable {
    let mut attacks: HashMap<HiddenTarget, (i64, i64)> = HashMap::new();
    for_each_draw(input, conf, bounds, |value, t| {
        // Hard mode redraws the attack anywhere in [tmin, tmax].
        let (lo, hi) = if input.mode == AttackMode::Hard {
            (t.tmin, t.tmax)
        } else {
            (value, value)
        };
        attacks
            .entry(t)
            .and_modify(|(a, b)| (*a, *b) = ((*a).min(lo), (*b).max(hi)))
            .or_insert((lo, hi));
    });
    let mut table = DrawTable::new();
    for (t, (lo, hi)) in attacks {
        table
            .entry((t.om0, t.ox0))
            .or_default()
            .push((t.tmin, t.tmax, lo, hi));
    }
    table
}

fn window_attack(table: &DrawTable, w: &Window) -> Option<(i64, i64)> {
    table
        .get(&(w.om0, w.ox0))?
        .iter()
        .filter(|&&(tmin, tmax, _, _)| {
            (w.tmin.0..=w.tmin.1).contains(&tmin) && (w.tmax.0..=w.tmax.1).contains(&tmax)
        })
        .map(|&(_, _, lo, hi)| (lo, hi))
        .reduce(|(a, b), (lo, hi)| (a.min(lo), b.max(hi)))
}

/// Attack range the generator can draw within one window (before the red-soul factor).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowEstimate {
    pub window: Window,
    pub seeds: u64,
    pub attack: (i64, i64),
}

#[derive(Debug, Clone)]
pub struct Estimate {
    pub observations: Vec<Observation>,
    /// Never empty, sorted by window.
    pub windows: Vec<WindowEstimate>,
}

impl Estimate {
    /// Number of compatible (seed, initial pair) runs.
    #[must_use]
    pub fn seed_count(&self) -> u64 {
        self.windows.iter().map(|w| w.seeds).sum()
    }

    /// Union of the attack ranges of every window (before the red-soul factor).
    #[must_use]
    pub fn attack(&self) -> (i64, i64) {
        self.windows.iter().fold((i64::MAX, i64::MIN), |(a, b), w| {
            (a.min(w.attack.0), b.max(w.attack.1))
        })
    }

    #[must_use]
    pub fn summary(&self) -> Summary {
        Summary {
            attack: self.attack(),
            seeds: self.seed_count(),
        }
    }
}

/// What the result needs from an [`Estimate`]: the union of its attack ranges and its number of
/// runs. A window's attack range does not depend on the other windows, so the summaries of
/// slices of the seeds [`merge`](Summary::merge) into the summary of the whole search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    /// Before the red-soul factor.
    pub attack: (i64, i64),
    pub seeds: u64,
}

impl Summary {
    #[must_use]
    pub fn merge(self, other: Self) -> Self {
        Self {
            attack: (
                self.attack.0.min(other.attack.0),
                self.attack.1.max(other.attack.1),
            ),
            seeds: self.seeds + other.seeds,
        }
    }
}

/// Checks that the readings are valid, consistent and reachable by the generator, without
/// searching any seed.
///
/// # Errors
///
/// Any validation error of [`observations`], [`EstimationError::PlannerMismatch`], or
/// [`EstimationError::Inconsistent`] when no hidden target fits the readings' bounds.
pub fn check_input(input: &EstimationInput, conf: &EstimConf) -> Result<(), EstimationError> {
    let (_, bounds) = prepare(input)?;
    if candidate_pairs(input, conf, bounds).is_empty() {
        return Err(EstimationError::Inconsistent);
    }
    Ok(())
}

/// Windows of the seeds of `seeds` whose offset path replays every reading (possibly none).
/// `progress` counts processed seeds; setting `cancel` stops the search early.
///
/// # Errors
///
/// Any validation error of [`observations`], [`EstimationError::PlannerMismatch`], or
/// [`EstimationError::Cancelled`] once `cancel` is set.
pub fn search_seeds(
    input: &EstimationInput,
    conf: &EstimConf,
    seeds: RangeInclusive<u32>,
    progress: &AtomicU64,
    cancel: &AtomicBool,
) -> Result<Vec<WindowMatch>, EstimationError> {
    let (raw, bounds) = prepare(input)?;
    let pairs = candidate_pairs(input, conf, bounds);
    crate::seed::search(&raw, &pairs, bounds, conf, seeds, progress, cancel)
        .ok_or(EstimationError::Cancelled)
}

/// Attack range of the windows gathered by [`search_seeds`], typically over slices of the seed
/// space.
///
/// # Errors
///
/// Any validation error of [`observations`], or [`EstimationError::Inconsistent`] when no window
/// is left.
pub fn finish(
    input: &EstimationInput,
    conf: &EstimConf,
    matches: Vec<WindowMatch>,
) -> Result<Estimate, EstimationError> {
    let observations = observations(input)?;
    let mut seeds: BTreeMap<Window, u64> = BTreeMap::new();
    for m in matches {
        *seeds.entry(m.window).or_default() += m.seeds;
    }
    if seeds.is_empty() {
        return Err(EstimationError::Inconsistent);
    }
    let bounds = seeds.keys().fold((i64::MAX, i64::MIN), |(lo, hi), w| {
        (lo.min(w.tmin.0), hi.max(w.tmax.1))
    });
    let table = draw_table(input, conf, bounds);
    let windows: Vec<WindowEstimate> = seeds
        .into_iter()
        .filter_map(|(window, seeds)| {
            Some(WindowEstimate {
                attack: window_attack(&table, &window)?,
                window,
                seeds,
            })
        })
        .collect();
    if windows.is_empty() {
        return Err(EstimationError::Inconsistent);
    }
    Ok(Estimate {
        observations,
        windows,
    })
}

/// [`finish`] reduced to its [`Summary`], for a slice of the seeds: `None` when none of its
/// windows is left.
///
/// # Errors
///
/// Any validation error of [`observations`].
pub fn summarize(
    input: &EstimationInput,
    conf: &EstimConf,
    matches: Vec<WindowMatch>,
) -> Result<Option<Summary>, EstimationError> {
    match finish(input, conf, matches) {
        Ok(estimate) => Ok(Some(estimate.summary())),
        Err(EstimationError::Inconsistent) => Ok(None),
        Err(e) => Err(e),
    }
}

/// [`search_seeds`] then [`finish`]; `seeds` is normally `0..=u32::MAX`.
///
/// # Errors
///
/// Any validation error of [`observations`], [`EstimationError::PlannerMismatch`], or
/// [`EstimationError::Inconsistent`] when no seed of `seeds` replays the readings.
pub fn estimate(
    input: &EstimationInput,
    conf: &EstimConf,
    seeds: RangeInclusive<u32>,
    progress: &AtomicU64,
) -> Result<Estimate, EstimationError> {
    let matches = search_seeds(input, conf, seeds, progress, &AtomicBool::new(false))?;
    finish(input, conf, matches)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{displayed_range, percent_for_rounds};
    use crate::parse::{InputOverrides, parse_text};
    use crate::seed::php_path;
    use rand::RngExt;
    use rand_mt::Mt64;

    fn pastebin_input() -> EstimationInput {
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
        EstimationInput {
            day: 14,
            readings: lines
                .iter()
                .map(|&(pct, min, max)| Reading { pct, min, max })
                .collect(),
            ..EstimationInput::default()
        }
    }

    fn search(
        input: &EstimationInput,
        seeds: RangeInclusive<u32>,
    ) -> Result<Estimate, EstimationError> {
        estimate(input, &EstimConf::default(), seeds, &AtomicU64::new(0))
    }

    fn fixture_input(text: &str) -> EstimationInput {
        parse_text(text)
            .into_input(&InputOverrides::default())
            .unwrap()
    }

    /// The single window found around `seed` (found by a full 2^32 sweep), and `seed` alone
    /// replays the readings.
    fn pinned_window(text: &str, seeds: RangeInclusive<u32>, seed: u32) -> Estimate {
        let input = fixture_input(text);
        let est = search(&input, seeds).unwrap();
        assert_eq!(est.seed_count(), 1);
        assert_eq!(search(&input, seed..=seed).unwrap().windows, est.windows);
        est
    }

    fn window(om0: i64, ox0: i64, tmin: i64, tmax: i64) -> Window {
        Window {
            om0,
            ox0,
            tmin: (tmin, tmin),
            tmax: (tmax, tmax),
        }
    }

    /// A random generator draw of `day` (uniform value, no reroll: only the target matters).
    fn random_draw(day: i64, rng: &mut Mt64) -> (i64, HiddenTarget) {
        let model = DayModel::new(day, AttackMode::Normal, &EstimConf::default());
        let value = rng.random_range(model.range.0..=model.range.1);
        let shift_raw = rng.random_range(0..=model.shift_raw_max);
        let off_raw = rng.random_range(model.off_raw.0..=model.off_raw.1);
        (value, model.generate_target(value, shift_raw, off_raw))
    }

    /// Readings the game displays for `target` whose path is seeded with `seed`.
    fn php_readings(
        target: &HiddenTarget,
        seed: u32,
        rounds: RangeInclusive<usize>,
        blocks: i64,
        soul: f64,
    ) -> Vec<Reading> {
        let path = php_path(seed, target.om0, target.ox0, &EstimConf::default());
        rounds
            .map(|n| {
                let (om, ox) = path[n];
                let (min, max) = displayed_range(target, om, ox, blocks, soul);
                Reading {
                    pct: percent_for_rounds(n),
                    min,
                    max,
                }
            })
            .collect()
    }

    /// `seed` alone replays the readings, and the windows around it cover `value`.
    fn assert_recovered(input: &EstimationInput, seed: u32, value: i64) {
        search(input, seed..=seed).unwrap();
        let (lo, hi) = search(input, seed - 20_000..=seed + 20_000)
            .unwrap()
            .attack();
        assert!(lo <= value && value <= hi, "{value} outside {lo}-{hi}");
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
            Err(EstimationError::ConflictingReadings(..))
        ));

        let bad_pct = EstimationInput {
            readings: vec![Reading {
                pct: 40,
                min: 1,
                max: 2,
            }],
            ..pastebin_input()
        };
        assert_eq!(
            observations(&bad_pct),
            Err(EstimationError::InvalidPercent(40))
        );
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
            search(&input, 0..=50_000).unwrap_err(),
            EstimationError::Inconsistent
        );
    }

    #[test]
    fn test_planner_mismatch_is_reported() {
        let input = EstimationInput {
            day: 18,
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
            ..EstimationInput::default()
        };
        assert_eq!(
            search(&input, 0..=10).unwrap_err(),
            EstimationError::PlannerMismatch(33)
        );
    }

    /// Real town, J14 (J13 planner + J14 readings); the gazette reported an attack of 2362.
    #[test]
    fn test_real_day_14_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j14_real_attack_2362.txt");
        let est = pinned_window(text, 0x471c_0000..=0x471c_ffff, 0x471c_9b8d);
        assert_eq!(est.windows[0].window, window(11, 17, 2276, 2512));
        assert_eq!(est.attack(), (2351, 2368));
    }

    /// Real town, J15; same range as the reference simulator, the gazette reported 2587.
    #[test]
    fn test_real_day_15_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j15_real_attack_2587.txt");
        let est = pinned_window(text, 0x123f_0000..=0x123f_ffff, 0x123f_81e5);
        assert_eq!(est.windows[0].window, window(4, 24, 2518, 2777));
        assert_eq!(est.attack(), (2582, 2597));
    }

    /// Real town, J17 with one red soul; the gazette reported 4115 = round(3957 × 1.04).
    #[test]
    fn test_real_day_17_with_a_red_soul_seed_pins_the_attack() {
        let text = include_str!("../tests/data/j17_real_attack_4115_red_soul.txt");
        let est = pinned_window(text, 0x9e76_0000..=0x9e76_ffff, 0x9e76_c676);
        assert_eq!(est.windows[0].window, window(11, 10, 3759, 4056));
        let (lo, hi) = est.attack();
        assert_eq!((lo, hi), (3949, 3971));
        let night = |v: i64| (v as f64 * 1.04).round() as i64;
        assert!(night(lo) <= 4115 && 4115 <= night(hi));
    }

    #[test]
    fn test_lone_zero_percent_reading_skips_the_replay() {
        let input = EstimationInput {
            day: 16,
            future: true,
            readings: vec![Reading {
                pct: 0,
                min: 3460,
                max: 4640,
            }],
            ..EstimationInput::default()
        };
        let progress = AtomicU64::new(0);
        let est = estimate(&input, &EstimConf::default(), 0..=u32::MAX, &progress).unwrap();
        assert_eq!(progress.into_inner(), 1 << 32);
        assert!(est.windows.iter().all(|w| w.seeds == 1 << 32));
    }

    #[test]
    fn test_cancelled_search_stops() {
        let input = fixture_input(include_str!("../tests/data/j15_real_attack_2587.txt"));
        let progress = AtomicU64::new(0);
        let cancel = AtomicBool::new(true);
        let searched = search_seeds(
            &input,
            &EstimConf::default(),
            0..=u32::MAX,
            &progress,
            &cancel,
        );
        assert_eq!(searched.unwrap_err(), EstimationError::Cancelled);
        assert_eq!(progress.into_inner(), 0);
    }

    #[test]
    fn test_split_search_matches_single_search() {
        let input = fixture_input(include_str!("../tests/data/j15_real_attack_2587.txt"));
        let conf = EstimConf::default();
        let whole = search(&input, 0x123f_0000..=0x123f_ffff).unwrap();

        check_input(&input, &conf).unwrap();
        let mut matches = Vec::new();
        for slice in [0x123f_0000..=0x123f_7fff, 0x123f_8000..=0x123f_ffff] {
            let (progress, cancel) = (AtomicU64::new(0), AtomicBool::new(false));
            matches.extend(search_seeds(&input, &conf, slice, &progress, &cancel).unwrap());
        }
        let split = finish(&input, &conf, matches).unwrap();
        assert_eq!(split.windows, whole.windows);
        assert_eq!(split.attack(), (2582, 2597));
        assert_eq!(
            finish(&input, &conf, Vec::new()).unwrap_err(),
            EstimationError::Inconsistent
        );
    }

    #[test]
    fn test_slice_summaries_merge_into_the_whole_summary() {
        let input = fixture_input(include_str!("../tests/data/j15_real_attack_2587.txt"));
        let conf = EstimConf::default();
        let whole = search(&input, 0x123f_0000..=0x123f_ffff).unwrap();

        let mut merged: Option<Summary> = None;
        for slice in [
            0x123f_0000..=0x123f_3fff,
            0x123f_4000..=0x123f_7fff,
            0x123f_8000..=0x123f_bfff,
            0x123f_c000..=0x123f_ffff,
        ] {
            let (progress, cancel) = (AtomicU64::new(0), AtomicBool::new(false));
            let matches = search_seeds(&input, &conf, slice, &progress, &cancel).unwrap();
            if let Some(summary) = summarize(&input, &conf, matches).unwrap() {
                merged = Some(merged.map_or(summary, |m| m.merge(summary)));
            }
        }
        assert_eq!(merged, Some(whole.summary()));
        assert_eq!(summarize(&input, &conf, Vec::new()).unwrap(), None);
    }

    #[test]
    fn test_synthetic_day_is_recovered() {
        let mut rng = Mt64::new(5);
        let seed = 0x00c0_ffee;
        let (value, target) = random_draw(16, &mut rng);
        let input = EstimationInput {
            day: 16,
            readings: php_readings(&target, seed, 8..=MAX_ROUNDS, 1, 1.0),
            ..EstimationInput::default()
        };
        assert_recovered(&input, seed, value);
    }

    #[test]
    fn test_synthetic_future_day_with_blocks_is_recovered() {
        let mut rng = Mt64::new(18);
        let seed = 0x0bad_cafe;
        let (value, target) = random_draw(18, &mut rng);
        let input = EstimationInput {
            day: 17,
            future: true,
            readings: php_readings(&target, seed, 0..=12, future_blocks(18), 1.0),
            ..EstimationInput::default()
        };
        assert_recovered(&input, seed, value);
    }

    #[test]
    fn test_synthetic_day_with_planner_and_red_soul_is_recovered() {
        let mut rng = Mt64::new(17);
        let seed = 0x1234_5678;
        let (value, target) = random_draw(17, &mut rng);
        let input = EstimationInput {
            day: 17,
            readings: php_readings(&target, seed, 8..=MAX_ROUNDS, 1, 1.04),
            planner: php_readings(&target, seed, 0..=MAX_ROUNDS, future_blocks(17), 1.04),
            soul_factor: 1.04,
            ..EstimationInput::default()
        };
        assert_recovered(&input, seed, value);
    }
}
