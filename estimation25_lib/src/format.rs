//! French text summary of a posterior, shared by the CLI and (later) the Discord bot.

use crate::inference::{GuetInput, Posterior};

pub const CREDIBLE_LEVELS: [f64; 4] = [0.5, 0.8, 0.95, 0.99];

fn pct(p: f64) -> String {
    let v = p * 100.0;
    if v >= 10.0 {
        format!("{v:.0} %")
    } else {
        format!("{v:.1} %")
    }
}

pub fn format_summary(input: &GuetInput, post: &Posterior, top_targets: usize) -> String {
    let mut out = String::new();
    let day = input.estimated_day();
    let kind = if input.future {
        format!(
            "estimation J+1 faite au J{}, blocs de {}",
            input.day,
            input.blocks()
        )
    } else {
        "estimation du jour".to_string()
    };
    out.push_str(&format!(
        "🔭 Tour de guet — attaque du J{day} ({kind}), mode {}\n",
        input.mode.label()
    ));

    if let (Some(first), Some(last)) = (post.observations.first(), post.observations.last()) {
        out.push_str(&format!(
            "{} relevés ({} % → {} %), dernier : {} - {} (largeur {})\n",
            post.observations.len(),
            first.pct,
            last.pct,
            last.min,
            last.max,
            last.max - last.min
        ));
    }
    if !input.future && !input.planner.is_empty() {
        out.push_str(&format!(
            "+ {} relevés J+1 pris au J{} (même tirage, blocs de {})\n",
            input.planner.len(),
            input.day - 1,
            crate::engine::future_blocks(input.day)
        ));
    }

    if input.soul_factor != 1.0 {
        out.push_str("\n🎯 Attaque estimée (avant âmes rouges)\n");
    } else {
        out.push_str("\n🎯 Attaque réelle estimée\n");
    }
    out.push_str(&format!(
        "  Médiane {} · moyenne {:.0}\n",
        post.median(),
        post.mean()
    ));
    for level in CREDIBLE_LEVELS {
        let (lo, hi) = post.central_interval(level);
        out.push_str(&format!(
            "  {:>4} : {lo} - {hi} (largeur {})\n",
            pct(level),
            hi - lo
        ));
    }
    let (lo, hi) = post.support();
    out.push_str(&format!("  Possible : {lo} - {hi} (largeur {})\n", hi - lo));

    let soul = input.soul_factor;
    if soul != 1.0 {
        // NightlyHandler: attack = round(zombies * soulFactor).
        let scale = |v: i64| (v as f64 * soul).round() as i64;
        out.push_str(&format!(
            "\n💀 Avec les âmes rouges (×{soul}), attaque de la nuit :\n  Médiane {}\n",
            scale(post.median())
        ));
        for level in CREDIBLE_LEVELS {
            let (lo, hi) = post.central_interval(level);
            out.push_str(&format!(
                "  {:>4} : {} - {}\n",
                pct(level),
                scale(lo),
                scale(hi)
            ));
        }
        out.push_str(&format!("  Possible : {} - {}\n", scale(lo), scale(hi)));
    }
    if let Some(last) = post.observations.last() {
        let last_width = (last.max - last.min).max(1) as f64;
        let (q_lo, q_hi) = post.central_interval(0.95);
        out.push_str(&format!(
            "  L'intervalle à 95 % est {} plus étroit que le dernier relevé\n",
            pct(1.0 - (q_hi - q_lo) as f64 * soul / last_width)
        ));
    }

    let pairs = post.offset_pairs();
    if !pairs.is_empty() {
        let shown: Vec<String> = pairs
            .iter()
            .take(6)
            .map(|((om0, ox0), p)| format!("({om0}, {ox0}) {}", pct(*p)))
            .collect();
        out.push_str(&format!("\n🧩 Offsets initiaux : {}\n", shown.join(" · ")));
    }

    if top_targets > 0 && !post.targets.is_empty() {
        out.push_str("🔍 Plages cachées les plus probables (tmin - tmax, offsets) :\n");
        for t in post.targets.iter().take(top_targets) {
            out.push_str(&format!(
                "  {} - {} ({}, {}) : {}\n",
                t.target.tmin,
                t.target.tmax,
                t.target.om0,
                t.target.ox0,
                pct(t.probability)
            ));
        }
    }

    out
}
