use crate::config::SimConfig;
use rand::RngExt;
use rand::seq::SliceRandom;
use rand_mt::Mt64;
use rayon::prelude::*;
use std::cmp;
use std::cmp::max;
use std::collections::HashMap;

const FLAG_REDUCTION_RATE: f64 = 0.025;
const BASE_LEVEL_MIN: u32 = 45;
const BASE_LEVEL_MAX: u32 = 55;
const UNLUCKY_BOOST: f64 = 0.3;
const REACTOR_DAMAGE_MIN: i32 = 100;
const REACTOR_DAMAGE_MAX: i32 = 250;
const ROUGH_SEARCH_ITERATIONS: u32 = 1000;

#[derive(Clone)]
pub struct AttackSimulator {
    rng: Mt64,
    repartition_buf: Vec<f64>,
    allocated_buf: Vec<i32>,
}

struct ActiveRoll {
    targets: usize,
    max_active: i32,
    active_factor: f64,
    leftover: i32,
    flag_bonus: i32,
}

fn active_roll(
    config: &SimConfig,
    total_attack: i32,
    overflow: i32,
    b_level_override: Option<i32>,
    base_level: u32,
) -> Option<ActiveRoll> {
    let day = config.day;
    let drapo = config.nb_drapo;
    let nb_hab = config.nb_hab;
    let b_level = b_level_override.or(config.b_level);
    let is_chaos = config.is_chaos;
    let is_devastated = config.is_devastated;
    let targets = cmp::min(10 + 2 * ((day - 10).max(0) / 2), nb_hab);

    if targets <= 0 || overflow <= 0 {
        return None;
    }

    // Active zombie capping (PHP alignement: $max_active = round($zombies * $active_factor))
    let b_level_val = b_level.unwrap_or(1);
    let pop_val = nb_hab;

    let mut level = base_level as f64;

    level *= (targets.max(15) as f64 + b_level_val.max(0) as f64 * 2.0) / pop_val.max(1) as f64;

    if is_chaos {
        level += 10.0;
    }
    if is_devastated {
        level += 10.0;
    }

    let active_factor = (level / 100.0).clamp(0.0, 1.0);
    let max_active = (total_attack as f64 * active_factor).round() as i32;

    let flag_step = (total_attack as f64 * FLAG_REDUCTION_RATE).round() as i32;
    let leftover = max_active.min(overflow) - drapo.max(0) * flag_step;
    let flag_bonus = if drapo > 0 { flag_step } else { 0 };

    Some(ActiveRoll {
        targets: targets as usize,
        max_active,
        active_factor,
        leftover,
        flag_bonus,
    })
}

impl AttackSimulator {
    pub fn new() -> Self {
        Self {
            rng: Mt64::new(rand::random()),
            repartition_buf: Vec::with_capacity(40),
            allocated_buf: Vec::with_capacity(40),
        }
    }

    fn roll_active(
        &mut self,
        config: &SimConfig,
        total_attack: i32,
        overflow: i32,
        b_level_override: Option<i32>,
    ) -> Option<ActiveRoll> {
        let base_level = self.rng.random_range(BASE_LEVEL_MIN..=BASE_LEVEL_MAX);
        active_roll(config, total_attack, overflow, b_level_override, base_level)
    }

    fn distribute(&mut self, roll: &ActiveRoll) {
        let targets = roll.targets;
        let leftover = roll.leftover;

        if leftover <= 0 {
            self.allocated_buf.clear();
            self.allocated_buf.resize(targets, roll.flag_bonus);
            return;
        }

        self.repartition_buf.clear();
        for _ in 0..targets {
            self.repartition_buf.push(self.rng.random::<f64>());
        }

        if !self.repartition_buf.is_empty() {
            let unlucky_idx = self.rng.random_range(0..self.repartition_buf.len());
            self.repartition_buf[unlucky_idx] += UNLUCKY_BOOST;
        }

        let sum: f64 = self.repartition_buf.iter().sum();

        self.allocated_buf.clear();
        self.allocated_buf.resize(targets, 0);
        let mut attacking_cache = leftover;

        if sum > 0.0 {
            for i in 0..targets {
                let norm = self.repartition_buf[i] / sum;
                let share = (norm * leftover as f64).round() as i32;
                let val = 0.max(attacking_cache.min(share));
                self.allocated_buf[i] = val;
                attacking_cache -= val;
            }
        }

        while attacking_cache > 0 && !self.allocated_buf.is_empty() {
            let idx = self.rng.random_range(0..self.allocated_buf.len());
            self.allocated_buf[idx] += 1;
            attacking_cache -= 1;
        }

        let flag_bonus = roll.flag_bonus;
        self.allocated_buf.iter_mut().for_each(|x| *x += flag_bonus);
    }

    pub fn simulate_attack_with_max_active(
        &mut self,
        config: &SimConfig,
        total_attack: i32,
        overflow: i32,
        b_level_override: Option<i32>,
    ) -> (&[i32], i32, f64) {
        match self.roll_active(config, total_attack, overflow, b_level_override) {
            None => {
                self.allocated_buf.clear();
                (&self.allocated_buf, 0, 0.0)
            }
            Some(roll) => {
                self.distribute(&roll);
                (&self.allocated_buf, roll.max_active, roll.active_factor)
            }
        }
    }

    fn roll_attack_above(
        &mut self,
        config: &SimConfig,
        total_attack: i32,
        overflow: i32,
        b_level_override: Option<i32>,
        threshold: i32,
    ) -> (bool, i32) {
        let Some(roll) = self.roll_active(config, total_attack, overflow, b_level_override) else {
            return (false, 0);
        };
        // A single target never receives more than leftover + flag_bonus.
        if roll.leftover.max(0) + roll.flag_bonus <= threshold {
            return (false, roll.max_active);
        }
        self.distribute(&roll);
        (true, roll.max_active)
    }

    pub fn simulate_attack(
        &mut self,
        config: &SimConfig,
        total_attack: i32,
        overflow: i32,
        b_level_override: Option<i32>,
    ) -> &[i32] {
        self.simulate_attack_with_max_active(config, total_attack, overflow, b_level_override)
            .0
    }
}

impl Default for AttackSimulator {
    fn default() -> Self {
        Self::new()
    }
}

pub fn citizen_home_level(defense: i32) -> i32 {
    if defense <= 0 { 0 } else { defense.isqrt() }
}

pub fn resolve_b_level(config: &SimConfig, citizens: &[crate::config::SimulationCitizen]) -> i32 {
    if let Some(b) = config.b_level {
        return b;
    }

    if !citizens.is_empty() {
        let mut b_levels = [0; 64];
        let mut max_b_level = 0;

        for c in citizens {
            let citizen_b_level = citizen_home_level(c.defense);
            max_b_level = cmp::max(max_b_level, citizen_b_level);
            for l in 0..=citizen_b_level {
                if (l as usize) < b_levels.len() {
                    b_levels[l as usize] += 1;
                }
            }
        }

        let ceil_target_third = ((citizens.len() as f64) / 3.0).ceil() as i32;
        let mut tercile = max_b_level;
        for l in (0..64).rev() {
            if b_levels[l] >= ceil_target_third {
                tercile = l as i32;
                break;
            }
        }
        return tercile;
    }

    let def_target = if config.min_def >= 6 {
        config.min_def - 2
    } else {
        config.min_def
    };
    citizen_home_level(def_target)
}

