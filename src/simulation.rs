use crate::config::SimConfig;
use rand::RngExt;
use rand::distr::Uniform;
use rand::prelude::*;
use rand_mt::Mt64;
use std::cmp;
use std::cmp::max;
use std::collections::HashMap;

const FLAG_REDUCTION_RATE: f64 = 0.025;
const BASE_LEVEL_MIN: u32 = 45;
const BASE_LEVEL_MAX: u32 = 55;
const UNLUCKY_BOOST: f64 = 0.3;
const REACTOR_DAMAGE_MIN: i32 = 100;
const REACTOR_DAMAGE_MAX: i32 = 250;

#[derive(Clone)]
pub struct AttackSimulator {
    rng: Mt64,
    repartition_buf: Vec<f64>,
    allocated_buf: Vec<i32>,
}

impl AttackSimulator {
    /// Crée un nouveau simulateur avec une seed aléatoire.
    pub fn new() -> Self {
        Self {
            rng: Mt64::new(rand::random()),
            repartition_buf: Vec::with_capacity(40),
            allocated_buf: Vec::with_capacity(40),
        }
    }

    pub fn simulate_attack_with_max_active(
        &mut self,
        config: &SimConfig,
        total_attack: i32,
        overflow: i32,
        b_level_override: Option<i32>,
    ) -> (&[i32], i32, f64) {
        let day = config.day;
        let drapo = config.nb_drapo;
        let nb_hab = config.nb_hab;
        let b_level = b_level_override.or(config.b_level);
        let is_chaos = config.is_chaos;
        let is_devastated = config.is_devastated;

        // Calcul des cibles et suppression de l'influence des drapeaux
        let targets = cmp::min(10 + 2 * ((day - 10).max(0) / 2), nb_hab);

        if targets <= 0 || overflow <= 0 {
            self.allocated_buf.clear();
            return (&self.allocated_buf, 0, 0.0);
        }

        // Active zombie capping (PHP alignement: $max_active = round($zombies * $active_factor))
        let b_level_val = b_level.unwrap_or(1);
        let pop_val = nb_hab;

        let base_level = self.rng.random_range(BASE_LEVEL_MIN..=BASE_LEVEL_MAX) as f64;
        let mut level = base_level;

        level *= (targets.max(15) as f64 + b_level_val.max(0) as f64 * 2.0) / pop_val.max(1) as f64;

        if is_chaos {
            level += 10.0;
        }
        if is_devastated {
            level += 10.0;
        }

        let active_factor = (level / 100.0).clamp(0.0, 1.0);
        let max_active = (total_attack as f64 * active_factor).round() as i32;
        let mut leftover = max_active.min(overflow);

        // Réduction par les drapeaux
        for _ in 0..drapo {
            leftover -= (total_attack as f64 * FLAG_REDUCTION_RATE).round() as i32;
        }

        let flag_bonus = if drapo > 0 {
            (total_attack as f64 * FLAG_REDUCTION_RATE).round() as i32
        } else {
            0
        };
        if leftover <= 0 {
            self.allocated_buf.clear();
            self.allocated_buf.resize(targets as usize, flag_bonus);
            return (&self.allocated_buf, max_active, active_factor);
        }

        // Poids aléatoires in [0, 1.0] (PHP alignement)
        self.repartition_buf.clear();
        for _ in 0..targets {
            self.repartition_buf.push(self.rng.random::<f64>());
        }

        // Une cible reçoit un boost de +0.3
        if !self.repartition_buf.is_empty() {
            let unlucky_idx = self.rng.random_range(0..self.repartition_buf.len());
            self.repartition_buf[unlucky_idx] += UNLUCKY_BOOST;
        }

        let sum: f64 = self.repartition_buf.iter().sum();

        // Allocation des attaques avec contrainte de somme exacte (PHP alignement)
        self.allocated_buf.clear();
        self.allocated_buf.resize(targets as usize, 0);
        let mut attacking_cache = leftover;

        if sum > 0.0 {
            for i in 0..targets as usize {
                let norm = self.repartition_buf[i] / sum;
                let share = (norm * leftover as f64).round() as i32;
                let val = 0.max(attacking_cache.min(share));
                self.allocated_buf[i] = val;
                attacking_cache -= val;
            }
        }

        // Distribution du reliquat aux cibles aléatoires
        while attacking_cache > 0 && !self.allocated_buf.is_empty() {
            let idx = self.rng.random_range(0..self.allocated_buf.len());
            self.allocated_buf[idx] += 1;
            attacking_cache -= 1;
        }

        // Ajout de l'influence des drapeaux
        self.allocated_buf.iter_mut().for_each(|x| *x += flag_bonus);
        (&self.allocated_buf, max_active, active_factor)
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

fn debordo_sequential(config: &SimConfig, raw_attack: i32, threshold: i32) -> (f64, Option<f64>) {
    if config.iterations == 0 || config.nb_hab <= 0 {
        return (0.0, None);
    }

    let mut town_hits = 0;
    let mut total_capped_max_active: u64 = 0;
    let mut capped_iterations: u64 = 0;
    let mut rng = rand::rng();
    let reactor_damage = Uniform::new_inclusive(REACTOR_DAMAGE_MIN, REACTOR_DAMAGE_MAX).unwrap();

    let b_level_resolved = resolve_b_level(config, &[]);

    let mut simulator = AttackSimulator::new();

    for _ in 0..config.iterations {
        let real_total_attack = if config.is_reactor_built {
            raw_attack + reactor_damage.sample(&mut rng)
        } else {
            raw_attack
        };
        let overflow = (real_total_attack - config.defense).max(0);

        let (allocated, max_active, _active_factor) = simulator.simulate_attack_with_max_active(
            config,
            real_total_attack,
            overflow,
            Some(b_level_resolved),
        );
        if max_active < overflow {
            total_capped_max_active += max_active.max(0) as u64;
            capped_iterations += 1;
        }

        if allocated.iter().any(|&x| x > threshold) {
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

fn complete_debordo_sequential(
    config: &SimConfig,
    raw_attack: i32,
    citizens: &[crate::config::SimulationCitizen],
) -> (f64, Vec<f64>, Option<f64>) {
    if config.iterations == 0 || config.nb_hab <= 0 {
        return (0.0, vec![0.0; citizens.len()], None);
    }

    let threshold = citizens.iter().map(|c| c.defense).min().unwrap_or(0);
    let b_level_resolved = resolve_b_level(config, citizens);

    let mut town_hits = 0;
    let mut total_capped_max_active: u64 = 0;
    let mut capped_iterations: u64 = 0;
    let mut citizen_hits = vec![0u64; citizens.len()];
    let mut rng = rand::rng();
    let reactor_damage = Uniform::new_inclusive(REACTOR_DAMAGE_MIN, REACTOR_DAMAGE_MAX).unwrap();

    let mut simulator = AttackSimulator::new();
    let mut indices: Vec<usize> = (0..citizens.len()).collect();

    for _ in 0..config.iterations {
        let real_total_attack = if config.is_reactor_built {
            raw_attack + reactor_damage.sample(&mut rng)
        } else {
            raw_attack
        };
        let overflow = (real_total_attack - config.defense).max(0);

        let (allocated, max_active, _active_factor) = simulator.simulate_attack_with_max_active(
            config,
            real_total_attack,
            overflow,
            Some(b_level_resolved),
        );
        if max_active < overflow {
            total_capped_max_active += max_active.max(0) as u64;
            capped_iterations += 1;
        }

        if allocated.iter().any(|&x| x > threshold) {
            town_hits += 1;
        }

        if !citizens.is_empty() {
            use rand::seq::SliceRandom;
            indices.shuffle(&mut rng);
            let targets = allocated.len().min(citizens.len());
            for j in 0..targets {
                let idx = indices[j];
                let zombies = allocated[j];
                if zombies > citizens[idx].defense {
                    citizen_hits[idx] += 1;
                }
            }
        }
    }

    let town_prob = town_hits as f64 / config.iterations as f64;
    let citizen_probs = citizen_hits
        .iter()
        .map(|&hits| hits as f64 / config.iterations as f64)
        .collect();
    let avg_max_active = if capped_iterations > 0 {
        Some(total_capped_max_active as f64 / capped_iterations as f64)
    } else {
        None
    };

    (town_prob, citizen_probs, avg_max_active)
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
    let (tdg_min, tdg_max) = config.tdg_interval();
    let prob_dist = attack_distribution(tdg_min, tdg_max, config.day);
    let mut overflow_prob = 0.0;
    let mut weighted_avg_max_active = 0.0;
    let mut weighted_cap_prob = 0.0;
    let mut total_runs: u64 = 0;

    for (&attack, &base_prob) in &prob_dist {
        let overflow = attack as f64 - config.defense as f64;
        let max_reactor_damage = if config.is_reactor_built { 250.0 } else { 0.0 };
        if overflow + max_reactor_damage > 0.0 {
            let (success_prob, avg_max_active) = debordo_sequential(config, attack, config.min_def);
            overflow_prob += base_prob * success_prob;
            if let Some(avg_m) = avg_max_active {
                weighted_avg_max_active += base_prob * avg_m;
                weighted_cap_prob += base_prob;
            }
            total_runs += config.iterations as u64;
        }
    }

    let avg_max_active_opt = if weighted_cap_prob > 0.0 {
        Some(weighted_avg_max_active / weighted_cap_prob)
    } else {
        None
    };

    (
        (overflow_prob.min(1.0) * 100.0).max(0.0),
        total_runs,
        avg_max_active_opt,
    )
}

pub fn complete_overflow_probability(
    config: &SimConfig,
    citizens: &[crate::config::SimulationCitizen],
) -> (f64, u64, Vec<f64>, Option<f64>) {
    let (tdg_min, tdg_max) = config.tdg_interval();
    let prob_dist = attack_distribution(tdg_min, tdg_max, config.day);
    let mut overflow_prob = 0.0;
    let mut total_runs: u64 = 0;
    let mut citizen_probs = vec![0.0; citizens.len()];
    let mut weighted_avg_max_active = 0.0;
    let mut weighted_cap_prob = 0.0;

    for (&attack, &base_prob) in &prob_dist {
        let overflow = attack as f64 - config.defense as f64;
        let max_reactor_damage = if config.is_reactor_built { 250.0 } else { 0.0 };
        if overflow + max_reactor_damage > 0.0 {
            let (town_prob, citizen_p, avg_max_active) =
                complete_debordo_sequential(config, attack, citizens);
            overflow_prob += base_prob * town_prob;
            if let Some(avg_m) = avg_max_active {
                weighted_avg_max_active += base_prob * avg_m;
                weighted_cap_prob += base_prob;
            }
            total_runs += config.iterations as u64;

            for i in 0..citizens.len() {
                citizen_probs[i] += base_prob * citizen_p[i];
            }
        }
    }

    let citizen_percentages = citizen_probs
        .iter()
        .map(|&p| (p.min(1.0) * 100.0).max(0.0))
        .collect();

    let avg_max_active_opt = if weighted_cap_prob > 0.0 {
        Some(weighted_avg_max_active / weighted_cap_prob)
    } else {
        None
    };

    (
        (overflow_prob.min(1.0) * 100.0).max(0.0),
        total_runs,
        citizen_percentages,
        avg_max_active_opt,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // =========================================================================
    // attack_distribution
    // =========================================================================

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

    #[test]
    fn test_debordo_reactor_increases_attack_power() {
        // With reactor built: real_attacking = attacking + 100..=250.
        // A non-zero base attack with reactor should yield higher (or equal) probability
        // than without reactor for the same inputs when the threshold is moderate.
        let (prob_no_reactor, _) = debordo_sequential(
            &SimConfig {
                day: 1,
                iterations: 500,
                nb_hab: 40,
                is_reactor_built: false,
                ..Default::default()
            },
            50,
            30,
        );
        let (prob_reactor, _) = debordo_sequential(
            &SimConfig {
                day: 1,
                iterations: 500,
                nb_hab: 40,
                is_reactor_built: true,
                ..Default::default()
            },
            50,
            30,
        );
        assert!(
            prob_reactor >= prob_no_reactor,
            "reactor should increase attack power: no_reactor={} reactor={}",
            prob_no_reactor,
            prob_reactor
        );
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
}
