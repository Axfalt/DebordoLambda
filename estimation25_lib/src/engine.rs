//! Emulation of the MyHordes watchtower estimation engine.
//!
//! Mirrors `PrepareZombieAttackEstimationAction` (hidden target generation) and
//! `EstimateZombieAttackAction` (per-citizen offset shrinking and displayed range).
//! Offsets are kept in thousandths of a percent: the game only ever subtracts
//! `mt_rand(a, b) / 1000` from integer starting offsets, so integers are exact.

use rand::RngExt;
use rand_mt::Mt64;
use serde::{Deserialize, Serialize};

/// Watchtower quality caps at 24 weighted citizens: `calculate_offsets` runs at most 24 rounds.
pub const MAX_ROUNDS: usize = 24;

/// Offsets are stored in thousandths of a percent (1000 = 1 %).
pub const MILLI: i64 = 1000;

/// `TownSetting::OptFeatureAttacks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AttackMode {
    #[default]
    Normal,
    Hard,
    Easy,
}

impl AttackMode {
    pub fn label(self) -> &'static str {
        match self {
            AttackMode::Normal => "normal",
            AttackMode::Hard => "hard",
            AttackMode::Easy => "easy",
        }
    }

    fn max_ratio(self) -> f64 {
        match self {
            AttackMode::Hard => 3.1,
            AttackMode::Easy => 0.75,
            AttackMode::Normal => 1.1,
        }
    }
}

/// `estimation.*` settings from `config/app/rules.yml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EstimConf {
    pub shift: i64,
    pub spread: i64,
    pub variance: i64,
    pub offset_min: i64,
    pub offset_max: i64,
}

impl Default for EstimConf {
    fn default() -> Self {
        EstimConf {
            shift: 10,
            spread: 10,
            variance: 48,
            offset_min: 15,
            offset_max: 36,
        }
    }
}

impl EstimConf {
    /// `calculate_offsets` stops shrinking once `om + ox <= spread - shift` (in milli-percent).
    pub fn min_spread_milli(&self) -> i64 {
        (self.spread - self.shift) * MILLI
    }
}

/// Theoretical attack range of a day: `[min, max]` fed to `mt_rand`.
pub fn attack_range(day: i64, mode: AttackMode) -> (i64, i64) {
    let max_ratio = mode.max_ratio();
    let ratio_min = if day <= 3 { 0.75 } else { max_ratio };
    let ratio_max = if day <= 3 {
        if day <= 1 { 0.5 } else { 0.75 }
    } else {
        max_ratio
    };
    let min = (ratio_min * ((day - 1).max(1) as f64 * 0.75 + 2.5).powi(3)).round() as i64;
    let max = (ratio_max * (day as f64 * 0.75 + 3.5).powi(3)).round() as i64;
    (min, max)
}

pub fn day_factor(day: i64) -> f64 {
    match day {
        ..=15 => 1.0,
        16..=20 => 0.75,
        21..=30 => 0.5,
        31..=40 => 0.25,
        _ => 0.15,
    }
}

/// Inclusive range of `mt_rand` for the raw minimum offset.
pub fn off_raw_range(day: i64, conf: &EstimConf) -> (i64, i64) {
    let f = day_factor(day);
    (
        (f * (conf.offset_min - conf.shift) as f64).round() as i64,
        (f * (conf.offset_max - conf.shift) as f64).round() as i64,
    )
}

/// Upper bound of `mt_rand(0, shift * factor * 100)` (PHP truncates the float argument).
pub fn shift_raw_max(day: i64, conf: &EstimConf) -> i64 {
    (conf.shift as f64 * day_factor(day) * 100.0) as i64
}

/// Prior of the drawn attack value: one `mt_rand(min, max)`, redrawn once when above the midpoint.
#[derive(Debug, Clone, Copy)]
pub struct ValuePrior {
    pub min: i64,
    pub max: i64,
    mid: f64,
    low_weight: f64,
    high_weight: f64,
}

impl ValuePrior {
    pub fn new(day: i64, mode: AttackMode) -> Self {
        let (min, max) = attack_range(day, mode);
        let n = (max - min + 1) as f64;
        let mid = min as f64 + 0.5 * (max - min) as f64;
        let high = (min..=max).filter(|&v| v as f64 > mid).count() as f64;
        ValuePrior {
            min,
            max,
            mid,
            low_weight: (1.0 + high / n) / n,
            high_weight: (high / n) / n,
        }
    }

    pub fn weight(&self, value: i64) -> f64 {
        if value < self.min || value > self.max {
            0.0
        } else if value as f64 > self.mid {
            self.high_weight
        } else {
            self.low_weight
        }
    }

