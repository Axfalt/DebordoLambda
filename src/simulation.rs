use rand::prelude::*;
use rand::distributions::Uniform;
use rand_mt::Mt64;
use rayon::prelude::*;
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
            attacking
        } else {
            attacking - reactor_damage.sample(&mut rng)
        };
        let mut simulator = AttackSimulator::new();
        let allocated = simulator.simulate_attack(day, real_attacking, nb_drapo);
        if allocated.iter().any(|&x| x > threshold) {
            hits += 1;
        }
    }

    hits as f64 / iterations as f64
}

fn attack_distribution(tdg_min: i32, tdg_max: i32) -> HashMap<i32, f64> {
    if tdg_min > tdg_max {
        return HashMap::new();
    }

    let total_count = tdg_max - tdg_min + 1;
    if total_count == 0 {
        return HashMap::new();
    }

    let mut prob = HashMap::new();
    for i in tdg_min..=tdg_max {
        prob.insert(i, 1.0 / total_count as f64);
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
    let prob_dist = attack_distribution(tdg_interval.0, tdg_interval.1);
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
    defense_range: (i32, i32),
    tdg_interval: (i32, i32),
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: u32,
    points: u32,
    is_reactor_built: bool,
) -> Vec<(f64, f64)> {
    let step = (defense_range.1 as f64 - defense_range.0 as f64) / (points - 1) as f64;

    (0..points)
        .into_par_iter()
        .map(|i| {
            let defense = defense_range.0 as f64 + i as f64 * step;
            let prob = overflow_probability(
                defense,
                tdg_interval,
                min_def,
                nb_drapo,
                day,
                iterations,
                is_reactor_built,
            );
            (defense, prob)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_attack_distribution() {
        let dist = attack_distribution(100, 102);
        assert_eq!(dist.len(), 3);
        assert!((dist[&100] - 1.0 / 3.0).abs() < 0.0001);
    }

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
}

