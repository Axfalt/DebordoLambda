//! Watchtower readings collected by `MyHordes` Optimizer (MHO).
//!
//! `GET {MHO_API}/AttaqueEstimation/Estimations/{day}?townId={town}` (with the [`ORIGIN_HEADER`])
//! returns, for the attack of `day`, today's readings (`estim`) and the J+1 readings taken the
//! day before (`planif`), keyed by percentage (`_0` … `_100`, `null` when nobody took it).
//!
//! This module only models and converts the payload; HTTP is left to the caller (the CLI, and
//! later the Lambda) so the library stays free of network dependencies.

use crate::inference::{EstimationError, EstimationInput, Reading};
use crate::parse::{InputOverrides, ParsedText};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const MHO_API: &str = "https://api.myhordesoptimizer.fr";
/// MHO rejects requests without an origin (`No Mho-Origin ...`, HTTP 400).
pub const ORIGIN_HEADER: (&str, &str) = ("Mho-Origin", "website");

/// URL of the readings for the attack of `day` in `town_id`.
#[must_use]
pub fn estimations_url(day: i64, town_id: i64) -> String {
    format!("{MHO_API}/AttaqueEstimation/Estimations/{day}?townId={town_id}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct MhoValue {
    pub min: i64,
    pub max: i64,
}

/// `EstimationRequestDto`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MhoEstimations {
    /// Day of the attack these readings estimate.
    pub day: i64,
    /// Readings taken on `day` (`EstimationsDto`).
    #[serde(default)]
    pub estim: BTreeMap<String, Option<MhoValue>>,
    /// J+1 readings taken on `day - 1` for the same attack.
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

    /// Readings taken on the attack day.
    #[must_use]
    pub fn today(&self) -> Vec<Reading> {
        readings(&self.estim)
    }

    /// J+1 readings taken the day before.
    #[must_use]
    pub fn planner(&self) -> Vec<Reading> {
        readings(&self.planif)
    }

    /// Builds the inference input for this attack.
    ///
    /// With `future` false (estimate of today's attack), today's readings are used and the J+1
    /// readings of the day before sharpen them. With `future` true (the payload of tomorrow's
    /// attack, fetched today), only the J+1 readings exist yet and are estimated as such.
    ///
    /// # Errors
    ///
    /// [`EstimationError::NoReadings`] when the payload holds no reading for the requested estimate.
    pub fn into_input(
        self,
        future: bool,
        overrides: &InputOverrides,
    ) -> Result<EstimationInput, EstimationError> {
        let planner = self.planner();
        let readings = if future { Vec::new() } else { self.today() };
        if readings.is_empty() && planner.is_empty() {
            return Err(EstimationError::NoReadings);
        }
        let parsed = ParsedText {
            day: (!future).then_some(self.day),
            readings,
            planner,
            planner_day: Some(self.day - 1),
            ..ParsedText::default()
        };
        // The payload fixes the day and the kind of estimate.
        let overrides = InputOverrides {
            day: None,
            future: None,
            ..overrides.clone()
        };
        parsed.into_input(&overrides)
    }
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

    #[test]
    fn test_today_estimate_uses_both_sections() {
        let est = MhoEstimations::from_json(SAMPLE).unwrap();
        let input = est.into_input(false, &InputOverrides::default()).unwrap();
        assert_eq!((input.day, input.future), (17, false));
        assert_eq!((input.readings.len(), input.planner.len()), (3, 3));
    }

    #[test]
    fn test_future_estimate_uses_planner_only() {
        let est = MhoEstimations::from_json(SAMPLE).unwrap();
        let input = est.into_input(true, &InputOverrides::default()).unwrap();
        assert_eq!(
            (input.day, input.future, input.estimated_day()),
            (16, true, 17)
        );
        assert_eq!(input.readings.len(), 3);
        assert!(input.planner.is_empty());
    }

    #[test]
    fn test_empty_payload_is_rejected() {
        let body = r#"{"day":18,"estim":{"_0":null,"_100":null},"planif":{"_0":null}}"#;
        let est = MhoEstimations::from_json(body).unwrap();
        assert_eq!(
            est.into_input(false, &InputOverrides::default()),
            Err(EstimationError::NoReadings)
        );
    }

    #[test]
    fn test_red_souls_override_is_kept() {
        let est = MhoEstimations::from_json(SAMPLE).unwrap();
        let overrides = InputOverrides {
            red_souls: Some(1),
            ..InputOverrides::default()
        };
        let input = est.into_input(false, &overrides).unwrap();
        assert!((input.soul_factor - 1.04).abs() < 1e-12);
    }

    #[test]
    fn test_url() {
        assert_eq!(
            estimations_url(18, 12345),
            "https://api.myhordesoptimizer.fr/AttaqueEstimation/Estimations/18?townId=12345"
        );
    }
}
