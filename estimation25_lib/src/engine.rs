//! Emulation of the `MyHordes` watchtower estimation engine.
//!
//! Mirrors `PrepareZombieAttackEstimationAction` (hidden target generation) and the display of
//! `EstimateZombieAttackAction`. The seeded offset shrinking itself is replayed in [`crate::seed`].

use serde::{Deserialize, Serialize};

/// Watchtower quality caps at 24 weighted citizens: `calculate_offsets` runs at most 24 rounds.
pub const MAX_ROUNDS: usize = 24;

/// `TownSetting::OptFeatureAttacks`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AttackMode {
    #[default]
    Normal,
    Hard,
    Easy,
}

impl AttackMode {
    #[must_use]
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
    /// `calculate_offsets` stops shrinking once `om + ox <= spread - shift` (percent).
    #[must_use]
    pub fn min_spread(&self) -> f64 {
        (self.spread - self.shift) as f64
    }
}

/// Theoretical attack range of a day: `[min, max]` fed to `mt_rand`.
#[must_use]
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

#[must_use]
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
#[must_use]
pub fn off_raw_range(day: i64, conf: &EstimConf) -> (i64, i64) {
    let f = day_factor(day);
    (
        (f * (conf.offset_min - conf.shift) as f64).round() as i64,
        (f * (conf.offset_max - conf.shift) as f64).round() as i64,
    )
}

/// Upper bound of `mt_rand(0, shift * factor * 100)` (PHP truncates the float argument).
#[must_use]
pub fn shift_raw_max(day: i64, conf: &EstimConf) -> i64 {
    (conf.shift as f64 * day_factor(day) * 100.0) as i64
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
#[inline]
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

/// Generator constants of one estimated day, computed once and shared by every draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DayModel {
    pub day: i64,
    pub mode: AttackMode,
    /// Theoretical attack range `[min, max]` of the day.
    pub range: (i64, i64),
    pub factor: f64,
    /// Inclusive `mt_rand` range of the raw minimum offset.
    pub off_raw: (i64, i64),
    /// Upper bound of the raw shift draw.
    pub shift_raw_max: i64,
    /// Total shift `factor * shift / 100`.
    shift_total: f64,
    /// Offset budget `round(factor * (variance - 2 * shift))`.
    offset_budget: f64,
    /// Minimum offset kept by the rebound logic.
    protect: f64,
}

impl DayModel {
    #[must_use]
    pub fn new(day: i64, mode: AttackMode, conf: &EstimConf) -> Self {
        let factor = day_factor(day);
        DayModel {
            day,
            mode,
            range: attack_range(day, mode),
            factor,
            off_raw: off_raw_range(day, conf),
            shift_raw_max: shift_raw_max(day, conf),
            shift_total: factor * conf.shift as f64 / 100.0,
            offset_budget: (factor * (conf.variance - 2 * conf.shift) as f64).round(),
            protect: if day <= 30 { 3.0 } else { 1.0 },
        }
    }

    /// First stage of the generator: `(target_min, target_max)` from the drawn value and shift.
    #[inline]
    #[must_use]
    pub fn shifted_targets(&self, value: i64, shift_raw: i64) -> (i64, i64) {
        let (min, max) = self.range;
        let mut shift_min = shift_raw as f64 / 10000.0;
        let mut shift_max = self.shift_total - shift_min;
        deshift(value, min, max, &mut shift_min, &mut shift_max);

        let v = value as f64;
        (
            (v - v * shift_min).round() as i64,
            (v + v * shift_max).round() as i64,
        )
    }

    /// Second stage of the generator: initial offsets after the rebound/protect logic.
    #[inline]
    #[must_use]
    pub fn initial_offsets(&self, tmin: i64, tmax: i64, off_raw: i64) -> (i64, i64) {
        let (min, max) = self.range;
        let mut off_min = off_raw as f64;
        let mut off_max = self.offset_budget - off_min;

        let mut o1 = off_min / 100.0;
        let mut o2 = off_max / 100.0;
        let rebound_min = deshift(tmin, min, max, &mut o1, &mut o2);
        let rebound_max = deshift(tmax, min, max, &mut o1, &mut o2);
        if rebound_min || rebound_max {
            off_min = (o1 * 100.0).round();
            off_max = (o2 * 100.0).round();

            if off_min < self.protect {
                off_max -= self.protect - off_min;
                off_min = self.protect;
            } else if off_max < self.protect {
                off_min -= self.protect - off_max;
                off_max = self.protect;
            }
        }

        (off_min.floor() as i64, off_max.floor() as i64)
    }

    /// Initial offset pairs of every raw offset draw (ascending `off_raw`), without allocating.
    pub fn offset_pairs(&self, tmin: i64, tmax: i64) -> impl Iterator<Item = (i64, i64)> + '_ {
        let plain = !self.may_rebound(tmin, tmax);
        let budget = self.offset_budget as i64;
        (self.off_raw.0..=self.off_raw.1).map(move |off_raw| {
            if plain {
                (off_raw, budget - off_raw)
            } else {
                self.initial_offsets(tmin, tmax, off_raw)
            }
        })
    }

    /// Whether any raw offset draw triggers the rebound logic for these targets. Without a first
    /// rebound both `deshift` calls see the same offsets, and the largest `o1` / `o2` come from the
    /// extreme draws, so checking those two is enough (same float expressions as [`deshift`]).
    fn may_rebound(&self, tmin: i64, tmax: i64) -> bool {
        let (min, max) = (self.range.0 as f64, self.range.1 as f64);
        let o1 = self.off_raw.1 as f64 / 100.0;
        let o2 = (self.offset_budget - self.off_raw.0 as f64) / 100.0;
        [tmin, tmax].into_iter().any(|t| {
            let v = t as f64;
            o1 > (v - min) / v || o2 > (max - v) / v
        })
    }

    #[must_use]
    pub fn generate_target(&self, value: i64, shift_raw: i64, off_raw: i64) -> HiddenTarget {
        let (tmin, tmax) = self.shifted_targets(value, shift_raw);
        let (om0, ox0) = self.initial_offsets(tmin, tmax, off_raw);
        HiddenTarget {
            tmin,
            tmax,
            om0,
            ox0,
        }
    }
}