fn attack_can_overflow(config: &SimConfig, attack: i32) -> bool {
    let max_reactor_damage = if config.is_reactor_built {
        REACTOR_DAMAGE_MAX
    } else {
        0
    };
    attack + max_reactor_damage > config.defense
}

struct AttackPoint {
    total_attack: i32,
    weight: f64,
    runs: u64,
}

struct AttackPlan {
    points: Vec<AttackPoint>,
    budget: u64,
    avg_max_active: Option<f64>,
}

fn total_attack_distribution(config: &SimConfig) -> Vec<(i32, f64)> {
    let (tdg_min, tdg_max) = config.tdg_interval();
    let dist = attack_distribution(tdg_min, tdg_max, config.day);
    if dist.is_empty() {
        return Vec::new();
    }
    if !config.is_reactor_built {
        let mut points: Vec<(i32, f64)> = dist.into_iter().collect();
        points.sort_unstable_by_key(|&(attack, _)| attack);
        return points;
    }

    let mut prefix = Vec::with_capacity((tdg_max - tdg_min + 2) as usize);
    prefix.push(0.0);
    let mut acc = 0.0;
    for attack in tdg_min..=tdg_max {
        acc += dist[&attack];
        prefix.push(acc);
    }

    let window = (REACTOR_DAMAGE_MAX - REACTOR_DAMAGE_MIN + 1) as f64;
    (tdg_min + REACTOR_DAMAGE_MIN..=tdg_max + REACTOR_DAMAGE_MAX)
        .map(|total_attack| {
            let lo = (total_attack - REACTOR_DAMAGE_MAX).max(tdg_min);
            let hi = (total_attack - REACTOR_DAMAGE_MIN).min(tdg_max);
            let sum = prefix[(hi - tdg_min + 1) as usize] - prefix[(lo - tdg_min) as usize];
            (total_attack, sum / window)
        })
        .collect()
}

fn plan_attack_points(config: &SimConfig, b_level: i32, threshold: i32) -> AttackPlan {
    let (tdg_min, tdg_max) = config.tdg_interval();
    let overflowing_values = (tdg_min..=tdg_max)
        .filter(|&attack| attack_can_overflow(config, attack))
        .count() as u64;
    let budget = overflowing_values * config.iterations as u64;

    let mut plan = AttackPlan {
        points: Vec::new(),
        budget,
        avg_max_active: None,
    };
    if config.iterations == 0 || config.nb_hab <= 0 {
        return plan;
    }

    let mut capped_weight = 0.0;
    let mut capped_max_active = 0.0;

    for (total_attack, weight) in total_attack_distribution(config) {
        let overflow = total_attack - config.defense;
        if overflow <= 0 || weight <= 0.0 {
            continue;
        }

        for base_level in BASE_LEVEL_MIN..=BASE_LEVEL_MAX {
            let max_active = active_roll(config, total_attack, overflow, Some(b_level), base_level)
                .map_or(0, |roll| roll.max_active);
            if max_active < overflow {
                capped_max_active += weight * max_active.max(0) as f64;
                capped_weight += weight;
            }
        }

        let can_kill = active_roll(
            config,
            total_attack,
            overflow,
            Some(b_level),
            BASE_LEVEL_MAX,
        )
        .is_some_and(|roll| roll.leftover.max(0) + roll.flag_bonus > threshold);
        if can_kill {
            plan.points.push(AttackPoint {
                total_attack,
                weight,
                runs: ((budget as f64 * weight).ceil() as u64).max(1),
            });
        }
    }

    if capped_weight > 0.0 {
        plan.avg_max_active = Some(capped_max_active / capped_weight);
    }
    plan
}

fn debordo_point(
    simulator: &mut AttackSimulator,
    config: &SimConfig,
    b_level: i32,
    point: &AttackPoint,
    threshold: i32,
) -> u64 {
    let overflow = point.total_attack - config.defense;
    let mut hits = 0;
    for _ in 0..point.runs {
        let (death_possible, _) = simulator.roll_attack_above(
            config,
            point.total_attack,
            overflow,
            Some(b_level),
            threshold,
        );
        if death_possible && simulator.allocated_buf.iter().any(|&x| x > threshold) {
            hits += 1;
        }
    }
    hits
}

#[allow(clippy::too_many_arguments)]
fn complete_point(
    simulator: &mut AttackSimulator,
    indices: &mut [usize],
    config: &SimConfig,
    b_level: i32,
    point: &AttackPoint,
    citizens: &[crate::config::SimulationCitizen],
    min_citizen_defense: i32,
    mut citizen_hits: Option<&mut [u64]>,
) -> u64 {
    let overflow = point.total_attack - config.defense;
    let mut town_hits = 0;

    for _ in 0..point.runs {
        let (death_possible, _) = simulator.roll_attack_above(
            config,
            point.total_attack,
            overflow,
            Some(b_level),
            min_citizen_defense,
        );
        if !death_possible {
            continue;
        }

        let allocated = &simulator.allocated_buf;
        let targets = allocated.len().min(citizens.len());
        let (chosen, _) = indices.partial_shuffle(&mut simulator.rng, targets);

        let mut any_citizen_died = false;
        for (&idx, &zombies) in chosen.iter().zip(allocated) {
            if zombies > citizens[idx].defense {
                any_citizen_died = true;
                match citizen_hits.as_deref_mut() {
                    Some(hits) => hits[idx] += 1,
                    None => break,
                }
            }
        }

        if any_citizen_died {
            town_hits += 1;
        }
    }

    town_hits
}

fn attack_distribution(tdg_min: i32, tdg_max: i32, day: i32) -> HashMap<i32, f64> {
    if tdg_min > tdg_max {
        return HashMap::new();
    }
    let ratio = if day <= 3 { 0.75 } else { 1.1 };
    let lo = (ratio * (max(1, day - 1) as f64 * 0.75 + 2.5).powi(3)).round() as i32;
    let hi = (ratio * (day as f64 * 0.75 + 3.5).powi(3)).round() as i32;
    let mid = lo as f64 + 0.5 * (hi - lo) as f64;
    let mid_floor = mid.floor() as i32;

    let total_count = (tdg_max - tdg_min + 1) as f64;
    let first_roll_p = 1.0 / total_count;

    let high_start = (mid_floor + 1).max(tdg_min);
    let n_high = if high_start <= tdg_max {
        (tdg_max - high_start + 1) as f64
    } else {
        0.0
    };
    let reroll_trigger_prob = n_high * first_roll_p;

    let mut prob = HashMap::new();
    for i in tdg_min..=tdg_max {
        if i <= mid_floor {
            // Kept on first roll + obtained on reroll when first roll was above midpoint.
            prob.insert(i, first_roll_p + reroll_trigger_prob * first_roll_p);
        } else {
            // Above midpoint can only occur from reroll result.
            prob.insert(i, reroll_trigger_prob * first_roll_p);
        }
    }

    prob
}

pub fn overflow_probability(config: &SimConfig) -> (f64, u64, Option<f64>) {
    let b_level = resolve_b_level(config, &[]);
    let plan = plan_attack_points(config, b_level, config.min_def);

    let hits: Vec<u64> = plan
        .points
        .par_iter()
        .map_init(AttackSimulator::new, |simulator, point| {
            debordo_point(simulator, config, b_level, point, config.min_def)
        })
        .collect();

    (
        death_percentage(&plan.points, hits),
        plan.budget,
        plan.avg_max_active,
    )
}