    pub fn sample(&self, rng: &mut Mt64) -> i64 {
        let value = rng.random_range(self.min..=self.max);
        if value as f64 > self.mid {
            rng.random_range(self.min..=self.max)
        } else {
            value
        }
    }
}

/// Hidden state of a day's estimation, as stored in `ZombieEstimation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct HiddenTarget {
    pub tmin: i64,
    pub tmax: i64,
    /// Initial offsets, in whole percents.
    pub om0: i64,
    pub ox0: i64,
}

/// `PrepareZombieAttackEstimationAction::deshift`.
fn deshift(
    value: i64,
    bound_min: i64,
    bound_max: i64,
    off_min: &mut f64,
    off_max: &mut f64,
) -> bool {
    let v = value as f64;
    let bound_min = (v - bound_min as f64) / v;
    let bound_max = (bound_max as f64 - v) / v;

    if *off_min > bound_min {
        *off_max += *off_min - bound_min;
        *off_min = bound_min;
        true
    } else if *off_max > bound_max {
        *off_min += *off_max - bound_max;
        *off_max = bound_max;
        true
    } else {
        false
    }
}

/// First stage of the generator: `(target_min, target_max)` from the drawn value and shift.
pub fn shifted_targets(
    day: i64,
    mode: AttackMode,
    conf: &EstimConf,
    value: i64,
    shift_raw: i64,
) -> (i64, i64) {
    let (min, max) = attack_range(day, mode);
    let f = day_factor(day);
    let mut shift_min = shift_raw as f64 / 10000.0;
    let mut shift_max = (f * conf.shift as f64 / 100.0) - shift_min;
    deshift(value, min, max, &mut shift_min, &mut shift_max);

    let v = value as f64;
    (
        (v - v * shift_min).round() as i64,
        (v + v * shift_max).round() as i64,
    )
}

/// Second stage of the generator: initial offsets after the rebound/protect logic.
pub fn initial_offsets(
    day: i64,
    mode: AttackMode,
    conf: &EstimConf,
    tmin: i64,
    tmax: i64,
    off_raw: i64,
) -> (i64, i64) {
    let (min, max) = attack_range(day, mode);
    let f = day_factor(day);
    let mut off_min = off_raw as f64;
    let mut off_max = (f * (conf.variance - 2 * conf.shift) as f64).round() - off_min;

    let mut o1 = off_min / 100.0;
    let mut o2 = off_max / 100.0;
    let rebound_min = deshift(tmin, min, max, &mut o1, &mut o2);
    let rebound_max = deshift(tmax, min, max, &mut o1, &mut o2);
    if rebound_min || rebound_max {
        off_min = (o1 * 100.0).round();
        off_max = (o2 * 100.0).round();

        let protect = if day <= 30 { 3.0 } else { 1.0 };
        if off_min < protect {
            off_max -= protect - off_min;
            off_min = protect;
        } else if off_max < protect {
            off_min -= protect - off_max;
            off_max = protect;
        }
    }

    (off_min.floor() as i64, off_max.floor() as i64)
}

pub fn generate_target(
    day: i64,
    mode: AttackMode,
    conf: &EstimConf,
    value: i64,
    shift_raw: i64,
    off_raw: i64,
) -> HiddenTarget {
    let (tmin, tmax) = shifted_targets(day, mode, conf, value, shift_raw);
    let (om0, ox0) = initial_offsets(day, mode, conf, tmin, tmax, off_raw);
    HiddenTarget {
        tmin,
        tmax,
        om0,
        ox0,
    }
}

/// Current offsets, in thousandths of a percent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offsets {
    pub min: i64,
    pub max: i64,
}

impl Offsets {
    pub fn initial(target: &HiddenTarget) -> Self {
        Offsets {
            min: target.om0 * MILLI,
            max: target.ox0 * MILLI,
        }
    }
}

/// Inclusive `mt_rand` bounds (in thousandths) of one shrink draw at round `i`.
pub fn step_bounds(off: Offsets, round: usize) -> (i64, i64) {
    let spendable =
        (off.min.max(0) + off.max.max(0)) as f64 / MILLI as f64 / (MAX_ROUNDS - round) as f64;
    (
        (spendable * 250.0).floor() as i64,
        (spendable * 1000.0).floor() as i64,
    )
}

/// Number of `k` in `0..=99` with `k < 100 * om / (om + ox)`, i.e. the success odds of
/// `RandomGenerator::chance(om / (om + ox))` in percent.
pub fn min_side_percent(off: Offsets) -> i64 {
    let total = off.min + off.max;
    if off.min <= 0 || total <= 0 {
        0
    } else {
        ((100 * off.min + total - 1) / total).min(100)
    }
}