/// Watchtower range displayed for offsets `om` / `ox` (percent), as `EstimateZombieAttackAction`
/// computes it: red-soul factor, `round()`, then J+1 block rounding.
#[must_use]
pub fn displayed_range(
    target: &HiddenTarget,
    om: f64,
    ox: f64,
    blocks: i64,
    soul_factor: f64,
) -> (i64, i64) {
    let (tmin, tmax) = (target.tmin as f64, target.tmax as f64);
    let mut min = ((tmin - tmin * om / 100.0) * soul_factor).round() as i64;
    let mut max = ((tmax + tmax * ox / 100.0) * soul_factor).round() as i64;
    if blocks > 1 {
        min = min.div_euclid(blocks) * blocks;
        max = (max + blocks - 1).div_euclid(blocks) * blocks;
    }
    (min, max)
}

/// Red-soul penalty per soul (`BuildingValueQuery::NightlyRedSoulPenalty`, 0.02 with the
/// level-2 blue soul building) and cap (`modifiers.red_soul_max_factor`).
pub const DEFAULT_SOUL_PENALTY: f64 = 0.04;
pub const DEFAULT_SOUL_MAX: f64 = 1.2;
pub const PANDEMONIUM_SOUL_MAX: f64 = 666.0;

/// `$soulFactor`: multiplies both displayed bounds and the night attack.
#[must_use]
pub fn soul_factor(red_souls: u32, penalty: f64, max: f64) -> f64 {
    (1.0 + penalty * f64::from(red_souls)).min(max)
}

pub(crate) fn factors_differ(a: f64, b: f64) -> bool {
    (a - b).abs() > f64::EPSILON
}

/// Rounding block of the J+1 estimate for the estimated day (`ceil(day / 5) * 5`).
#[must_use]
pub fn future_blocks(estimated_day: i64) -> i64 {
    ((estimated_day + 4) / 5 * 5).max(1)
}

/// Percentage shown for `rounds` weighted citizens.
#[must_use]
pub fn percent_for_rounds(rounds: usize) -> u32 {
    ((rounds.min(MAX_ROUNDS) as f64 / MAX_ROUNDS as f64) * 100.0).round() as u32
}

/// Inverse of [`percent_for_rounds`]; `None` if no round count displays this percentage.
#[must_use]
pub fn rounds_for_percent(pct: u32) -> Option<usize> {
    let rounds = (f64::from(pct) * MAX_ROUNDS as f64 / 100.0).round() as usize;
    (rounds <= MAX_ROUNDS && percent_for_rounds(rounds) == pct).then_some(rounds)
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
        let (_, ox) = DayModel::new(14, AttackMode::Normal, &conf).initial_offsets(2200, 2420, 7);
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
        let target = DayModel::new(14, AttackMode::Normal, &conf).generate_target(2200, 500, 5);
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
        let target = DayModel::new(14, AttackMode::Normal, &conf).generate_target(2400, 500, 10);
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
        let (tmin, tmax) = DayModel::new(14, AttackMode::Normal, &conf).shifted_targets(2030, 1000);
        assert_eq!(tmin, 2022);
        assert_eq!(tmax, 2030 + 203 - 8);
    }

    #[test]
    fn test_offset_pairs_fast_path_matches_initial_offsets() {
        let conf = EstimConf::default();
        for (day, mode) in [
            (14, AttackMode::Normal),
            (18, AttackMode::Normal),
            (35, AttackMode::Hard),
        ] {
            let model = DayModel::new(day, mode, &conf);
            let (min, max) = model.range;
            for tmin in (min..=max).step_by(7) {
                for tmax in (tmin..=max).step_by(11) {
                    let expected = (model.off_raw.0..=model.off_raw.1)
                        .map(|o| model.initial_offsets(tmin, tmax, o));
                    assert!(
                        model.offset_pairs(tmin, tmax).eq(expected),
                        "day {day} {tmin}-{tmax}"
                    );
                }
            }
        }
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
    fn test_displayed_range_blocks_and_red_souls() {
        let target = HiddenTarget {
            tmin: 2000,
            tmax: 2200,
            om0: 0,
            ox0: 0,
        };
        assert_eq!(displayed_range(&target, 1.0, 1.0, 1, 1.0), (1980, 2222));
        assert_eq!(displayed_range(&target, 1.0, 1.0, 15, 1.0), (1980, 2235));
        // 1980 * 1.04 = 2059.2, 2222 * 1.04 = 2310.88.
        assert_eq!(displayed_range(&target, 1.0, 1.0, 1, 1.04), (2059, 2311));
    }
}