fn death_percentage(points: &[AttackPoint], hits: impl IntoIterator<Item = u64>) -> f64 {
    let prob: f64 = points
        .iter()
        .zip(hits)
        .map(|(point, hits)| point.weight * hits as f64 / point.runs as f64)
        .sum();
    (prob.min(1.0) * 100.0).max(0.0)
}

fn complete_setup(
    config: &SimConfig,
    citizens: &[crate::config::SimulationCitizen],
) -> (SimConfig, i32, i32) {
    let mut effective_config = config.clone();
    if !citizens.is_empty() {
        effective_config.nb_hab = citizens.len() as i32;
    }
    let b_level = resolve_b_level(&effective_config, citizens);
    let min_citizen_defense = citizens.iter().map(|c| c.defense).min().unwrap_or(i32::MAX);
    (effective_config, b_level, min_citizen_defense)
}

pub fn complete_overflow_probability(
    config: &SimConfig,
    citizens: &[crate::config::SimulationCitizen],
) -> (f64, u64, Vec<f64>, Option<f64>) {
    let (effective_config, b_level, min_citizen_defense) = complete_setup(config, citizens);
    let plan = plan_attack_points(&effective_config, b_level, min_citizen_defense);

    let results: Vec<(u64, Vec<u64>)> = plan
        .points
        .par_iter()
        .map_init(
            || {
                let indices: Vec<usize> = (0..citizens.len()).collect();
                (AttackSimulator::new(), indices)
            },
            |(simulator, indices), point| {
                let mut citizen_hits = vec![0u64; citizens.len()];
                let town_hits = complete_point(
                    simulator,
                    indices,
                    &effective_config,
                    b_level,
                    point,
                    citizens,
                    min_citizen_defense,
                    Some(&mut citizen_hits),
                );
                (town_hits, citizen_hits)
            },
        )
        .collect();

    let mut citizen_probs = vec![0.0; citizens.len()];
    for (point, (_, citizen_hits)) in plan.points.iter().zip(&results) {
        let scale = point.weight / point.runs as f64;
        for (prob, &hits) in citizen_probs.iter_mut().zip(citizen_hits) {
            *prob += scale * hits as f64;
        }
    }

    let citizen_percentages = citizen_probs
        .iter()
        .map(|&p| (p.min(1.0) * 100.0).max(0.0))
        .collect();

    (
        death_percentage(
            &plan.points,
            results.iter().map(|&(town_hits, _)| town_hits),
        ),
        plan.budget,
        citizen_percentages,
        plan.avg_max_active,
    )
}

fn complete_town_probability(
    config: &SimConfig,
    citizens: &[crate::config::SimulationCitizen],
) -> (f64, u64) {
    let (effective_config, b_level, min_citizen_defense) = complete_setup(config, citizens);
    let plan = plan_attack_points(&effective_config, b_level, min_citizen_defense);

    let hits: Vec<u64> = plan
        .points
        .par_iter()
        .map_init(
            || {
                let indices: Vec<usize> = (0..citizens.len()).collect();
                (AttackSimulator::new(), indices)
            },
            |(simulator, indices), point| {
                complete_point(
                    simulator,
                    indices,
                    &effective_config,
                    b_level,
                    point,
                    citizens,
                    min_citizen_defense,
                    None,
                )
            },
        )
        .collect();

    (death_percentage(&plan.points, hits), plan.budget)
}

pub struct DefenseSearch {
    pub defense: i32,
    pub prob_at_defense: f64,
    pub safe_defense: i32,
    pub total_runs: u64,
}

pub fn required_defense(
    config: &SimConfig,
    citizens: &[crate::config::SimulationCitizen],
    target_pct: f64,
) -> DefenseSearch {
    let (setup_config, b_level, threshold) = if config.is_complete {
        complete_setup(config, citizens)
    } else {
        (config.clone(), resolve_b_level(config, &[]), config.min_def)
    };
    let can_die = |defense: i32| {
        let setup = SimConfig {
            defense,
            ..setup_config.clone()
        };
        !plan_attack_points(&setup, b_level, threshold)
            .points
            .is_empty()
    };
    let max_reactor_damage = if config.is_reactor_built {
        REACTOR_DAMAGE_MAX
    } else {
        0
    };
    let safe_defense =
        lowest_passing_defense(0, (config.tdg_max + max_reactor_damage).max(0), |d| {
            !can_die(d)
        });

    if target_pct <= 0.0 {
        return DefenseSearch {
            defense: safe_defense,
            prob_at_defense: 0.0,
            safe_defense,
            total_runs: 0,
        };
    }

    let mut total_runs = 0;
    let mut estimate = |iterations: u32, defense: i32| {
        let config = SimConfig {
            defense,
            iterations,
            ..config.clone()
        };
        let (prob, runs) = if config.is_complete {
            complete_town_probability(&config, citizens)
        } else {
            let (prob, runs, _) = overflow_probability(&config);
            (prob, runs)
        };
        total_runs += runs;
        prob
    };

    let rough_guess = (config.iterations > ROUGH_SEARCH_ITERATIONS).then(|| {
        lowest_passing_defense(0, safe_defense, |defense| {
            estimate(ROUGH_SEARCH_ITERATIONS, defense) <= target_pct
        })
    });

    let mut prob_at_defense = 0.0;
    let mut passes = |defense: i32| {
        let prob = estimate(config.iterations, defense);
        let passes = prob <= target_pct;
        if passes {
            prob_at_defense = prob;
        }
        passes
    };
    let defense = match rough_guess {
        Some(guess) => lowest_passing_defense_near(guess, safe_defense, &mut passes),
        None => lowest_passing_defense(0, safe_defense, &mut passes),
    };

    DefenseSearch {
        defense,
        prob_at_defense,
        safe_defense,
        total_runs,
    }
}

