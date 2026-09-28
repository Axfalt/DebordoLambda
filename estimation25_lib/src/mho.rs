//! Watchtower readings collected by `MyHordes` Optimizer (MHO).
//!
//! `GET {MHO_API}/AttaqueEstimation/Estimations/{day}?townId={town}` (with the [`ORIGIN_HEADER`])
//! returns what the watchtower showed on `day`: the readings of that day's attack (`estim`) and
//! the J+1 readings, taken the same day, of the **next** attack (`planif`), keyed by percentage
//! (`_0` … `_100`, `null` when nobody took it). The readings of one attack therefore come from
//! two payloads: `estim` of its day and `planif` of the day before (see [`attack_input`]).
//!
//! This module only models and converts the payloads; HTTP is left to the callers (the CLI and
//! the worker Lambda) so the library stays free of network dependencies.

use crate::inference::{EstimationError, EstimationInput, Reading};
use crate::parse::{InputOverrides, ParsedText};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MHO_API: &str = "https://api.myhordesoptimizer.fr";
/// MHO rejects requests without an origin (`No Mho-Origin ...`, HTTP 400).
pub const ORIGIN_HEADER: (&str, &str) = ("Mho-Origin", "website");

/// URL of the readings shown on `day` in `town_id`.
#[must_use]
pub fn estimations_url(day: i64, town_id: i64) -> String {
    format!("{MHO_API}/AttaqueEstimation/Estimations/{day}?townId={town_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MhoValue {
    pub min: i64,
    pub max: i64,
}

/// `EstimationRequestDto`: what the watchtower showed on `day`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MhoEstimations {
    pub day: i64,
    /// Readings of the attack of `day`, taken on `day` (`EstimationsDto`).
    #[serde(default)]
    pub estim: BTreeMap<String, Option<MhoValue>>,
    /// J+1 readings of the attack of `day + 1`, taken on `day`.
    #[serde(default)]
    pub planif: BTreeMap<String, Option<MhoValue>>,
}

/// `{"_33": {"min": 2047, "max": 2749}, "_38": null, …}` → readings sorted by percentage.
fn readings(values: &BTreeMap<String, Option<MhoValue>>) -> Vec<Reading> {
    let mut out: Vec<Reading> = values
        .iter()
        .filter_map(|(key, value)| {
            let pct = key.strip_prefix('_')?.parse().ok()?;
            let v = (*value)?;
            Some(Reading {
                pct,
                min: v.min,
                max: v.max,
            })
        })
        .collect();
    out.sort_unstable_by_key(|r| r.pct);
    out
}

impl MhoEstimations {
    /// Parses the JSON body of the estimations endpoint.
    ///
    /// # Errors
    ///
    /// The `serde_json` error when the body is not an `EstimationRequestDto`.
    pub fn from_json(body: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(body)
    }

    /// Readings of the attack of `day`, taken on `day`.
    #[must_use]
    pub fn today(&self) -> Vec<Reading> {
        readings(&self.estim)
    }

    /// J+1 readings of the attack of `day + 1`, taken on `day`.
    #[must_use]
    pub fn planner(&self) -> Vec<Reading> {
        readings(&self.planif)
    }
}

/// Days whose payloads hold the readings of the attack of `attack_day`: that day (`estim`) and
/// the day before (`planif`).
#[must_use]
pub fn payload_days(attack_day: i64) -> [i64; 2] {
    [attack_day, attack_day - 1]
}

