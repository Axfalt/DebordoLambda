//! French result message of an estimate (Discord markdown, same layout as the bot's other
//! commands), shared by `/estimation25` and the local CLI.
//!
//! The tightened attack range is the headline; the parameters and the seeds are secondary.
//! Everything is written straight into one `String` (`fmt::Write`), with no intermediate strings.

use crate::engine::{AttackMode, future_blocks};
use crate::inference::{Estimate, EstimationInput};
use std::fmt::{self, Write};

/// Seeds listed individually; beyond that only their count is given (Discord's 2000 chars).
const MAX_LISTED_SEEDS: usize = 5;

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
    if input.has_red_souls() {
        writeln!(out, "• **👻 Âmes rouges**: ×{}", input.soul_factor)?;
    }
    out.push('\n');
    Ok(())
}

/// The headline: the attack range the watchtower readings were tightened to.
fn write_attack(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    let (lo, hi) = estimate.attack();
    if input.has_red_souls() {
        // NightlyHandler: the night attack is round(zombies * soulFactor).
        let soul = input.soul_factor;
        let night = |v: i64| (v as f64 * soul).round() as i64;
        writeln!(out, "🎯 **Attaque: {} - {}**", night(lo), night(hi))?;
        writeln!(out, "-# Attaque avant âmes rouges : {lo} - {hi}")?;
    } else {
        writeln!(out, "🎯 **Attaque: {lo} - {hi}**")?;
    }
    out.push('\n');
    Ok(())
}

/// A bound pinned by the seed (`a`) or left open between two values (`a…b`).
struct Bound((i64, i64));

impl fmt::Display for Bound {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (a, b) = self.0;
        if a == b {
            write!(f, "{a}")
        } else {
            write!(f, "{a}…{b}")
        }
    }
}

fn write_seeds(out: &mut String, estimate: &Estimate) -> fmt::Result {
    let seeds = &estimate.seeds;
    if seeds.len() > 1 {
        writeln!(out, "-# {} seeds compatibles", seeds.len())?;
    }
    for s in seeds.iter().take(MAX_LISTED_SEEDS) {
        let m = &s.seed;
        writeln!(
            out,
            "-# 🎲 Seed {:#010x} · offsets ({}, {}) · plage cachée {} - {} · attaque {} - {}",
            m.seed,
            m.om0,
            m.ox0,
            Bound(m.tmin),
            Bound(m.tmax),
            s.attack.0,
            s.attack.1
        )?;
    }
    if seeds.len() > MAX_LISTED_SEEDS {
        writeln!(out, "-# … et {} autres", seeds.len() - MAX_LISTED_SEEDS)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference::{Observation, SeedEstimate};
    use crate::seed::SeedMatch;

    fn estimate(seeds: usize) -> Estimate {
        let seed = |i: usize| SeedEstimate {
            seed: SeedMatch {
                seed: 0x9e76_c676 + u32::try_from(i).unwrap(),
                om0: 11,
                ox0: 10,
                tmin: (3759, 3759),
                tmax: (4056, 4056),
            },
            attack: (3949, 3971),
        };
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
            seeds: (0..seeds).map(seed).collect(),
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
        assert!(text.contains("-# 🎲 Seed 0x9e76c676 · offsets (11, 10)"));
        assert!(!text.contains("seeds compatibles"));
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
    fn test_many_seeds_are_capped() {
        let input = EstimationInput {
            day: 17,
            ..EstimationInput::default()
        };
        let text = format_summary(&input, &estimate(8));
        assert!(text.contains("-# 8 seeds compatibles\n"));
        assert_eq!(text.matches("-# 🎲 Seed").count(), MAX_LISTED_SEEDS);
        assert!(text.contains("-# … et 3 autres\n"));
    }
}