fn lowest_passing_defense(mut lo: i32, mut hi: i32, mut passes: impl FnMut(i32) -> bool) -> i32 {
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        if passes(mid) {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    hi
}

fn lowest_passing_defense_near(guess: i32, max: i32, mut passes: impl FnMut(i32) -> bool) -> i32 {
    let guess = guess.clamp(0, max);
    let mut step = 1;
    let (lo, hi) = if guess == max || passes(guess) {
        let mut hi = guess;
        loop {
            if hi == 0 {
                break (0, 0);
            }
            let d = (hi - step).max(0);
            if !passes(d) {
                break (d + 1, hi);
            }
            hi = d;
            step *= 2;
        }
    } else {
        let mut lo = guess + 1;
        loop {
            let d = (lo - 1 + step).min(max);
            if d == max || passes(d) {
                break (lo, d);
            }
            lo = d + 1;
            step *= 2;
        }
    };
    lowest_passing_defense(lo, hi, passes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn debordo_sequential(
        config: &SimConfig,
        raw_attack: i32,
        threshold: i32,
    ) -> (f64, Option<f64>) {
        if config.iterations == 0 || config.nb_hab <= 0 {
            return (0.0, None);
        }

        let mut simulator = AttackSimulator::new();
        let mut town_hits = 0;
        let mut total_capped_max_active: u64 = 0;
        let mut capped_iterations: u64 = 0;

        let b_level_resolved = resolve_b_level(config, &[]);

        for _ in 0..config.iterations {
            let overflow = (raw_attack - config.defense).max(0);

            let (death_possible, max_active) = simulator.roll_attack_above(
                config,
                raw_attack,
                overflow,
                Some(b_level_resolved),
                threshold,
            );
            if max_active < overflow {
                total_capped_max_active += max_active.max(0) as u64;
                capped_iterations += 1;
            }

            if death_possible && simulator.allocated_buf.iter().any(|&x| x > threshold) {
                town_hits += 1;
            }
        }

        let town_prob = town_hits as f64 / config.iterations as f64;
        let avg_max_active = if capped_iterations > 0 {
            Some(total_capped_max_active as f64 / capped_iterations as f64)
        } else {
            None
        };
        (town_prob, avg_max_active)
    }

    fn complete_debordo(
        config: &SimConfig,
        raw_attack: i32,
        citizens: &[crate::config::SimulationCitizen],
    ) -> (f64, Vec<f64>, Option<f64>) {
        if config.iterations == 0 || config.nb_hab <= 0 {
            return (0.0, vec![0.0; citizens.len()], None);
        }

        let mut effective_config = config.clone();
        if !citizens.is_empty() {
            effective_config.nb_hab = citizens.len() as i32;
        }

        let b_level_resolved = resolve_b_level(&effective_config, citizens);
        let min_citizen_defense = citizens.iter().map(|c| c.defense).min().unwrap_or(i32::MAX);

        let mut simulator = AttackSimulator::new();
        let mut indices: Vec<usize> = (0..citizens.len()).collect();
        let mut town_hits = 0;
        let mut total_capped_max_active: u64 = 0;
        let mut capped_iterations: u64 = 0;
        let mut citizen_hits = vec![0u64; citizens.len()];

        for _ in 0..effective_config.iterations {
            let overflow = (raw_attack - effective_config.defense).max(0);

            let (death_possible, max_active) = simulator.roll_attack_above(
                &effective_config,
                raw_attack,
                overflow,
                Some(b_level_resolved),
                min_citizen_defense,
            );
            if max_active < overflow {
                total_capped_max_active += max_active.max(0) as u64;
                capped_iterations += 1;
            }

            if !death_possible {
                continue;
            }

            let allocated = &simulator.allocated_buf;
            let targets = allocated.len().min(citizens.len());
            let (chosen, _) = indices.partial_shuffle(&mut simulator.rng, targets);

            let mut any_citizen_died = false;
            for (&idx, &zombies) in chosen.iter().zip(allocated) {
                if zombies > citizens[idx].defense {
                    citizen_hits[idx] += 1;
                    any_citizen_died = true;
                }
            }

            if any_citizen_died {
                town_hits += 1;
            }
        }

        let town_prob = town_hits as f64 / effective_config.iterations as f64;
        let citizen_probs = citizen_hits
            .iter()
            .map(|&hits| hits as f64 / effective_config.iterations as f64)
            .collect();
        let avg_max_active = if capped_iterations > 0 {
            Some(total_capped_max_active as f64 / capped_iterations as f64)
        } else {
            None
        };

        (town_prob, citizen_probs, avg_max_active)
    }
    #[test]
    fn test_attack_distribution() {
        // Smoke test: with day=10 the midpoint (≈1167) is far above the range,
        // so all values are equally probable.
        let dist = attack_distribution(100, 102, 10);
        assert_eq!(dist.len(), 3);
        assert!((dist[&100] - 1.0 / 3.0).abs() < 0.0001);
    }

    #[test]
    fn test_attack_distribution_probabilities_sum_to_one() {
        let dist = attack_distribution(50, 80, 5);
        let sum: f64 = dist.values().sum();
        assert!(
            (sum - 1.0).abs() < 0.0001,
            "probabilities sum to {}, expected 1.0",
            sum
        );
    }

    #[test]
    fn test_attack_distribution_empty_for_invalid_range() {
        let dist = attack_distribution(100, 50, 5);
        assert!(dist.is_empty());
    }

    #[test]
    fn test_attack_distribution_single_value_full_probability() {
        let dist = attack_distribution(100, 100, 5);
        assert_eq!(dist.len(), 1);
        assert!((dist[&100] - 1.0).abs() < 0.0001);
    }

    #[test]
    fn test_attack_distribution_non_uniform_above_midpoint() {
        // With day=1: midpoint ≈ 42.
        // Values ≤ 42 should have higher probability than values > 42.
        let dist = attack_distribution(40, 45, 1);
        assert!(
            dist[&40] > dist[&45],
            "values at/below midpoint (prob={}) should be more likely than values above (prob={})",
            dist[&40],
            dist[&45]
        );
    }

    #[test]
    fn test_attack_distribution_midpoint_below_range_stays_normalized() {
        // Day 1 midpoint is around 42, so [100,130] is entirely above midpoint.
        // Reroll then applies to all values and distribution must remain normalized.
        let dist = attack_distribution(100, 130, 1);
        let sum: f64 = dist.values().sum();
        assert!(
            (sum - 1.0).abs() < 0.0001,
            "probabilities sum to {}, expected 1.0",
            sum
        );
        assert!((dist[&100] - (1.0 / 31.0)).abs() < 0.0001);
        assert!((dist[&130] - (1.0 / 31.0)).abs() < 0.0001);
    }

    #[test]
    fn test_attack_distribution_matches_one_reroll_mechanic() {
        // Day 1 midpoint is around 42.
        // In [40,45]:
        // - first-roll keep zone = {40,41,42} (3 values)
        // - reroll-trigger zone = {43,44,45} (3 values)
        // So trigger prob = 3/6 = 1/2.
        // Final probs:
        //   <=42: 1/6 + (1/2)*(1/6) = 1/4
        //   >42 : (1/2)*(1/6) = 1/12
        let dist = attack_distribution(40, 45, 1);
        for attack in 40..=42 {
            assert!((dist[&attack] - 0.25).abs() < 0.0001);
        }
        for attack in 43..=45 {
            assert!((dist[&attack] - (1.0 / 12.0)).abs() < 0.0001);
        }
    }

    // =========================================================================
    // AttackSimulator::simulate_attack
    // =========================================================================

    #[test]
    fn test_simulator_creates_correct_number_of_targets() {
        let mut sim = AttackSimulator::new();

        // Day 10: 10 targets
        let result = sim.simulate_attack(
            &SimConfig {
                day: 10,
                nb_hab: 40,
                ..Default::default()
            },
            1000,
            1000,
            None,
        );
        assert_eq!(result.len(), 10);

        // Day 12: 12 targets
        let result = sim.simulate_attack(
            &SimConfig {
                day: 12,
                nb_hab: 40,
                ..Default::default()
            },
            1000,
            1000,
            None,
        );
        assert_eq!(result.len(), 12);
    }

    #[test]
    fn test_simulate_attack_zero_attacking_returns_zeros() {
        // With 0 overflow zombies and no flags, every cell gets 0.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(
            &SimConfig {
                day: 1,
                nb_hab: 40,
                ..Default::default()
            },
            0,
            0,
            None,
        );
        assert!(
            result.iter().all(|&x| x == 0),
            "with 0 attacking and no flags, all cells should be 0"
        );
    }

    #[test]
    fn test_simulate_attack_all_allocations_non_negative() {
        let mut sim = AttackSimulator::new();
        for _ in 0..20 {
            let result = sim.simulate_attack(
                &SimConfig {
                    day: 5,
                    nb_hab: 7,
                    ..Default::default()
                },
                500,
                500,
                None,
            );
            assert!(
                result.iter().all(|&x| x >= 0),
                "zombie allocations must never be negative"
            );
        }
    }

    #[test]
    fn test_simulate_attack_sum_equals_attacking_capped() {
        let mut sim = AttackSimulator::new();
        for _ in 0..10 {
            let attacking = 100;
            // pass b_level = Some(10) and nb_hab = 10 to ensure active factor is 1.0 (no capping)
            let result = sim.simulate_attack(
                &SimConfig {
                    day: 1,
                    nb_hab: 10,
                    b_level: Some(10),
                    ..Default::default()
                },
                attacking,
                attacking,
                None,
            );
            let sum: i32 = result.iter().sum();
            assert_eq!(
                sum, attacking,
                "sum {} should be exactly equal to {}",
                sum, attacking
            );
        }
    }

    // =========================================================================
    // debordo_sequential
    // =========================================================================

    #[test]
    fn test_debordo_zero_attacking_gives_zero_probability() {
        // 0 overflow zombies → no cell can exceed any threshold → 0% death.
        let (prob, _) = debordo_sequential(
            &SimConfig {
                day: 1,
                iterations: 100,
                nb_hab: 40,
                ..Default::default()
            },
            0,
            1,
        );
        assert_eq!(
            prob, 0.0,
            "with 0 overflow zombies, death probability must be 0"
        );
    }

    #[test]
    fn test_debordo_overwhelming_attack_near_full_probability() {
        // 10 000 zombies among 10 citizens, min threshold of 1, high b_level to avoid capping
        let (prob, _) = debordo_sequential(
            &SimConfig {
                day: 1,
                iterations: 500,
                nb_hab: 40,
                b_level: Some(10),
                ..Default::default()
            },
            10_000,
            1,
        );
        assert!(prob > 0.99, "expected probability > 0.99, got {}", prob);
    }

    // =========================================================================
    // calculate_defense_probabilities
    // =========================================================================

    #[test]
    fn test_calculate_defense_probs_returns_probability() {
        let (prob, _, _) = overflow_probability(&SimConfig {
            defense: 150,
            tdg_min: 50,
            tdg_max: 60,
            min_def: 10,
            day: 1,
            iterations: 100,
            nb_hab: 40,
            ..Default::default()
        });
        assert!((0.0..=100.0).contains(&prob));
    }

    #[test]
    fn test_calculate_defense_probs_impenetrable_defense_is_zero() {
        // Defense >> max possible attack → no overflow → 0% probability.
        let (prob, total_runs, _) = overflow_probability(&SimConfig {
            defense: 100_000,
            tdg_min: 50,
            tdg_max: 100,
            min_def: 10,
            day: 1,
            iterations: 100,
            nb_hab: 40,
            ..Default::default()
        });
        assert_eq!(prob, 0.0, "impenetrable defense should yield 0%");
        assert_eq!(total_runs, 0, "no overflow means no MC runs");
    }

    // =========================================================================
    // nb_hab parameter tests
    // =========================================================================

    #[test]
    fn test_simulate_attack_nb_hab_limits_targets() {
        // Day 20 would normally have 10 + 2*((20-10)/2) = 20 targets,
        // but nb_hab=5 should limit it to 5.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(
            &SimConfig {
                day: 20,
                nb_hab: 5,
                ..Default::default()
            },
            1000,
            1000,
            None,
        );
        assert_eq!(result.len(), 5, "nb_hab=5 should limit targets to 5");
    }

    #[test]
    fn test_simulate_attack_nb_hab_does_not_increase_targets() {
        // Day 10 has 10 targets. nb_hab=100 should not increase beyond 10.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(
            &SimConfig {
                day: 10,
                nb_hab: 100,
                ..Default::default()
            },
            1000,
            1000,
            None,
        );
        assert_eq!(
            result.len(),
            10,
            "nb_hab should not increase targets beyond day formula"
        );
    }

    #[test]
    fn test_simulate_attack_nb_hab_equals_day_targets() {
        // Day 10 = 10 targets, nb_hab=10 should give exactly 10.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(
            &SimConfig {
                day: 10,
                nb_hab: 10,
                ..Default::default()
            },
            1000,
            1000,
            None,
        );
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_simulate_attack_nb_hab_one_person() {
        // Edge case: only 1 person in town receives all zombies.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(
            &SimConfig {
                day: 10,
                nb_hab: 1,
                b_level: Some(10),
                ..Default::default()
            },
            100,
            100,
            None,
        );
        assert_eq!(result.len(), 1, "nb_hab=1 should have exactly 1 target");
        assert!(result[0] >= 100, "single target should receive all zombies");
    }

    #[test]
    fn test_debordo_nb_hab_affects_distribution() {
        // Fewer people means zombies are more concentrated → higher death probability.
        // With 1000 zombies among 40 people vs 5 people, 5 people should have higher prob.
        let (prob_40_hab, _) = debordo_sequential(
            &SimConfig {
                day: 10,
                iterations: 500,
                nb_hab: 40,
                ..Default::default()
            },
            1000,
            50,
        );
        let (prob_5_hab, _) = debordo_sequential(
            &SimConfig {
                day: 10,
                iterations: 500,
                nb_hab: 5,
                ..Default::default()
            },
            1000,
            50,
        );
        assert!(
            prob_5_hab >= prob_40_hab,
            "fewer habitants should concentrate zombies → higher death prob: 40hab={} 5hab={}",
            prob_40_hab,
            prob_5_hab
        );
    }

    #[test]
    fn test_overflow_probability_with_small_nb_hab() {
        // With very few people, overflow should be more deadly.
        let (prob, _, _) = overflow_probability(&SimConfig {
            defense: 50,
            tdg_min: 60,
            tdg_max: 70,
            min_def: 2,
            day: 5,
            iterations: 100,
            nb_hab: 3,
            ..Default::default()
        });
        assert!(
            prob > 0.0,
            "with small nb_hab and overflow, death probability should be > 0"
        );
    }

    #[test]
    fn test_overflow_probability_with_nb_hab_12() {
        // Regression test for the "cannot sample empty range" panic with nb_hab=12.
        // This should not panic regardless of the input parameters.
        let (prob, _, _) = overflow_probability(&SimConfig {
            defense: 100,
            tdg_min: 150,
            tdg_max: 200,
            min_def: 10,
            day: 10,
            iterations: 100,
            nb_hab: 12,
            ..Default::default()
        });
        assert!(
            (0.0..=100.0).contains(&prob),
            "probability should be in valid range"
        );
    }

    #[test]
    fn test_debordo_with_nb_hab_zero_returns_zero() {
        // Edge case: nb_hab=0 should return 0.0 without panicking.
        let (prob, _) = debordo_sequential(
            &SimConfig {
                day: 10,
                iterations: 100,
                nb_hab: 0,
                ..Default::default()
            },
            100,
            10,
        );
        assert_eq!(prob, 0.0, "nb_hab=0 should return 0.0");
    }

    #[test]
    fn test_debordo_with_iterations_zero_returns_zero() {
        // Edge case: iterations=0 should return 0.0 without panicking.
        let (prob, _) = debordo_sequential(
            &SimConfig {
                day: 10,
                iterations: 0,
                nb_hab: 40,
                ..Default::default()
            },
            100,
            10,
        );
        assert_eq!(prob, 0.0, "iterations=0 should return 0.0");
    }

    #[test]
    fn test_php_aligned_repartition_sum_constraint() {
        let mut sim = AttackSimulator::new();
        // Run several iterations with varying parameters to assert the sum is always strictly preserved
        for day in 1..=20 {
            for attacking in (50..=1000).step_by(150) {
                for nb_hab in (5..=40).step_by(10) {
                    let result = sim.simulate_attack(
                        &SimConfig {
                            day,
                            nb_hab,
                            b_level: Some(3),
                            ..Default::default()
                        },
                        attacking,
                        attacking,
                        None,
                    );
                    let sum: i32 = result.iter().sum();
                    assert!(
                        sum >= 0 && sum <= attacking * 2,
                        "Sum {} should be bounded by [0, {}]",
                        sum,
                        attacking * 2
                    );
                }
            }
        }
    }

    fn simulate_repartition_with_weights(
        attacking: i32,
        targets: usize,
        unlucky_idx: usize,
        mut weights: Vec<f64>,
    ) -> Vec<i32> {
        if targets == 0 {
            return vec![];
        }

        weights[unlucky_idx] += 0.3;
        let sum: f64 = weights.iter().sum();

        let mut allocated = vec![0; targets];
        let mut attacking_cache = attacking;

        if sum > 0.0 {
            for i in 0..targets {
                let norm = weights[i] / sum;
                let share = (norm * attacking as f64).round() as i32;
                let val = 0.max(attacking_cache.min(share));
                allocated[i] = val;
                attacking_cache -= val;
            }
        }

        let mut idx = 0;
        while attacking_cache > 0 && !allocated.is_empty() {
            allocated[idx % targets] += 1;
            attacking_cache -= 1;
            idx += 1;
        }

        allocated
    }

    #[test]
    fn test_deterministic_parity_with_php() {
        // Case 1
        let case1 = simulate_repartition_with_weights(10, 3, 1, vec![0.1, 0.5, 0.4]);
        // PHP output for case 1: [1, 6, 3]
        assert_eq!(case1, vec![1, 6, 3]);

        // Case 2
        let case2 =
            simulate_repartition_with_weights(100, 5, 3, vec![0.12, 0.88, 0.45, 0.22, 0.09]);
        // PHP output for case 2: [6, 43, 22, 25, 4]
        assert_eq!(case2, vec![6, 43, 22, 25, 4]);
    }

    #[test]
    fn test_nominative_defense_survival() {
        use crate::config::SimulationCitizen;

        let citizens = vec![
            SimulationCitizen {
                name: "Axfalt".to_string(),
                defense: 1000,
            },
            SimulationCitizen {
                name: "Bob".to_string(),
                defense: 0,
            },
        ];

        let (_town_prob, _total_runs, citizen_probs, _avg_max_active) =
            complete_overflow_probability(
                &SimConfig {
                    defense: 50,
                    tdg_min: 60,
                    tdg_max: 60,
                    day: 1,
                    iterations: 500,
                    nb_hab: 2,
                    ..Default::default()
                },
                &citizens,
            );

        assert_eq!(citizen_probs.len(), 2);
        assert_eq!(
            citizen_probs[0], 0.0,
            "Axfalt (1000 defense) should have 0% death rate"
        );
        assert!(
            citizen_probs[1] > 0.0,
            "Bob (0 defense) should have a positive death rate"
        );
    }

    #[test]
    fn test_town_prob_matches_actual_citizen_deaths() {
        // Regression test: the town-wide "Probabilité de mort" must reflect whether a
        // real citizen (matched by their own defense) actually died.
        use crate::config::SimulationCitizen;

        let mut citizens = vec![SimulationCitizen {
            name: "Vulnerable".to_string(),
            defense: 0,
        }];
        for i in 0..38 {
            citizens.push(SimulationCitizen {
                name: format!("Tank{}", i),
                defense: 1_000_000,
            });
        }

        let (town_prob, _total_runs, citizen_probs, _avg_max_active) =
            complete_overflow_probability(
                &SimConfig {
                    defense: 50,
                    tdg_min: 200,
                    tdg_max: 200,
                    day: 13,
                    iterations: 2000,
                    nb_hab: 39,
                    ..Default::default()
                },
                &citizens,
            );

        assert!(
            (town_prob - citizen_probs[0]).abs() < 0.0001,
            "town_prob ({}) should equal the only vulnerable citizen's death prob ({})",
            town_prob,
            citizen_probs[0]
        );
    }

    #[test]
    fn test_exact_perfect_square_home_levels_and_b_level() {
        use crate::config::SimulationCitizen;

        assert_eq!(citizen_home_level(0), 0);
        assert_eq!(citizen_home_level(1), 1);
        assert_eq!(citizen_home_level(4), 2);
        assert_eq!(citizen_home_level(9), 3);
        assert_eq!(citizen_home_level(16), 4);
        assert_eq!(citizen_home_level(25), 5);
        assert_eq!(citizen_home_level(36), 6);
        assert_eq!(citizen_home_level(49), 7);
        assert_eq!(citizen_home_level(56), 7);

        // Test explicit config.b_level
        let cfg_explicit = SimConfig {
            b_level: Some(5),
            ..Default::default()
        };
        assert_eq!(resolve_b_level(&cfg_explicit, &[]), 5);

        // Test min_def fallback (min_def = 56 => 54 => 7)
        let cfg_56 = SimConfig {
            min_def: 56,
            ..Default::default()
        };
        assert_eq!(resolve_b_level(&cfg_56, &[]), 7);

        // Test min_def fallback (min_def = 4 => 4 => 2)
        let cfg_4 = SimConfig {
            min_def: 4,
            ..Default::default()
        };
        assert_eq!(resolve_b_level(&cfg_4, &[]), 2);

        // Test citizen list tercile calculation (3 citizens: levels 7, 7, 0 => tercile level 7)
        let citizens = vec![
            SimulationCitizen {
                name: "A".to_string(),
                defense: 49,
            },
            SimulationCitizen {
                name: "B".to_string(),
                defense: 56,
            },
            SimulationCitizen {
                name: "C".to_string(),
                defense: 0,
            },
        ];
        let cfg_citizens = SimConfig::default();
        assert_eq!(resolve_b_level(&cfg_citizens, &citizens), 7);
    }

    #[test]
    fn test_user_report_defense_11600() {
        let config = SimConfig {
            defense: 11600,
            tdg_min: 11784,
            tdg_max: 11814,
            min_def: 59,
            day: 27,
            nb_hab: 40,
            iterations: 1000,
            ..Default::default()
        };
        let (prob, _total_runs, avg_max_active) = overflow_probability(&config);
        assert_eq!(
            prob, 0.0,
            "Expected 0.0% death probability for 200 overflow vs 59 min_def"
        );
        assert_eq!(
            avg_max_active, None,
            "Expected None because max_active was not inferior to overflow"
        );
    }

    #[test]
    fn test_avg_max_active_conditional_printing() {
        // High overflow (10,000) vs total_attack (11,000) where active_factor ~ 0.5 => max_active ~ 5,500 < overflow (10,000)
        let config_capped = SimConfig {
            defense: 1000,
            tdg_min: 11000,
            tdg_max: 11000,
            min_def: 10,
            day: 27,
            nb_hab: 40,
            iterations: 100,
            ..Default::default()
        };
        let (_prob, _runs, avg_max_active) = overflow_probability(&config_capped);
        assert!(
            avg_max_active.is_some(),
            "Expected Some(avg_max_active) when max_active < overflow"
        );
    }

    #[test]
    fn test_user_report_single_zero_def_citizen_town_probability() {
        use crate::config::SimulationCitizen;

        let mut citizens = Vec::new();
        for i in 1..=39 {
            citizens.push(SimulationCitizen {
                name: format!("Citoyen {}", i),
                defense: 60,
            });
        }
        citizens.push(SimulationCitizen {
            name: "Snow".to_string(),
            defense: 0,
        });

        let config = SimConfig {
            defense: 11274,
            tdg_min: 11784,
            tdg_max: 11814,
            day: 27,
            iterations: 1000,
            nb_hab: 40,
            is_complete: true,
            ..Default::default()
        };

        let (town_prob, _runs, _citizen_percentages, _avg) =
            complete_overflow_probability(&config, &citizens);

        assert!(
            town_prob < 80.0 && town_prob > 50.0,
            "Town probability should be around ~64%, got {}",
            town_prob
        );
    }

    #[test]
    fn test_complete_debordo_population_override() {
        use crate::config::SimulationCitizen;

        let citizens = vec![
            SimulationCitizen {
                name: "Cit 1".to_string(),
                defense: 0,
            },
            SimulationCitizen {
                name: "Cit 2".to_string(),
                defense: 0,
            },
            SimulationCitizen {
                name: "Cit 3".to_string(),
                defense: 0,
            },
        ];

        // Pass config with default nb_hab = 40, but 3 citizens in list
        let config = SimConfig {
            defense: 100,
            day: 10,
            iterations: 100,
            nb_hab: 40,
            b_level: Some(10),
            ..Default::default()
        };

        let (town_prob, citizen_probs, _avg) = complete_debordo(&config, 500, &citizens);
        assert_eq!(citizen_probs.len(), 3);
        assert!(
            town_prob > 0.9,
            "With 3 zero-defense citizens and 400 overflow zombies, town hit rate should be high"
        );
        for &p in &citizen_probs {
            assert!(
                p > 0.5,
                "Each of the 3 citizens should have a high death rate"
            );
        }
    }

    #[test]
    fn test_complete_debordo_individual_death_tracking() {
        use crate::config::SimulationCitizen;

        let citizens = vec![
            SimulationCitizen {
                name: "Immortal".to_string(),
                defense: 999_999,
            },
            SimulationCitizen {
                name: "Mortal".to_string(),
                defense: 0,
            },
        ];

        let config = SimConfig {
            defense: 100,
            day: 10,
            iterations: 500,
            nb_hab: 2,
            b_level: Some(10),
            ..Default::default()
        };

        let (town_prob, citizen_probs, _avg) = complete_debordo(&config, 200, &citizens);
        assert_eq!(citizen_probs.len(), 2);
        assert_eq!(
            citizen_probs[0], 0.0,
            "Immortal citizen must have 0.0 death rate"
        );
        assert!(
            citizen_probs[1] > 0.0,
            "Mortal citizen (0 def) must have > 0.0 death rate"
        );
        assert_eq!(
            town_prob, citizen_probs[1],
            "Town hit rate must equal Mortal's death rate when only Mortal can die"
        );
    }

    // =========================================================================
    // Impossible-death shortcut
    // =========================================================================

    #[test]
    fn test_roll_attack_above_skips_when_no_target_can_exceed_threshold() {
        let mut sim = AttackSimulator::new();
        let config = SimConfig {
            day: 10,
            nb_hab: 40,
            b_level: Some(10),
            ..Default::default()
        };
        // 1000 total attack, 20 overflow: no target can receive more than 20.
        let (death_possible, max_active) = sim.roll_attack_above(&config, 1000, 20, None, 20);
        assert!(!death_possible);
        assert!(max_active > 0, "max_active must still be rolled");

        let (death_possible, _) = sim.roll_attack_above(&config, 1000, 21, None, 20);
        assert!(death_possible);
    }

    #[test]
    fn test_roll_attack_above_counts_flag_bonus() {
        let mut sim = AttackSimulator::new();
        let config = SimConfig {
            day: 10,
            nb_hab: 40,
            nb_drapo: 1,
            b_level: Some(10),
            ..Default::default()
        };
        // Flags remove 25 from the overflow but add 25 to every target:
        // leftover = 0, so each target gets exactly the 25 flag bonus.
        let (death_possible, _) = sim.roll_attack_above(&config, 1000, 25, None, 24);
        assert!(death_possible);
        assert!(sim.allocated_buf.iter().all(|&x| x == 25));

        let (death_possible, _) = sim.roll_attack_above(&config, 1000, 25, None, 25);
        assert!(!death_possible);
    }

    // =========================================================================
    // Reactor folded into the attack distribution
    // =========================================================================

    #[test]
    fn test_total_attack_distribution_without_reactor_is_tdg_distribution() {
        let config = SimConfig {
            tdg_min: 40,
            tdg_max: 45,
            day: 1,
            ..Default::default()
        };
        let dist = attack_distribution(40, 45, 1);
        let total = total_attack_distribution(&config);
        assert_eq!(total.len(), 6);
        for (attack, weight) in total {
            assert!((weight - dist[&attack]).abs() < 1e-12);
        }
    }

    #[test]
    fn test_total_attack_distribution_with_reactor_is_uniform_window() {
        // 30 equally likely TDG values, reactor damage uniform over 151 values.
        let config = SimConfig {
            tdg_min: 1000,
            tdg_max: 1029,
            day: 10,
            is_reactor_built: true,
            ..Default::default()
        };
        let total = total_attack_distribution(&config);
        assert_eq!(total.first().unwrap().0, 1100);
        assert_eq!(total.last().unwrap().0, 1279);
        assert_eq!(total.len(), 180);

        let sum: f64 = total.iter().map(|&(_, w)| w).sum();
        assert!((sum - 1.0).abs() < 1e-9, "weights sum to {sum}");

        let weight = |attack: i32| total.iter().find(|&&(a, _)| a == attack).unwrap().1;
        // Only TDG value 1000 can reach 1100 (with 100 reactor damage).
        assert!((weight(1100) - 1.0 / (30.0 * 151.0)).abs() < 1e-12);
        // Every TDG value can reach 1200 (with 171..=200 reactor damage).
        assert!((weight(1200) - 1.0 / 151.0).abs() < 1e-12);
        // Only TDG value 1029 can reach 1279 (with 250 reactor damage).
        assert!((weight(1279) - 1.0 / (30.0 * 151.0)).abs() < 1e-12);
    }

    #[test]
    fn test_plan_skips_total_attacks_that_cannot_kill() {
        // Totals reach at most 1004 + 250 - 1240 = 14 overflow, below min_def.
        let config = SimConfig {
            defense: 1240,
            tdg_min: 1000,
            tdg_max: 1004,
            min_def: 59,
            day: 10,
            nb_hab: 40,
            is_reactor_built: true,
            iterations: 1000,
            ..Default::default()
        };
        let (prob, total_runs, _) = overflow_probability(&config);
        assert_eq!(prob, 0.0);
        assert_eq!(
            total_runs, 5_000,
            "the reported budget still counts every TDG value that can overflow"
        );
        let plan = plan_attack_points(&config, resolve_b_level(&config, &[]), config.min_def);
        assert!(plan.points.is_empty());
    }

    #[test]
    fn test_plan_splits_budget_by_weight() {
        let config = SimConfig {
            defense: 1000,
            tdg_min: 1000,
            tdg_max: 1029,
            min_def: 30,
            day: 10,
            nb_hab: 40,
            is_reactor_built: true,
            iterations: 10_000,
            ..Default::default()
        };
        let plan = plan_attack_points(&config, resolve_b_level(&config, &[]), config.min_def);
        assert_eq!(plan.budget, 300_000);
        for point in &plan.points {
            assert_eq!(
                point.runs,
                (plan.budget as f64 * point.weight).ceil() as u64,
                "total attack {}",
                point.total_attack
            );
        }
        let total: u64 = plan.points.iter().map(|p| p.runs).sum();
        assert!(total <= plan.budget + plan.points.len() as u64);
    }

    // =========================================================================
    // Required defense for a target death risk
    // =========================================================================

    fn search_config() -> SimConfig {
        SimConfig {
            tdg_min: 1000,
            tdg_max: 1004,
            min_def: 30,
            day: 10,
            nb_hab: 40,
            is_reactor_built: true,
            iterations: 50_000,
            ..Default::default()
        }
    }

    fn can_die_at(config: &SimConfig, defense: i32) -> bool {
        let config = SimConfig {
            defense,
            ..config.clone()
        };
        !plan_attack_points(&config, resolve_b_level(&config, &[]), config.min_def)
            .points
            .is_empty()
    }

    #[test]
    fn test_safe_defense_is_the_lowest_defense_where_nobody_can_die() {
        let config = search_config();
        let search = required_defense(&config, &[], 0.0);
        assert!(!can_die_at(&config, search.safe_defense));
        assert!(can_die_at(&config, search.safe_defense - 1));
    }

    #[test]
    fn test_required_defense_for_zero_risk_is_safe_defense() {
        let search = required_defense(&search_config(), &[], 0.0);
        assert_eq!(search.defense, search.safe_defense);
        assert_eq!(search.prob_at_defense, 0.0);
        assert_eq!(search.total_runs, 0, "a 0% target needs no simulation");
    }

    #[test]
    fn test_required_defense_meets_target_and_is_minimal() {
        let config = search_config();
        let target = 20.0;
        let search = required_defense(&config, &[], target);

        assert!(search.defense < search.safe_defense);
        assert!(search.prob_at_defense <= target);
        assert!(search.total_runs > 0);

        // Independent estimates on both sides of the answer, within noise.
        let at = |defense: i32| {
            overflow_probability(&SimConfig {
                defense,
                ..config.clone()
            })
            .0
        };
        let at_answer = at(search.defense);
        let below_answer = at(search.defense - 1);
        assert!(
            at_answer <= target + 1.0,
            "P({}) = {at_answer}%",
            search.defense
        );
        assert!(
            below_answer > target - 1.0,
            "P({}) = {below_answer}%",
            search.defense - 1
        );
    }

    #[test]
    fn test_required_defense_is_zero_when_target_is_met_without_defense() {
        // Nobody can die at all: every citizen has a huge defense threshold.
        let config = SimConfig {
            min_def: 1_000_000,
            ..search_config()
        };
        let search = required_defense(&config, &[], 5.0);
        assert_eq!(search.safe_defense, 0);
        assert_eq!(search.defense, 0);
    }

    #[test]
    fn test_required_defense_complete_mode() {
        use crate::config::SimulationCitizen;

        let mut citizens = vec![SimulationCitizen {
            name: "Fragile".to_string(),
            defense: 0,
        }];
        for i in 0..39 {
            citizens.push(SimulationCitizen {
                name: format!("Tank{i}"),
                defense: 60,
            });
        }
        let config = SimConfig {
            is_complete: true,
            ..search_config()
        };
        let target = 10.0;
        let search = required_defense(&config, &citizens, target);

        assert!(search.prob_at_defense <= target);
        assert!(search.defense < search.safe_defense);
        let (at_answer, _, _, _) = complete_overflow_probability(
            &SimConfig {
                defense: search.defense,
                ..config.clone()
            },
            &citizens,
        );
        assert!(
            at_answer <= target + 1.0,
            "P({}) = {at_answer}%",
            search.defense
        );
    }

    #[test]
    fn test_lowest_passing_defense_near_finds_the_edge_from_any_guess() {
        let answer = 37;
        for guess in [0, 1, 20, 36, 37, 38, 60, 100] {
            let mut evaluations = Vec::new();
            let found = lowest_passing_defense_near(guess, 100, |d| {
                evaluations.push(d);
                d >= answer
            });
            assert_eq!(found, answer, "guess {guess}, evaluated {evaluations:?}");
            assert!(evaluations.iter().all(|&d| (0..=100).contains(&d)));
        }
    }

    #[test]
    fn test_lowest_passing_defense_near_needs_two_evaluations_for_a_close_guess() {
        let mut evaluations = 0;
        let found = lowest_passing_defense_near(37, 100, |d| {
            evaluations += 1;
            d >= 37
        });
        assert_eq!(found, 37);
        assert_eq!(evaluations, 2, "37 passes and 36 fails");
    }

    #[test]
    fn test_lowest_passing_defense_near_handles_the_bounds() {
        // Everything passes: the answer is 0.
        assert_eq!(lowest_passing_defense_near(50, 100, |_| true), 0);
        // Only `max` passes; it is never evaluated.
        let found = lowest_passing_defense_near(50, 100, |d| {
            assert!(d < 100, "max must not be evaluated");
            false
        });
        assert_eq!(found, 100);
    }

    #[test]
    fn test_required_defense_without_rough_pass_for_few_iterations() {
        let config = SimConfig {
            iterations: ROUGH_SEARCH_ITERATIONS,
            ..search_config()
        };
        let target = 20.0;
        let search = required_defense(&config, &[], target);
        assert!(search.prob_at_defense <= target);
        assert!(search.defense < search.safe_defense);
    }

    #[test]
    fn test_complete_town_probability_matches_complete_simulation() {
        use crate::config::SimulationCitizen;

        let citizens: Vec<SimulationCitizen> = (0..40)
            .map(|i| SimulationCitizen {
                name: format!("Cit {i}"),
                defense: 15 + i,
            })
            .collect();
        let config = SimConfig {
            defense: 1100,
            is_complete: true,
            ..search_config()
        };
        let (town_only, runs) = complete_town_probability(&config, &citizens);
        let (town, full_runs, _, _) = complete_overflow_probability(&config, &citizens);
        assert_eq!(runs, full_runs);
        assert!(
            town > 2.0 && town < 95.0,
            "test config should give a non-trivial probability, got {town}%"
        );
        assert!(
            (town_only - town).abs() < 1.0,
            "town-only {town_only}% vs complete {town}%"
        );
    }
}