/// Builds the input for the attack of `attack_day` from the payload of that day (`attack`, whose
/// `estim` holds the day's readings) and of the day before (`eve`, whose `planif` holds the J+1
/// readings of this attack). Either may be missing.
///
/// Without readings of the attack day yet (typically estimating tomorrow's attack), the J+1
/// readings alone give a J+1 estimate made on `attack_day - 1`.
///
/// # Errors
///
/// [`EstimationError::NoReadings`] when neither payload holds a reading of this attack.
pub fn attack_input(
    attack_day: i64,
    attack: Option<&MhoEstimations>,
    eve: Option<&MhoEstimations>,
    overrides: &InputOverrides,
) -> Result<EstimationInput, EstimationError> {
    let readings = attack.map_or_else(Vec::new, MhoEstimations::today);
    let planner = eve.map_or_else(Vec::new, MhoEstimations::planner);
    if readings.is_empty() && planner.is_empty() {
        return Err(EstimationError::NoReadings);
    }
    let parsed = ParsedText {
        day: (!readings.is_empty()).then_some(attack_day),
        readings,
        planner,
        planner_day: Some(attack_day - 1),
        ..ParsedText::default()
    };
    // The payloads fix the day and the kind of estimate.
    let overrides = InputOverrides {
        day: None,
        future: None,
        ..overrides.clone()
    };
    parsed.into_input(&overrides)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shape of a real response (J17 example, readings trimmed).
    const SAMPLE: &str = r#"{"day":17,
        "estim":{"_0":null,"_29":null,"_33":{"min":3666,"max":4555},"_38":null,
                 "_42":{"min":3712,"max":4507},"_100":{"min":3869,"max":4218}},
        "planif":{"_0":{"min":3460,"max":4640},"_4":{"min":3460,"max":4620},
                  "_100":{"min":3860,"max":4220}}}"#;

    #[test]
    fn test_parses_readings_and_skips_nulls() {
        let est = MhoEstimations::from_json(SAMPLE).unwrap();
        assert_eq!(est.day, 17);
        let today = est.today();
        assert_eq!(today.len(), 3);
        assert_eq!(
            today[0],
            Reading {
                pct: 33,
                min: 3666,
                max: 4555
            }
        );
        assert_eq!(today[2].pct, 100);
        assert_eq!(est.planner().len(), 3);
    }

    /// Payload of `day` with the given `estim` and `planif` readings.
    fn payload(day: i64, estim: &[Reading], planif: &[Reading]) -> MhoEstimations {
        let map = |readings: &[Reading]| {
            readings
                .iter()
                .map(|r| {
                    let value = MhoValue {
                        min: r.min,
                        max: r.max,
                    };
                    (format!("_{}", r.pct), Some(value))
                })
                .collect()
        };
        MhoEstimations {
            day,
            estim: map(estim),
            planif: map(planif),
        }
    }

    fn reading(pct: u32, min: i64, max: i64) -> Reading {
        Reading { pct, min, max }
    }

    #[test]
    fn test_payload_days() {
        assert_eq!(payload_days(17), [17, 16]);
    }

    #[test]
    fn test_attack_readings_come_from_two_days() {
        // Day 16 showed the J+1 readings of the J17 attack; day 17 its own readings, plus the
        // J+1 readings of the J18 attack, which must not be mixed in.
        let eve = payload(16, &[reading(33, 3000, 3500)], &[reading(0, 3460, 4640)]);
        let attack = payload(17, &[reading(33, 3666, 4555)], &[reading(0, 3900, 5000)]);
        let input =
            attack_input(17, Some(&attack), Some(&eve), &InputOverrides::default()).unwrap();
        assert_eq!((input.day, input.future), (17, false));
        assert_eq!(input.readings, vec![reading(33, 3666, 4555)]);
        assert_eq!(input.planner, vec![reading(0, 3460, 4640)]);
    }

    #[test]
    fn test_future_estimate_uses_the_eve_planner_only() {
        // Estimating tomorrow's attack (J18) on J17: only day 17's J+1 readings exist.
        let today = payload(17, &[reading(33, 3666, 4555)], &[reading(0, 3900, 5000)]);
        let input = attack_input(18, None, Some(&today), &InputOverrides::default()).unwrap();
        assert_eq!(
            (input.day, input.future, input.estimated_day()),
            (17, true, 18)
        );
        assert_eq!(input.readings, vec![reading(0, 3900, 5000)]);
        assert!(input.planner.is_empty());
    }

    #[test]
    fn test_empty_payloads_are_rejected() {
        let body = r#"{"day":18,"estim":{"_0":null,"_100":null},"planif":{"_0":null}}"#;
        let empty = MhoEstimations::from_json(body).unwrap();
        let o = InputOverrides::default();
        assert_eq!(
            attack_input(18, Some(&empty), Some(&empty), &o),
            Err(EstimationError::NoReadings)
        );
        assert_eq!(
            attack_input(18, None, None, &o),
            Err(EstimationError::NoReadings)
        );
    }

    /// The real J17 day (one red soul) rebuilt as MHO payloads still pins seed `0x9e76c676`.
    #[test]
    fn test_real_day_17_from_payloads() {
        let text = include_str!("../tests/data/j17_real_attack_4115_red_soul.txt");
        let parsed = crate::parse::parse_text(text);
        let eve = payload(16, &[], &parsed.planner);
        let attack = payload(17, &parsed.readings, &[]);
        let overrides = InputOverrides {
            red_souls: Some(1),
            ..InputOverrides::default()
        };
        let input = attack_input(17, Some(&attack), Some(&eve), &overrides).unwrap();
        assert!((input.soul_factor - 1.04).abs() < 1e-12);

        let est = crate::estimate(
            &input,
            &crate::EstimConf::default(),
            0x9e76_0000..=0x9e76_ffff,
            &std::sync::atomic::AtomicU64::new(0),
        )
        .unwrap();
        assert_eq!(est.seeds.len(), 1);
        assert_eq!(est.seeds[0].seed.seed, 0x9e76_c676);
        assert_eq!(est.attack(), (3949, 3971));
    }

    #[test]
    fn test_url() {
        assert_eq!(
            estimations_url(18, 12345),
            "https://api.myhordesoptimizer.fr/AttaqueEstimation/Estimations/18?townId=12345"
        );
    }
}