/// One round of `calculate_offsets`, drawing in the same order as the PHP code.
pub fn step_offsets(off: Offsets, round: usize, min_spread_milli: i64, rng: &mut Mt64) -> Offsets {
    if off.min + off.max <= min_spread_milli {
        return off;
    }
    let (lo, hi) = step_bounds(off, round);
    let pct = min_side_percent(off);
    let increase_min = match pct {
        100 => true,
        0 => false,
        p => rng.random_range(0..=99) < p,
    };
    let alter = rng.random_range(lo..=hi);
    if rng.random_range(0..=99) < 25 {
        let alter_max = rng.random_range(lo..=hi);
        Offsets {
            min: (off.min - alter).max(0),
            max: (off.max - alter_max).max(0),
        }
    } else if increase_min && off.min > 0 {
        Offsets {
            min: (off.min - alter).max(0),
            max: off.max,
        }
    } else {
        Offsets {
            min: off.min,
            max: (off.max - alter).max(0),
        }
    }
}

/// Displayed watchtower range for a given offset state (`EstimateZombieAttackAction`, no red souls).
pub fn displayed_range(target: &HiddenTarget, off: Offsets, blocks: i64) -> (i64, i64) {
    let tmin = target.tmin as f64;
    let tmax = target.tmax as f64;
    let om = off.min as f64 / MILLI as f64;
    let ox = off.max as f64 / MILLI as f64;
    let mut min = (tmin - tmin * om / 100.0).round() as i64;
    let mut max = (tmax + tmax * ox / 100.0).round() as i64;
    if blocks > 1 {
        min = min.div_euclid(blocks) * blocks;
        max = (max + blocks - 1).div_euclid(blocks) * blocks;
    }
    (min, max)
}

/// Red-soul penalty per soul (`BuildingValueQuery::NightlyRedSoulPenalty`, 0.02 with the
/// level-2 blue soul building) and default cap (`modifiers.red_soul_max_factor`, 666 in Pandemonium).
pub const DEFAULT_SOUL_PENALTY: f64 = 0.04;
pub const DEFAULT_SOUL_MAX: f64 = 1.2;

/// `$soulFactor`: multiplies both displayed bounds and the night attack.
pub fn soul_factor(red_souls: u32, penalty: f64, max: f64) -> f64 {
    (1.0 + penalty * red_souls as f64).min(max)
}

/// Rounding block of the J+1 estimate for the estimated day (`ceil(day / 5) * 5`).
pub fn future_blocks(estimated_day: i64) -> i64 {
    ((estimated_day + 4) / 5 * 5).max(1)
}

/// Percentage shown for `rounds` weighted citizens.
pub fn percent_for_rounds(rounds: usize) -> u32 {
    ((rounds.min(MAX_ROUNDS) as f64 / MAX_ROUNDS as f64) * 100.0).round() as u32
}

/// Inverse of [`percent_for_rounds`]; `None` if no round count displays this percentage.
pub fn rounds_for_percent(pct: u32) -> Option<usize> {
    let rounds = (pct as f64 * MAX_ROUNDS as f64 / 100.0).round() as usize;
    (rounds <= MAX_ROUNDS && percent_for_rounds(rounds) == pct).then_some(rounds)
}

/// A full synthetic day: what the game would draw, and the offset path after 0..=24 rounds.
#[derive(Debug, Clone)]
pub struct SimulatedDay {
    pub attack: i64,
    pub target: HiddenTarget,
    pub path: Vec<Offsets>,
}

