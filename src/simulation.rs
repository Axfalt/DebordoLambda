use std::cmp;
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
    
    pub fn simulate_attack(&mut self, day: i32, attacking: i32, drapo: i32, nb_hab: i32) -> Vec<i32> {
        // Calcul des cibles et suppression de l'influence des drapeaux
        let targets = cmp::min(10 + 2 * ((day - 10).max(0) / 2), nb_hab);

        if targets <= 0 {
            return Vec::new();
        }

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
    nb_hab: i32,
) -> f64 {
    if iterations == 0 || nb_hab <= 0 {
        return 0.0;
    }

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
        let allocated = simulator.simulate_attack(day, real_attacking, nb_drapo, nb_hab);
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

pub fn overflow_probability(
    defense: f64,
    tdg_interval: (i32, i32),
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: u32,
    is_reactor_built: bool,
    nb_hab: i32
) -> (f64, u64) {
    let prob_dist = attack_distribution(tdg_interval.0, tdg_interval.1, day);
    let mut overflow_prob = 0.0;
    let mut total_runs: u64 = 0;

    for (&attack, &base_prob) in &prob_dist {
        let overflow = attack as f64 - defense;
        let max_reactor_damage = if is_reactor_built { 125.0 } else { 0.0 };
        if overflow + max_reactor_damage > 0.0 {
            let success_prob = debordo_sequential(
                day,
                overflow as i32,
                min_def,
                nb_drapo,
                iterations,
                is_reactor_built,
                nb_hab,
            );
            overflow_prob += base_prob * success_prob;
            total_runs += iterations as u64;
        }
    }

    (overflow_prob * 100.0, total_runs)
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

    #[test]
    fn test_attack_distribution_midpoint_below_range_stays_normalized() {
        // Day 1 midpoint is around 42, so [100,130] is entirely above midpoint.
        // Reroll then applies to all values and distribution must remain normalized.
        let dist = attack_distribution(100, 130, 1);
        let sum: f64 = dist.values().sum();
        assert!((sum - 1.0).abs() < 0.0001, "probabilities sum to {}, expected 1.0", sum);
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
        let result = sim.simulate_attack(10, 1000, 0, 40);
        assert_eq!(result.len(), 10);

        // Day 12: 12 targets
        let result = sim.simulate_attack(12, 1000, 0, 40);
        assert_eq!(result.len(), 12);
    }

    #[test]
    fn test_simulate_attack_zero_attacking_returns_zeros() {
        // With 0 overflow zombies and no flags, every cell gets 0.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(1, 0, 0, 40);
        assert!(
            result.iter().all(|&x| x == 0),
            "with 0 attacking and no flags, all cells should be 0"
        );
    }

    #[test]
    fn test_simulate_attack_all_allocations_non_negative() {
        let mut sim = AttackSimulator::new();
        for _ in 0..20 {
            let result = sim.simulate_attack(5, 500, 0, 7);
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
            let result = sim.simulate_attack(1, attacking, 0, 40);
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
        let prob = debordo_sequential(1, 0, 1, 0, 100, false, 40);
        assert_eq!(prob, 0.0, "with 0 overflow zombies, death probability must be 0");
    }

    #[test]
    fn test_debordo_overwhelming_attack_near_full_probability() {
        // 10 000 zombies among 10 citizens, min threshold of 1 → always at least one death.
        let prob = debordo_sequential(1, 10_000, 1, 0, 500, false, 40);
        assert!(prob > 0.99, "expected probability > 0.99, got {}", prob);
    }

    #[test]
    fn test_debordo_reactor_increases_attack_power() {
        // With reactor built: real_attacking = attacking + 100..=250.
        // A non-zero base attack with reactor should yield higher (or equal) probability
        // than without reactor for the same inputs when the threshold is moderate.
        let prob_no_reactor = debordo_sequential(1, 50, 30, 0, 500, false, 40);
        let prob_reactor = debordo_sequential(1, 50, 30, 0, 500, true, 40);
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
        let (prob, _) = overflow_probability(150.0, (50, 60), 10, 0, 1, 100, false, 40);
        assert!(prob >= 0.0 && prob <= 100.0);
    }

    #[test]
    fn test_calculate_defense_probs_impenetrable_defense_is_zero() {
        // Defense >> max possible attack → no overflow → 0% probability.
        let (prob, total_runs) = overflow_probability(100_000.0, (50, 100), 10, 0, 1, 100, false, 40);
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
        let result = sim.simulate_attack(20, 1000, 0, 5);
        assert_eq!(result.len(), 5, "nb_hab=5 should limit targets to 5");
    }

    #[test]
    fn test_simulate_attack_nb_hab_does_not_increase_targets() {
        // Day 10 has 10 targets. nb_hab=100 should not increase beyond 10.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(10, 1000, 0, 100);
        assert_eq!(result.len(), 10, "nb_hab should not increase targets beyond day formula");
    }

    #[test]
    fn test_simulate_attack_nb_hab_equals_day_targets() {
        // Day 10 = 10 targets, nb_hab=10 should give exactly 10.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(10, 1000, 0, 10);
        assert_eq!(result.len(), 10);
    }

    #[test]
    fn test_simulate_attack_nb_hab_one_person() {
        // Edge case: only 1 person in town receives all zombies.
        let mut sim = AttackSimulator::new();
        let result = sim.simulate_attack(10, 100, 0, 1);
        assert_eq!(result.len(), 1, "nb_hab=1 should have exactly 1 target");
        assert!(result[0] >= 100, "single target should receive all zombies");
    }

    #[test]
    fn test_debordo_nb_hab_affects_distribution() {
        // Fewer people means zombies are more concentrated → higher death probability.
        // With 1000 zombies among 40 people vs 5 people, 5 people should have higher prob.
        let prob_40_hab = debordo_sequential(10, 1000, 50, 0, 500, false, 40);
        let prob_5_hab = debordo_sequential(10, 1000, 50, 0, 500, false, 5);
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
        let (prob, _) = overflow_probability(50.0, (60, 70), 10, 0, 5, 100, false, 3);
        assert!(prob > 0.0, "with small nb_hab and overflow, death probability should be > 0");
    }

    #[test]
    fn test_overflow_probability_with_nb_hab_12() {
        // Regression test for the "cannot sample empty range" panic with nb_hab=12.
        // This should not panic regardless of the input parameters.
        let (prob, _) = overflow_probability(100.0, (150, 200), 10, 0, 10, 100, false, 12);
        assert!(prob >= 0.0 && prob <= 100.0, "probability should be in valid range");
    }

    #[test]
    fn test_debordo_with_nb_hab_zero_returns_zero() {
        // Edge case: nb_hab=0 should return 0.0 without panicking.
        let prob = debordo_sequential(10, 100, 10, 0, 100, false, 0);
        assert_eq!(prob, 0.0, "nb_hab=0 should return 0.0");
    }

    #[test]
    fn test_debordo_with_iterations_zero_returns_zero() {
        // Edge case: iterations=0 should return 0.0 without panicking.
        let prob = debordo_sequential(10, 100, 10, 0, 0, false, 40);
        assert_eq!(prob, 0.0, "iterations=0 should return 0.0");
    }
}

