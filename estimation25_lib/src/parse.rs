//! Parsing of pasted watchtower readings, e.g. `[b][33%][/b] 2047 - 2749 🧟`,
//! with optional `jour: 14`, `demain: oui`, `âmes: 1`, `pénalité: 0.02`, `pénalité veille: 0.04`
//! lines.
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
    pub red_souls: Option<u32>,
    pub planner_red_souls: Option<u32>,
    pub soul_penalty: Option<f64>,
    /// Penalty per red soul when the planner (J+1) readings were taken, when it differs from
    /// today's (the level-2 blue soul building was voted in between).
    pub planner_soul_penalty: Option<f64>,
    pub soul_max: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedText {
    pub day: Option<i64>,
    pub future: Option<bool>,
    pub readings: Vec<Reading>,
    /// Readings of a `Planificateur` (J+1) section.
    pub planner: Vec<Reading>,
    /// Day on which the `Planificateur` readings were taken.
    pub planner_day: Option<i64>,
    /// Red souls in town today / when the planner readings were taken.
    pub red_souls: Option<u32>,
    pub planner_red_souls: Option<u32>,
    /// Penalty per red soul (0.04, or 0.02 with the level-2 blue soul building), today / when
    /// the planner readings were taken (defaults to today's).
    pub soul_penalty: Option<f64>,
    pub planner_soul_penalty: Option<f64>,
    /// Cap of the red-soul factor (1.2, or 666 in Pandemonium).
    pub soul_max: Option<f64>,
    /// Non-empty lines that were neither a reading nor a known key.
    pub ignored: Vec<String>,
    /// Ignored lines shaped like a setting (`key: value`): an unknown key or an invalid value,
    /// most likely a typo that would silently change the search.
    pub unknown_settings: Vec<String>,
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
        // Every town type of the game uses normal attacks except custom private towns, which
        // the tool does not support (their `attacks` setting cannot be read).
        let mode = AttackMode::Normal;
        let penalty = o
            .soul_penalty
            .or(self.soul_penalty)
            .unwrap_or(DEFAULT_SOUL_PENALTY);
        let planner_penalty = o
            .planner_soul_penalty
            .or(self.planner_soul_penalty)
            .unwrap_or(penalty);
        let max = o.soul_max.or(self.soul_max).unwrap_or(DEFAULT_SOUL_MAX);
        let factor = |souls: u32| soul_factor(souls, penalty, max);
        let planner_factor = |souls: u32| soul_factor(souls, planner_penalty, max);
        let souls = o.red_souls.or(self.red_souls);
        // The red souls of the planner day default to today's.
        let planner_souls = o.planner_red_souls.or(self.planner_red_souls).or(souls);

        if self.readings.is_empty() && !self.planner.is_empty() {
            // Only yesterday's J+1 readings: they were displayed with the planner day's factor,
            // tonight's attack uses today's.
            let readings_factor = planner_factor(planner_souls.unwrap_or(0));
            let night_factor = factor(souls.or(planner_souls).unwrap_or(0));
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
                soul_factor: readings_factor,
                planner_soul_factor: None,
                attack_soul_factor: Some(night_factor)
                    .filter(|f| (f - readings_factor).abs() > f64::EPSILON),
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
            planner_soul_factor: planner_souls.map(planner_factor),
            attack_soul_factor: None,
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
pub fn parse_bool(value: &str) -> Option<bool> {
    match value.trim().to_lowercase().as_str() {
        "oui" | "o" | "yes" | "y" | "true" | "vrai" | "1" => Some(true),
        "non" | "n" | "no" | "false" | "faux" | "0" => Some(false),
        _ => None,
    }
}

/// Pasteable text of an input: `overrides`' red-soul lines, then the readings in
/// `Planificateur J… / Estimation J…` sections. [`parse_text`] reads it back into the same input,
/// so a result can be re-run after editing (the `/estimation25` "Voir la configuration" button).
#[must_use]
pub fn format_input_text(input: &EstimationInput, overrides: &InputOverrides) -> String {
    use std::fmt::Write as _;

    fn section(out: &mut String, title: &str, readings: &[Reading]) {
        out.push_str(title);
        out.push('\n');
        for r in readings {
            let _ = writeln!(out, "{}% : {} - {}", r.pct, r.min, r.max);
        }
    }

    let mut out = String::with_capacity(64 + 24 * (input.readings.len() + input.planner.len()));
    if let Some(n) = overrides.red_souls {
        let _ = writeln!(out, "âmes: {n}");
    }
    if let Some(n) = overrides.planner_red_souls {
        let _ = writeln!(out, "âmes veille: {n}");
    }
    if let Some(p) = overrides.soul_penalty {
        let _ = writeln!(out, "pénalité: {p}");
    }
    if let Some(p) = overrides.planner_soul_penalty {
        let _ = writeln!(out, "pénalité veille: {p}");
    }
    if let Some(m) = overrides.soul_max {
        let _ = writeln!(out, "âmes max: {m}");
    }
    if input.future {
        // A J+1 estimate: its readings were taken on `day`.
        section(
            &mut out,
            &format!("Planificateur J{}", input.day),
            &input.readings,
        );
    } else {
        if !input.planner.is_empty() {
            section(
                &mut out,
                &format!("Planificateur J{}", input.day - 1),
                &input.planner,
            );
        }
        section(
            &mut out,
            &format!("Estimation J{}", input.day),
            &input.readings,
        );
    }
    out
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
            // `pénalité_veille`, `Pénalité-veille` and `penalite  veille` are the same key.
            let key = key
                .to_lowercase()
                .replace('â', "a")
                .replace('é', "e")
                .replace(['_', '-'], " ");
            let key = key.split_whitespace().collect::<Vec<_>>().join(" ");
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
                "penalite veille" | "penalite ames veille" | "penalite planificateur" => {
                    decimal(value)
                        .map(|p| parsed.planner_soul_penalty = Some(p))
                        .is_some()
                }
                "ames max" | "ames rouges max" | "plafond ames" => {
                    decimal(value).map(|m| parsed.soul_max = Some(m)).is_some()
                }
                "jour" | "day" | "j" => numbers(value)
                    .first()
                    .map(|&(_, _, d)| parsed.day = Some(d))
                    .is_some(),
                "demain" | "j+1" | "futur" | "future" => {
                    parse_bool(value).map(|b| parsed.future = Some(b)).is_some()
                }
                _ => false,
            }
        });
        if !recognised {
            if line.contains([':', '=']) {
                parsed.unknown_settings.push(line.to_string());
            }
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
        let text = "jour: 14\ndemain: oui\n\n[b][33%][/b] 2047 - 2749 🧟\nblabla\n[b][100%][/b] 2089 - 2361 🧟\n";
        let parsed = parse_text(text);
        assert_eq!(parsed.day, Some(14));
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
    fn test_eve_penalty_applies_to_the_planner_only() {
        // The level-2 blue soul building was voted on day 17: 0.04 per soul on day 16's J+1
        // readings, 0.02 on day 17's.
        let text = "Planificateur J16\n0% : 3460 - 4640\nEstimation J17\nâmes: 2\n\
                    pénalité: 0,02\npénalité veille: 0,04\n33% : 3666 - 4555\n";
        let parsed = parse_text(text);
        assert_eq!(parsed.planner_soul_penalty, Some(0.04));
        let input = parsed.into_input(&InputOverrides::default()).unwrap();
        assert!((input.soul_factor - 1.04).abs() < 1e-12);
        assert!((input.planner_soul_factor.unwrap() - 1.08).abs() < 1e-12);

        // Without it, both days use the same penalty.
        let input = parse_text(
            "Planificateur J16\n0% : 3460 - 4640\nEstimation J17\nâmes: 2\n\
                                pénalité: 0,02\n33% : 3666 - 4555\n",
        )
        .into_input(&InputOverrides::default())
        .unwrap();
        assert!((input.planner_soul_factor.unwrap() - 1.04).abs() < 1e-12);
    }

    #[test]
    fn test_setting_keys_accept_underscores_and_report_typos() {
        let parsed = parse_text(
            "penalité_veille: 0.04\nPénalité-veille = 0,04\npénalité veile: 0.04\n\
             âmes: deux\nTour de guet\n33% : 3666 - 4555\n",
        );
        assert_eq!(parsed.planner_soul_penalty, Some(0.04));
        assert_eq!(
            parsed.unknown_settings,
            vec!["pénalité veile: 0.04".to_string(), "âmes: deux".to_string()]
        );
        // A line without `:` is only ignored.
        assert!(parsed.ignored.contains(&"Tour de guet".to_string()));
    }

    #[test]
    fn test_planner_only_attack_uses_todays_penalty() {
        // Only yesterday's J+1 readings (0.04 per soul then), SPA2 active tonight (0.02).
        let text = "Planificateur J26\nâmes: 1\n8% : 12180 - 14730\n83% : 13140 - 14370\n";
        let overrides = InputOverrides {
            soul_penalty: Some(0.02),
            planner_soul_penalty: Some(0.04),
            ..InputOverrides::default()
        };
        let input = parse_text(text).into_input(&overrides).unwrap();
        assert!(input.future);
        assert!((input.soul_factor - 1.04).abs() < 1e-12);
        assert!((input.night_soul_factor() - 1.02).abs() < 1e-12);

        // Same penalty both days: a single factor.
        let input = parse_text(text)
            .into_input(&InputOverrides::default())
            .unwrap();
        assert_eq!(input.attack_soul_factor, None);
        assert!((input.night_soul_factor() - 1.04).abs() < 1e-12);
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
    fn test_input_text_roundtrips() {
        let overrides = InputOverrides {
            red_souls: Some(1),
            soul_penalty: Some(0.02),
            planner_soul_penalty: Some(0.04),
            soul_max: Some(666.0),
            ..InputOverrides::default()
        };
        let today = parse_text(TWO_SECTIONS).into_input(&overrides).unwrap();
        let text = format_input_text(&today, &overrides);
        let back = parse_text(&text)
            .into_input(&InputOverrides::default())
            .unwrap();
        assert_eq!(back, today);

        let future = parse_text("Planificateur J17\n0% : 4060 - 5340\n100% : 4180 - 4580\n")
            .into_input(&InputOverrides::default())
            .unwrap();
        let back = parse_text(&format_input_text(&future, &InputOverrides::default()))
            .into_input(&InputOverrides::default())
            .unwrap();
        assert_eq!(back, future);
    }

    #[test]
    fn test_day_accepts_j_prefix() {
        assert_eq!(parse_text("jour: J14").day, Some(14));
    }
}
