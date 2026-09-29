//! French result message of an estimate (Discord markdown), shared by `/estimation25` and the CLI.

use crate::engine::{AttackMode, factors_differ, future_blocks};
use crate::inference::{Estimate, EstimationInput};
use std::fmt::{self, Write};

/// The estimate without its timing footer: callers add `-# ⏱️ …` with what they measured.
#[must_use]
pub fn format_summary(input: &EstimationInput, estimate: &Estimate) -> String {
    let mut out = String::with_capacity(768);
    write_summary(&mut out, input, estimate).expect("writing to a String cannot fail");
    out
}

fn write_summary(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    writeln!(
        out,
        "## 🔭 Estimation de l'attaque du J{}\n",
        input.estimated_day()
    )?;
    write_parameters(out, input, estimate)?;
    write_attack(out, input, estimate)?;
    write_seeds(out, estimate)
}

fn write_parameters(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    out.push_str("**Paramètres:**\n");
    write!(out, "• **📅 Jour**: {}", input.estimated_day())?;
    if input.future {
        write!(out, " (estimation J+1 faite au J{})", input.day)?;
    }
    out.push('\n');

    let obs = &estimate.observations;
    if let (Some(first), Some(last)) = (obs.first(), obs.last()) {
        writeln!(
            out,
            "• **🔭 Dernier relevé**: {} - {} ({} %)",
            last.min, last.max, last.pct
        )?;
        write!(
            out,
            "• **📋 Relevés**: {} ({} % → {} %)",
            obs.len(),
            first.pct,
            last.pct
        )?;
        if !input.future && !input.planner.is_empty() {
            write!(
                out,
                " + {} J+1 du J{} (blocs de {})",
                input.planner.len(),
                input.day - 1,
                future_blocks(input.day)
            )?;
        }
        out.push('\n');
    }
    if input.mode != AttackMode::Normal {
        writeln!(out, "• **⚔️ Mode**: {}", input.mode.label())?;
    }
    let eve = input.planner_soul_factor.filter(|&f| {
        !input.future && !input.planner.is_empty() && factors_differ(f, input.soul_factor)
    });
    let night = Some(input.night_soul_factor()).filter(|&f| factors_differ(f, input.soul_factor));
    if input.has_red_souls() || eve.is_some() {
        write!(out, "• **👻 Âmes rouges**: ×{}", input.soul_factor)?;
        if let Some(f) = eve {
            write!(out, " (veille ×{f})")?;
        }
        if let Some(f) = night {
            write!(out, " (attaque ×{f})")?;
        }
        out.push('\n');
    }
    out.push('\n');
    Ok(())
}

fn write_attack(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    let (lo, hi) = estimate.attack();
    if input.has_red_souls() {
        // NightlyHandler: the night attack is round(zombies * soulFactor).
        let soul = input.night_soul_factor();
        let night = |v: i64| (v as f64 * soul).round() as i64;
        writeln!(out, "🎯 **Attaque: {} - {}**", night(lo), night(hi))?;
        writeln!(out, "-# Attaque avant âmes rouges : {lo} - {hi}")?;
    } else {
        writeln!(out, "🎯 **Attaque: {lo} - {hi}**")?;
    }
    Ok(())
}

fn write_seeds(out: &mut String, estimate: &Estimate) -> fmt::Result {
    let count = estimate.seed_count();
    if count > 1 {
        writeln!(out, "-# {count} runs compatibles")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{Observation, Reading, WindowEstimate};
    use crate::seed::Window;

    fn estimate(seeds: u64) -> Estimate {
        Estimate {
            observations: vec![
                Observation {
                    rounds: 8,
                    pct: 33,
                    min: 3666,
                    max: 4555,
                },
                Observation {
                    rounds: 24,
                    pct: 100,
                    min: 3869,
                    max: 4218,
                },
            ],
            windows: vec![WindowEstimate {
                window: Window {
                    om0: 11,
                    ox0: 10,
                    tmin: (3759, 3759),
                    tmax: (4056, 4056),
                },
                seeds,
                attack: (3949, 3971),
            }],
        }
    }

    #[test]
    fn test_headline_is_the_tightened_attack() {
        let input = EstimationInput {
            day: 17,
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(1));
        assert!(text.starts_with("## 🔭 Estimation de l'attaque du J17\n"));
        assert!(text.contains("🎯 **Attaque: 3949 - 3971**\n"));
        assert!(text.contains("• **🔭 Dernier relevé**: 3869 - 4218 (100 %)\n"));
        assert!(!text.contains("Seed 0x"));
        assert!(!text.contains("runs compatibles"));
        assert!(!text.contains("Âmes rouges"));
    }

    #[test]
    fn test_red_souls_headline_is_the_night_attack() {
        let input = EstimationInput {
            day: 17,
            soul_factor: 1.04,
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(1));
        assert!(text.contains("🎯 **Attaque: 4107 - 4130**\n"));
        assert!(text.contains("-# Attaque avant âmes rouges : 3949 - 3971\n"));
        assert!(text.contains("• **👻 Âmes rouges**: ×1.04\n"));
    }

    #[test]
    fn test_eve_soul_factor_is_shown_when_it_differs() {
        let input = EstimationInput {
            day: 17,
            soul_factor: 1.04,
            planner_soul_factor: Some(1.08),
            planner: vec![Reading {
                pct: 0,
                min: 3460,
                max: 4640,
            }],
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(1));
        assert!(text.contains("• **👻 Âmes rouges**: ×1.04 (veille ×1.08)\n"));
    }

    #[test]
    fn test_night_soul_factor_drives_the_headline() {
        let input = EstimationInput {
            day: 26,
            future: true,
            soul_factor: 1.04,
            attack_soul_factor: Some(1.02),
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(1));
        // round(3949 × 1.02) - round(3971 × 1.02)
        assert!(text.contains("🎯 **Attaque: 4028 - 4050**\n"));
        assert!(text.contains("• **👻 Âmes rouges**: ×1.04 (attaque ×1.02)\n"));
    }

    #[test]
    fn test_several_seeds_are_only_counted() {
        let input = EstimationInput {
            day: 17,
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(8));
        assert!(text.contains("-# 8 runs compatibles\n"));
        assert!(!text.contains("Seed 0x"));
    }
}
