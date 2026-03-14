use std::cmp::max;
use rand::prelude::*;
use rand::distributions::Uniform;
use rand_mt::Mt64;
use std::collections::HashMap;

#[derive(Clone)]
pub struct AttackSimulator {
    rng: Mt64,
}

impl AttackSimulator {
    /// Crée un nouveau simulateur avec une seed aléatoire.
    pub fn new() -> Self {
        Self {
            rng: Mt64::new(rand::random()),
        }
    }
    
    pub fn simulate_attack(&mut self, day: i32, attacking: i32, drapo: i32) -> Vec<i32> {
        // Calcul des cibles et suppression de l'influence des drapeaux
        let targets = 10 + 2 * ((day - 10).max(0) / 2);
        let mut leftover = attacking;

        // Réduction par les drapeaux
        for _ in 0..drapo {
            leftover -= (attacking as f64 * 0.025).round() as i32;
        }

        if leftover <= 0 {
            let flag_bonus = (attacking as f64 * 0.025).round() as i32;
            return vec![flag_bonus; targets as usize];
        }

        // Poids aléatoires
        let mut repartition: Vec<f64> = (0..targets).map(|_| self.rng.r#gen::<f64>()).collect();

        // Une cible reçoit un boost de +0.3
        let unlucky_index = self.rng.gen_range(0..targets as usize);
        repartition[unlucky_index] += 0.3;

        // Normalisation
        let sum_weights: f64 = repartition.iter().sum();
        let normalized: Vec<f64> = repartition.iter().map(|x| x / sum_weights).collect();

        // Allocation des attaques (avec arrondi)
        let mut allocated: Vec<i32> = normalized
            .iter()
            .map(|p| 0.max((p * leftover as f64).round() as i32).min(leftover))
            .collect();

        // Allocation des attaques restantes
        let mut attacking_cache = leftover - allocated.iter().sum::<i32>();
        while attacking_cache > 0 {
            let idx = self.rng.gen_range(0..targets as usize);
            allocated[idx] += 1;
            attacking_cache -= 1;
        }

        // Ajout de l'influence des drapeaux
        let flag_bonus = (attacking as f64 * 0.025).round() as i32;
        allocated.iter_mut().for_each(|x| *x += flag_bonus);
        allocated
    }
}

impl Default for AttackSimulator {
    fn default() -> Self {
        Self::new()
    }
}
fn debordo_sequential(
    day: i32,
    attacking: i32,
    threshold: i32,
    nb_drapo: i32,
    iterations: u32,
    is_reactor_built: bool,
) -> f64 {
    let mut hits = 0;
    let mut rng = rand::thread_rng();
    let reactor_damage = Uniform::from(100..=250);


    for _ in 0..iterations {
        let real_attacking = if is_reactor_built {
            attacking + reactor_damage.sample(&mut rng)
        } else {
            attacking
        };
        let mut simulator = AttackSimulator::new();
        let allocated = simulator.simulate_attack(day, real_attacking, nb_drapo);
        if allocated.iter().any(|&x| x > threshold) {
            hits += 1;
        }
    }

    hits as f64 / iterations as f64
}

fn attack_distribution(tdg_min: i32, tdg_max: i32, day: i32) -> HashMap<i32, f64> {
    if tdg_min > tdg_max {
        return HashMap::new();
    }
    let ratio = if day <= 3 { 0.75 } else { 1.1 } as f64;
    let lo = (ratio * (max(1, day - 1) as f64 * 0.75 + 2.5).powi(3)).round() as i32;
    let hi = (ratio * (day as f64 * 0.75 + 3.5).powi(3)).round() as i32;
    let mid = lo as f64 + 0.5 * (hi - lo) as f64;
    let mid_floor = mid.floor() as i32;

    let total_count = (tdg_max - tdg_min + 1) as f64;
    let p = 1.0 / total_count;

    let n_high = if mid_floor < tdg_max {
        (tdg_max - mid_floor) as f64
    } else {
        0.0
    };

    let reroll_prob = n_high * p;

    let mut prob = HashMap::new();
    for i in tdg_min..=tdg_max {
        if i <= mid_floor {
            prob.insert(i, p + reroll_prob * p);
        } else {
            prob.insert(i, reroll_prob * p);
        }
    }

    prob
}

fn overflow_probability(
    defense: f64,
    tdg_interval: (i32, i32),
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: u32,
    is_reactor_built: bool,
) -> f64 {
    let prob_dist = attack_distribution(tdg_interval.0, tdg_interval.1, day);
    let mut overflow_prob = 0.0;

    for (&attack, &base_prob) in &prob_dist {
        let overflow = attack as f64 - defense;
        if overflow > 0.0 {
            let overflow_int = overflow as i32;
            let success_prob = debordo_sequential(
                day,
                overflow_int,
                min_def,
                nb_drapo,
                iterations,
                is_reactor_built,
            );
            overflow_prob += base_prob * success_prob;
        }
    }

    overflow_prob * 100.0
}


pub fn calculate_defense_probabilities(
    defense: i32,
    tdg_interval: (i32, i32),
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: u32,
    is_reactor_built: bool,
) -> f64 {
    overflow_probability(
        defense as f64,
        tdg_interval,
        min_def,
        nb_drapo,
        day,
        iterations,
        is_reactor_built,
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
        assert!((sum - 1.0).abs() < 0.0001, "probabilities sum to {}, expected 1.0", sum);
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

    // =========================================================================
    // AttackSimulator::simulate_attack
    // =========================================================================

    #[test]
    fn test_simulator_creates_correct_number_of_targets() {
        let mut sim = AttackSimulator::new();

        // Day 10: 10 targets
        let result = sim.simulate_attack(10, 1000, 0);
        assert_eq!(result.len(), 10);

        // Day 12: 12 targets
        let result = sim.simulate_attack(12, 1000, 0);
        assert_eq!(result.len(), 12);
    }

    #[test]
    fn test_simulate_attack_zero_attacking_returns_zeros() {
        // With 0 overflow zombies and no flags, every cell gets 0.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(1, 0, 0);
        assert!(
            result.iter().all(|&x| x == 0),
            "with 0 attacking and no flags, all cells should be 0"
        );
    }

    #[test]
    fn test_simulate_attack_all_allocations_non_negative() {
        let mut sim = AttackSimulator::new();
        for _ in 0..20 {
            let result = sim.simulate_attack(5, 500, 0);
            assert!(
                result.iter().all(|&x| x >= 0),
                "zombie allocations must never be negative"
            );
        }
    }

    #[test]
    fn test_simulate_attack_sum_at_least_attacking_without_flags() {
        // Without flags, rounding can only add zombies, never remove them.
        // sum(allocated) >= attacking is the intended behavior (see copilot-instructions.md).
        let mut sim = AttackSimulator::new();
        for _ in 0..10 {
            let attacking = 100;
            let result = sim.simulate_attack(1, attacking, 0);
            let sum: i32 = result.iter().sum();
            assert!(sum >= attacking, "sum {} should be >= attacking {}", sum, attacking);
        }
    }

    // =========================================================================
    // debordo_sequential
    // =========================================================================

    #[test]
    fn test_debordo_zero_attacking_gives_zero_probability() {
        // 0 overflow zombies → no cell can exceed any threshold → 0% death.
        let prob = debordo_sequential(1, 0, 1, 0, 100, false);
        assert_eq!(prob, 0.0, "with 0 overflow zombies, death probability must be 0");
    }

    #[test]
    fn test_debordo_overwhelming_attack_near_full_probability() {
        // 10 000 zombies among 10 citizens, min threshold of 1 → always at least one death.
        let prob = debordo_sequential(1, 10_000, 1, 0, 500, false);
        assert!(prob > 0.99, "expected probability > 0.99, got {}", prob);
    }

    #[test]
    fn test_debordo_reactor_increases_attack_power() {
        // With reactor built: real_attacking = attacking + 100..=250.
        // A non-zero base attack with reactor should yield higher (or equal) probability
        // than without reactor for the same inputs when the threshold is moderate.
        let prob_no_reactor = debordo_sequential(1, 50, 30, 0, 500, false);
        let prob_reactor = debordo_sequential(1, 50, 30, 0, 500, true);
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
        let prob = calculate_defense_probabilities(150, (50, 60), 10, 0, 1, 100, false);
        assert!(prob >= 0.0 && prob <= 100.0);
    }

    #[test]
    fn test_calculate_defense_probs_impenetrable_defense_is_zero() {
        // Defense >> max possible attack → no overflow → 0% probability.
        let prob = calculate_defense_probabilities(100_000, (50, 100), 10, 0, 1, 100, false);
        assert_eq!(prob, 0.0, "impenetrable defense should yield 0%");
    }
}

