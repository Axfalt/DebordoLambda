//! French text summary of an estimate, shared by the CLI and (later) the Discord bot.
//!
//! Everything is written straight into one `String` (`fmt::Write`), with no intermediate strings.

use crate::engine::future_blocks;
use crate::inference::{Estimate, EstimationInput};
use std::fmt::{self, Write};

#[must_use]
pub fn format_summary(input: &EstimationInput, estimate: &Estimate) -> String {
    let mut out = String::with_capacity(512);
    write_summary(&mut out, input, estimate).expect("writing to a String cannot fail");
    out
}

fn write_summary(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    write_header(out, input, estimate)?;
    write_seeds(out, estimate)?;
    write_attack(out, input, estimate)
}

fn write_header(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    write!(
        out,
        "🔭 Tour de guet — attaque du J{} (",
        input.estimated_day()
    )?;
    if input.future {
        write!(
            out,
            "estimation J+1 faite au J{}, blocs de {}",
            input.day,
            input.blocks()
        )?;
    } else {
        out.push_str("estimation du jour");
    }
    writeln!(out, "), mode {}", input.mode.label())?;

    let obs = &estimate.observations;
    if let (Some(first), Some(last)) = (obs.first(), obs.last()) {
        writeln!(
            out,
            "{} relevés ({} % → {} %), dernier : {} - {} (largeur {})",
            obs.len(),
            first.pct,
            last.pct,
            last.min,
            last.max,
            last.max - last.min
        )?;
    }
    if !input.future && !input.planner.is_empty() {
        writeln!(
            out,
            "+ {} relevés J+1 pris au J{} (même tirage, blocs de {})",
            input.planner.len(),
            input.day - 1,
            future_blocks(input.day)
        )?;
    }
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
    out.push('\n');
    for s in &estimate.seeds {
        let m = &s.seed;
        writeln!(
            out,
            "🎲 Seed {:#010x} : offsets ({}, {}), plage cachée {} - {} → attaque {} - {}",
            m.seed,
            m.om0,
            m.ox0,
            Bound(m.tmin),
            Bound(m.tmax),
            s.attack.0,
            s.attack.1
        )?;
    }
    Ok(())
}

fn write_attack(out: &mut String, input: &EstimationInput, estimate: &Estimate) -> fmt::Result {
    let (lo, hi) = estimate.attack();
    if input.has_red_souls() {
        // NightlyHandler: the night attack is round(zombies * soulFactor).
        let soul = input.soul_factor;
        let night = |v: i64| (v as f64 * soul).round() as i64;
        writeln!(
            out,
            "\n🎯 Attaque stockée : {lo} - {hi} (largeur {})",
            hi - lo
        )?;
        writeln!(
            out,
            "💀 Attaque de la nuit (âmes rouges ×{soul}) : {} - {}",
            night(lo),
            night(hi)
        )
    } else {
        writeln!(out, "\n🎯 Attaque : {lo} - {hi} (largeur {})", hi - lo)
    }
}