pub fn simulate_day(day: i64, mode: AttackMode, conf: &EstimConf, rng: &mut Mt64) -> SimulatedDay {
    let value = ValuePrior::new(day, mode).sample(rng);
    let (off_lo, off_hi) = off_raw_range(day, conf);
    let off_raw = rng.random_range(off_lo..=off_hi);
    let shift_raw = rng.random_range(0..=shift_raw_max(day, conf));
    let target = generate_target(day, mode, conf, value, shift_raw, off_raw);
    let attack = match mode {
        AttackMode::Hard => rng.random_range(target.tmin..=target.tmax),
        _ => value,
    };

    let mut path = Vec::with_capacity(MAX_ROUNDS + 1);
    let mut off = Offsets::initial(&target);
    path.push(off);
    for round in 0..MAX_ROUNDS {
        off = step_offsets(off, round, conf.min_spread_milli(), rng);
        path.push(off);
    }

    SimulatedDay {
        attack,
        target,
        path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attack_range_matches_pastebin_day_14() {
        assert_eq!(attack_range(14, AttackMode::Normal), (2022, 3018));
    }

    #[test]
    fn test_day_14_offset_budget() {
        let conf = EstimConf::default();
        assert_eq!(off_raw_range(14, &conf), (5, 26));
        assert_eq!(shift_raw_max(14, &conf), 1000);
        let (_, ox) = initial_offsets(14, AttackMode::Normal, &conf, 2200, 2420, 7);
        assert_eq!(ox, 21);
    }

    #[test]
    fn test_factor_scales_offsets() {
        let conf = EstimConf::default();
        assert_eq!(off_raw_range(35, &conf), (1, 7));
        assert_eq!(shift_raw_max(35, &conf), 250);
        assert_eq!(shift_raw_max(45, &conf), 150);
    }

    #[test]
    fn test_rebound_near_minimum_moves_offset_to_max_side() {
        // value 2200, 5 % shift each side: tmin = 2090, tmax = 2310.
        // 5 % below 2090 crosses the day minimum 2022, so the excess moves up: (3.25, 24.75) → (3, 25).
        let conf = EstimConf::default();
        let target = generate_target(14, AttackMode::Normal, &conf, 2200, 500, 5);
        assert_eq!(
            target,
            HiddenTarget {
                tmin: 2090,
                tmax: 2310,
                om0: 3,
                ox0: 25
            }
        );
    }

    #[test]
    fn test_no_rebound_in_the_middle_of_the_range() {
        let conf = EstimConf::default();
        let target = generate_target(14, AttackMode::Normal, &conf, 2400, 500, 10);
        assert_eq!(
            target,
            HiddenTarget {
                tmin: 2280,
                tmax: 2520,
                om0: 10,
                ox0: 18
            }
        );
    }

    #[test]
    fn test_shift_is_pushed_up_when_value_is_near_minimum() {
        // value 2030: only 0.39 % room below, so at most that shift stays on the min side.
        let conf = EstimConf::default();
        let (tmin, tmax) = shifted_targets(14, AttackMode::Normal, &conf, 2030, 1000);
        assert_eq!(tmin, 2022);
        assert_eq!(tmax, 2030 + 203 - 8);
    }

    #[test]
    fn test_value_prior_sums_to_one_and_favours_low_half() {
        let prior = ValuePrior::new(14, AttackMode::Normal);
        let total: f64 = (prior.min..=prior.max).map(|v| prior.weight(v)).sum();
        assert!((total - 1.0).abs() < 1e-9);
        assert!(prior.weight(prior.min) > prior.weight(prior.max));
        assert_eq!(prior.weight(prior.max + 1), 0.0);
    }

    #[test]
    fn test_min_side_percent_matches_chance() {
        assert_eq!(min_side_percent(Offsets { min: 0, max: 5000 }), 0);
        assert_eq!(min_side_percent(Offsets { min: 5000, max: 0 }), 100);
        assert_eq!(
            min_side_percent(Offsets {
                min: 1000,
                max: 1000
            }),
            50
        );
        // 100 * 1/3 = 33.3 → k in 0..=33 → 34 %.
        assert_eq!(
            min_side_percent(Offsets {
                min: 1000,
                max: 2000
            }),
            34
        );
    }

    #[test]
    fn test_percent_rounds_roundtrip() {
        for rounds in 0..=MAX_ROUNDS {
            assert_eq!(rounds_for_percent(percent_for_rounds(rounds)), Some(rounds));
        }
        assert_eq!(percent_for_rounds(9), 38);
        assert_eq!(percent_for_rounds(15), 63);
        assert_eq!(rounds_for_percent(40), None);
    }

    #[test]
    fn test_future_blocks() {
        assert_eq!(future_blocks(15), 15);
        assert_eq!(future_blocks(16), 20);
        assert_eq!(future_blocks(1), 5);
    }

    #[test]
    fn test_displayed_range_block_rounding() {
        let target = HiddenTarget {
            tmin: 2000,
            tmax: 2200,
            om0: 0,
            ox0: 0,
        };
        let off = Offsets {
            min: 1000,
            max: 1000,
        };
        assert_eq!(displayed_range(&target, off, 1), (1980, 2222));
        assert_eq!(displayed_range(&target, off, 15), (1980, 2235));
    }

    #[test]
    fn test_simulated_path_is_monotone_and_readings_nested() {
        let conf = EstimConf::default();
        let mut rng = Mt64::new(42);
        for _ in 0..200 {
            let day = simulate_day(14, AttackMode::Normal, &conf, &mut rng);
            assert!(day.target.tmin <= day.attack && day.attack <= day.target.tmax);
            for pair in day.path.windows(2) {
                assert!(pair[1].min <= pair[0].min && pair[1].max <= pair[0].max);
                assert!(pair[1].min >= 0 && pair[1].max >= 0);
                let (a0, b0) = displayed_range(&day.target, pair[0], 1);
                let (a1, b1) = displayed_range(&day.target, pair[1], 1);
                assert!(a0 <= a1 && b1 <= b0);
            }
        }
    }
}
