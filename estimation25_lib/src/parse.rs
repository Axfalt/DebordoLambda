//! Parsing of pasted watchtower readings, e.g. `[b][33%][/b] 2047 - 2749 🧟`,
//! with optional `jour: 14`, `mode: hard`, `demain: oui`, `âmes: 1`, `pénalité: 0.02` lines.
//!
//! A paste may hold two sections: `Planificateur J17` (J+1 readings taken on day 17) and
//! `Estimation J18` (today's readings). Readings before any header are today's; an `âmes: N`
//! line inside the `Planificateur` section gives the red souls of that day.

use crate::engine::{AttackMode, DEFAULT_SOUL_MAX, DEFAULT_SOUL_PENALTY, soul_factor};
use crate::inference::{EstimationError, EstimationInput, Reading};
use serde::{Deserialize, Serialize};

/// Values given outside the text (command-line options); they take precedence over the text.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InputOverrides {
    pub day: Option<i64>,
    pub future: Option<bool>,
    pub mode: Option<AttackMode>,
    pub red_souls: Option<u32>,
    pub planner_red_souls: Option<u32>,
    pub soul_penalty: Option<f64>,
    pub soul_max: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedText {
    pub day: Option<i64>,
    pub future: Option<bool>,
    pub mode: Option<AttackMode>,
    pub readings: Vec<Reading>,
    /// Readings of a `Planificateur` (J+1) section.
    pub planner: Vec<Reading>,
    /// Day on which the `Planificateur` readings were taken.
    pub planner_day: Option<i64>,
    /// Red souls in town today / when the planner readings were taken.
    pub red_souls: Option<u32>,
    pub planner_red_souls: Option<u32>,
    /// Penalty per red soul (0.04, or 0.02 with the level-2 blue soul building).
    pub soul_penalty: Option<f64>,
    /// Cap of the red-soul factor (1.2, or 666 in Pandemonium).
    pub soul_max: Option<f64>,
    /// Non-empty lines that were neither a reading nor a known key.
    pub ignored: Vec<String>,
}

impl ParsedText {
    /// Builds the inference input; explicit arguments override what the text says.
    ///
    /// A paste holding only a `Planificateur` section is a J+1 estimate made on `planner_day`.
    /// With both sections, the J+1 readings sharpen today's (same seeded path).
    ///
    /// # Errors
    ///
    /// [`EstimationError::MissingDay`] when neither the overrides nor the text give a day.
    pub fn into_input(self, o: &InputOverrides) -> Result<EstimationInput, EstimationError> {
        let mode = o.mode.or(self.mode).unwrap_or_default();
        let penalty = o
            .soul_penalty
            .or(self.soul_penalty)
            .unwrap_or(DEFAULT_SOUL_PENALTY);
        let max = o.soul_max.or(self.soul_max).unwrap_or(DEFAULT_SOUL_MAX);
        let factor = |souls: u32| soul_factor(souls, penalty, max);
        let souls = o.red_souls.or(self.red_souls);
        let planner_souls = o.planner_red_souls.or(self.planner_red_souls);

        if self.readings.is_empty() && !self.planner.is_empty() {
            return Ok(EstimationInput {
                day: o
                    .day
                    .or(self.planner_day)
                    .or(self.day)
                    .ok_or(EstimationError::MissingDay)?,
                future: true,
                mode,
                readings: self.planner,
                planner: Vec::new(),
                soul_factor: factor(planner_souls.or(souls).unwrap_or(0)),
                planner_soul_factor: None,
            });
        }
        Ok(EstimationInput {
            day: o
                .day
                .or(self.day)
                .or(self.planner_day.map(|d| d + 1))
                .ok_or(EstimationError::MissingDay)?,
            future: o.future.or(self.future).unwrap_or(false),
            mode,
            readings: self.readings,
            planner: self.planner,
            soul_factor: factor(souls.unwrap_or(0)),
            planner_soul_factor: planner_souls.map(factor),
        })
    }
}

/// Digit runs of a line with their byte span.
fn numbers(line: &str) -> Vec<(usize, usize, i64)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, c) in line
        .char_indices()
        .chain(std::iter::once((line.len(), ' ')))
    {
        match (c.is_ascii_digit(), start) {
            (true, None) => start = Some(i),
            (false, Some(s)) => {
                if let Ok(v) = line[s..i].parse() {
                    out.push((s, i, v));
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

fn parse_reading(line: &str) -> Option<Reading> {
    let nums = numbers(line);
    let pct_index = nums
        .iter()
        .position(|&(_, end, _)| line[end..].trim_start().starts_with('%'))?;
    let pct = u32::try_from(nums[pct_index].2).ok()?;
    let rest = &nums[pct_index + 1..];
    match rest {
        [(_, _, min), (_, _, max), ..] => Some(Reading {
            pct,
            min: *min,
            max: *max,
        }),
        _ => None,
    }
}

#[must_use]
pub fn parse_mode(value: &str) -> Option<AttackMode> {
    match value.trim().to_lowercase().as_str() {
        "normal" | "normale" => Some(AttackMode::Normal),
        "hard" | "difficile" | "dur" | "dure" => Some(AttackMode::Hard),
        "easy" | "facile" => Some(AttackMode::Easy),
        _ => None,
    }
}

#[must_use]
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "oui" | "o" | "yes" | "y" | "true" | "vrai" | "1" => Some(true),
        "non" | "n" | "no" | "false" | "faux" | "0" => Some(false),
        _ => None,
    }
}

/// A decimal value, with `.` or `,` as separator.
fn decimal(value: &str) -> Option<f64> {
    value.trim().replace(',', ".").parse().ok()
}

#[must_use]
pub fn parse_text(text: &str) -> ParsedText {
    let mut parsed = ParsedText::default();
    let mut in_planner = false;
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if line.contains('%') {
            match parse_reading(line) {
                Some(reading) if in_planner => parsed.planner.push(reading),
                Some(reading) => parsed.readings.push(reading),
                // `63% : -`: nobody took that reading.
                None if numbers(line).len() == 1 => {}
                None => parsed.ignored.push(line.to_string()),
            }
            continue;
        }

        let lower = line.to_lowercase();
        let first_number = numbers(line).first().map(|&(_, _, v)| v);
        if lower.starts_with("planificateur") {
            in_planner = true;
            parsed.planner_day = first_number.or(parsed.planner_day);
            continue;
        }
        if lower.starts_with("estimation") && !line.contains(':') {
            in_planner = false;
            parsed.day = first_number.or(parsed.day);
            continue;
        }

        let recognised = line.split_once([':', '=']).is_some_and(|(key, value)| {
            let key = key
                .trim()
                .to_lowercase()
                .replace('â', "a")
                .replace('é', "e");
            let count = || {
                numbers(value)
                    .first()
                    .and_then(|&(_, _, v)| u32::try_from(v).ok())
            };
            match key.as_str() {
                "ames" | "ames rouges" | "red souls" if in_planner => count()
                    .map(|n| parsed.planner_red_souls = Some(n))
                    .is_some(),
                "ames" | "ames rouges" | "red souls" => {
                    count().map(|n| parsed.red_souls = Some(n)).is_some()
                }
                "ames veille" | "ames rouges veille" | "ames planificateur" => count()
                    .map(|n| parsed.planner_red_souls = Some(n))
                    .is_some(),
                "penalite" | "penalite ames" => decimal(value)
                    .map(|p| parsed.soul_penalty = Some(p))
                    .is_some(),
                "ames max" | "ames rouges max" | "plafond ames" => {
                    decimal(value).map(|m| parsed.soul_max = Some(m)).is_some()
                }
                "jour" | "day" | "j" => numbers(value)
                    .first()
                    .map(|&(_, _, d)| parsed.day = Some(d))
                    .is_some(),
                "mode" | "attaques" | "attaque" => {
                    parse_mode(value).map(|m| parsed.mode = Some(m)).is_some()
                }
                "demain" | "j+1" | "futur" | "future" => {
                    parse_bool(value).map(|b| parsed.future = Some(b)).is_some()
                }
                _ => false,
            }
        });
        if !recognised {
            parsed.ignored.push(line.to_string());
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parses_bbcode_reading_with_emoji() {
        assert_eq!(
            parse_reading("[b][33%][/b] 2047 - 2749 🧟"),
            Some(Reading {
                pct: 33,
                min: 2047,
                max: 2749
            })
        );
    }

    #[test]
    fn test_parses_plain_reading_forms() {
        assert_eq!(
            parse_reading("100% 2089-2361"),
            Some(Reading {
                pct: 100,
                min: 2089,
                max: 2361
            })
        );
        assert_eq!(
            parse_reading("  8 %  :  150 – 210"),
            Some(Reading {
                pct: 8,
                min: 150,
                max: 210
            })
        );
        assert_eq!(parse_reading("33% 2047"), None);
    }

    #[test]
    fn test_parse_text_keys_and_readings() {
        let text = "jour: 14\nMode = hard\ndemain: oui\n\n[b][33%][/b] 2047 - 2749 🧟\nblabla\n[b][100%][/b] 2089 - 2361 🧟\n";
        let parsed = parse_text(text);
        assert_eq!(parsed.day, Some(14));
        assert_eq!(parsed.mode, Some(AttackMode::Hard));
        assert_eq!(parsed.future, Some(true));
        assert_eq!(parsed.readings.len(), 2);
        assert_eq!(parsed.ignored, vec!["blabla".to_string()]);
    }

    const TWO_SECTIONS: &str = "Estimations pour le jour 18
        Attaque J18 calculée : 4197 - 4561
        Planificateur J17
        0% : 4060 - 5340
        63% : -
        100% : 4180 - 4580
        Estimation J18
        0% : -
        33% : 4178 - 5145
        100% : 4197 - 4568
";

    #[test]
    fn test_parse_planner_and_today_sections() {
        let parsed = parse_text(TWO_SECTIONS);
        assert_eq!(parsed.day, Some(18));
        assert_eq!(parsed.planner_day, Some(17));
        assert_eq!(parsed.planner.len(), 2);
        assert_eq!(parsed.readings.len(), 2);
        assert_eq!(
            parsed.ignored,
            vec!["Attaque J18 calculée : 4197 - 4561".to_string()]
        );

        let input = parsed.into_input(&InputOverrides::default()).unwrap();
        assert_eq!((input.day, input.future), (18, false));
        assert_eq!(input.planner.len(), 2);
    }

    #[test]
    fn test_planner_only_paste_is_a_future_estimate() {
        let text = "Planificateur J17
0% : 4060 - 5340
100% : 4180 - 4580
";
        let input = parse_text(text)
            .into_input(&InputOverrides::default())
            .unwrap();
        assert_eq!(
            (input.day, input.future, input.readings.len()),
            (17, true, 2)
        );
        assert_eq!(input.estimated_day(), 18);
    }

    #[test]
    fn test_red_souls_per_section() {
        let text = "Planificateur J16\nâmes: 1\n0% : 3460 - 4640\nEstimation J17\nÂmes rouges : 2\n\
                    33% : 3666 - 4555\npénalité: 0,02\n";
        let parsed = parse_text(text);
        assert_eq!(parsed.planner_red_souls, Some(1));
        assert_eq!(parsed.red_souls, Some(2));
        assert_eq!(parsed.soul_penalty, Some(0.02));
        let input = parsed.into_input(&InputOverrides::default()).unwrap();
        assert!((input.soul_factor - 1.04).abs() < 1e-12);
        assert!((input.planner_soul_factor.unwrap() - 1.02).abs() < 1e-12);
    }

    #[test]
    fn test_soul_cap_key() {
        let parsed = parse_text("jour: 25\nâmes: 10\nâmes max: 666\n33% : 9236 - 10804\n");
        assert_eq!(parsed.soul_max, Some(666.0));
        let input = parsed.into_input(&InputOverrides::default()).unwrap();
        // 1 + 0.04 × 10 = 1.4, above the default cap of 1.2 but below 666.
        assert!((input.soul_factor - 1.4).abs() < 1e-12);
    }

    #[test]
    fn test_day_accepts_j_prefix() {
        assert_eq!(parse_text("jour: J14").day, Some(14));
    }
}
