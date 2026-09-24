use rand::RngExt;
use rand::seq::SliceRandom;
use rand_mt::Mt64;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::cmp::min;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Statistics {
    pub mean: f64,
    pub median: f64,
    pub min: i32,
    pub max: i32,
    pub q1: f64,
    pub q3: f64,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimBuilding {
    pub name: String,
    pub life: i32,
    pub max_life: i32,
    pub breakable: bool,
    pub temporary: bool,
}

pub fn default_buildings() -> Vec<SimBuilding> {
    vec![
        ("Aqua-tourelles", 50),
        ("Armurerie", 40),
        ("Arroseurs auto", 85),
        ("Atelier", 25),
        ("Barbelés", 10),
        ("Bastion", 25),
        ("Boucherie", 40),
        ("Cage à viande", 40),
        ("Carte améliorée", 25),
        ("Catapulte primitive", 40),
        ("Chemin de ronde", 25),
        ("Cimetière cadenassé", 42),
        ("Cloison en bois", 30),
        ("Cloison métallique", 30),
        ("Crémato-Cue", 40),
        ("Dispositifs d’urgence", 40),
        ("Douves", 60),
        ("Éclairage public", 25),
        ("Espace nature des Ermites", 60),
        ("Établi des Techniciens", 60),
        ("Feux d’artifice", 90),
        ("Fixations de défenses", 50),
        ("Fondations", 30),
        ("Fosse à pieux", 35),
        ("Galeries des Fouineurs", 30),
        ("Grand fossé", 70),
        ("Grogro mur", 50),
        ("Habitations fortifiées", 50),
        ("Hamâme", 20),
        ("Infirmerie", 40),
        ("Labyrinthe", 200),
        ("Manufacture", 40),
        ("Muraille", 25),
        ("Muraille à pointes", 35),
        ("Muraille évolutive", 65),
        ("Neurotoxine", 60),
        ("Oubliettes", 25),
        ("Pamplemousses explosifs", 40),
        ("Phare", 30),
        ("Planificateur", 20),
        ("Plateforme d'observation", 30),
        ("Pluvio-canons", 40),
        ("Pommier de l’Outre-Monde", 30),
        ("Portail", 15),
        ("Potager", 60),
        ("Potence", 13),
        ("Pulvérisateur", 50),
        ("Purificateur", 75),
        ("Remparts avancés", 40),
        ("Repaire des Éclaireurs", 25),
        ("Robinetterie", 130),
        ("Salle de garde", 50),
        ("Sanctuaire", 20),
        ("Sanibroyeur", 55),
        ("Scrutateur", 30),
        ("Source Purificatrice d'Âme", 30),
        ("Système de Pièges des Apprivoiseurs", 40),
        ("Tour de guet", 15),
        ("Tour des Gardiens", 35),
        ("Vaporisateur", 40),
    ]
    .into_iter()
    .map(|(name, max_life)| SimBuilding {
        name: name.to_string(),
        life: max_life,
        max_life,
        breakable: true,
        temporary: false,
    })
    .collect()
}

pub fn damage_pool(attack: i32, total_defense: i32, watch_def: i32) -> i32 {
    let initial_overflow = (attack - total_defense).max(0);
    let blocked_by_watch = initial_overflow.min(watch_def.max(0));
    ((attack - blocked_by_watch) as f64 * 0.2).round() as i32
}

fn reparo_gen(
    damage_inflicted: i32,
    buildings: &[(i32, i32)],
    rng: &mut Mt64,
    scratch: &mut Vec<(i32, i32)>,
) -> i32 {
    let mut damage_counter = damage_inflicted;
    let mut total_damaged_hp = 0;

    scratch.clear();
    scratch.extend_from_slice(buildings);
    scratch.shuffle(rng);

    while damage_counter > 0 && !scratch.is_empty() {
        let (life, max_life) = scratch.pop().unwrap();

        // Le jeu exclut les bâtiments sans PV de prototype (`getHp() <= 0`) des cibles.
        if max_life <= 0 {
            continue;
        }

        let lower_damage_limit = (max_life as f64 * 0.1).ceil() as i32;
        let raw_damage = rng.random_range(lower_damage_limit..=max_life);

        let damages = min(life, raw_damage);
        let damages = min(damages, damage_counter);

        let real_damages = min(damages, (max_life as f64 * 0.7).ceil() as i32);
        total_damaged_hp += real_damages;
        damage_counter -= damages;
    }

    total_damaged_hp
}

fn reparostats(damage_inflicted: i32, iterations: u32, buildings: &[SimBuilding]) -> Vec<i32> {
    let mut rng = Mt64::new(rand::random());
    let life_pairs: Vec<(i32, i32)> = buildings.iter().map(|b| (b.life, b.max_life)).collect();
    let mut scratch = Vec::with_capacity(life_pairs.len());
    (0..iterations)
        .map(|_| reparo_gen(damage_inflicted, &life_pairs, &mut rng, &mut scratch))
        .collect()
}

fn compute_statistics(data: &[i32]) -> Statistics {
    let mut sorted_data = data.to_vec();
    sorted_data.sort();

    let len = sorted_data.len();
    if len == 0 {
        return Statistics {
            mean: 0.0,
            median: 0.0,
            min: 0,
            max: 0,
            q1: 0.0,
            q3: 0.0,
        };
    }

    let sum: i32 = sorted_data.iter().sum();
    let mean = sum as f64 / len as f64;

    let median = if len.is_multiple_of(2) {
        (sorted_data[len / 2 - 1] + sorted_data[len / 2]) as f64 / 2.0
    } else {
        sorted_data[len / 2] as f64
    };

    let min = *sorted_data.first().unwrap();
    let max = *sorted_data.last().unwrap();

    let q1 = if len.is_multiple_of(4) {
        (sorted_data[len / 4 - 1] + sorted_data[len / 4]) as f64 / 2.0
    } else {
        sorted_data[len / 4] as f64
    };

    let q3 = if (len * 3).is_multiple_of(4) {
        (sorted_data[(len * 3) / 4 - 1] + sorted_data[(len * 3) / 4]) as f64 / 2.0
    } else {
        sorted_data[(len * 3) / 4] as f64
    };

    Statistics {
        mean,
        median,
        min,
        max,
        q1,
        q3,
    }
}

pub fn calculate_reparation_probabilities(
    total_defense: i32,
    watch_def: i32,
    tdg_interval: (i32, i32),
    iterations: u32,
    buildings: &[SimBuilding],
) -> Vec<(i32, Statistics)> {
    let (tdg_min, tdg_max) = tdg_interval;
    if tdg_min > tdg_max {
        return Vec::new();
    }

    (tdg_min..=tdg_max)
        .into_par_iter()
        .map(|attack| {
            let damage_inflicted = damage_pool(attack, total_defense, watch_def);
            let stats = if damage_inflicted <= 0 {
                Statistics {
                    mean: 0.0,
                    median: 0.0,
                    min: 0,
                    max: 0,
                    q1: 0.0,
                    q3: 0.0,
                }
            } else {
                let results = reparostats(damage_inflicted, iterations, buildings);
                compute_statistics(&results)
            };
            (attack, stats)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn building(name: &str, life: i32, max_life: i32) -> SimBuilding {
        SimBuilding {
            name: name.to_string(),
            life,
            max_life,
            breakable: true,
            temporary: false,
        }
    }

    fn life_pairs(buildings: &[SimBuilding]) -> Vec<(i32, i32)> {
        buildings.iter().map(|b| (b.life, b.max_life)).collect()
    }

    // =========================================================================
    // damage_pool
    // =========================================================================

    #[test]
    fn test_damage_pool_subtracts_total_then_watch() {
        // attack=1000, total=300 => overflow=700; watch=200 blocks 200 of it =>
        // pool = round((1000 - 200) * 0.2) = 160.
        assert_eq!(damage_pool(1000, 300, 200), 160);
    }

    #[test]
    fn test_damage_pool_watch_capped_by_overflow_not_raw_attack() {
        // attack=1000, total=900 => overflow=100. watch=500 can only block
        // min(overflow, watch) = 100 (not the full 500). pool = round((1000-100)*0.2) = 180.
        assert_eq!(damage_pool(1000, 900, 500), 180);
    }

    #[test]
    fn test_damage_pool_zero_only_when_watch_kills_whole_attack() {
        // No total defense => overflow = attack; watch >= attack kills everything.
        assert_eq!(damage_pool(100, 0, 100), 0);
    }

    #[test]
    fn test_damage_pool_total_defense_does_not_itself_reduce_building_damage() {
        // Matches NightlyHandler.php: "Only 20% of the attack is inflicted to buildings /
        // zombies - amount of zombies killed by the watch". Walls that stop everything leave
        // the watch nothing to kill, so buildings still take 20% of the full attack.
        assert_eq!(damage_pool(100, 200, 0), 20);
        assert_eq!(damage_pool(100, 200, 500), 20);
    }

    #[test]
    fn test_damage_pool_stronger_walls_can_mean_more_building_damage() {
        // Counterintuitive but faithful to the game: stronger total defense shrinks the
        // overflow, which caps how much the watch can be credited for.
        let weak_walls = damage_pool(1000, 0, 500); // overflow 1000, watch kills 500
        let strong_walls = damage_pool(1000, 900, 500); // overflow 100, watch kills 100
        assert_eq!(weak_walls, 100);
        assert_eq!(strong_walls, 180);
        assert!(strong_walls > weak_walls);
    }

    #[test]
    fn test_damage_pool_never_negative() {
        assert!(damage_pool(0, 500, 500) >= 0);
        assert!(damage_pool(0, 0, 0) >= 0);
    }

    // =========================================================================
    // reparo_gen (given an already-computed damage pool)
    // =========================================================================

    #[test]
    fn test_reparo_gen_does_not_panic_on_near_destroyed_building() {
        let mut rng = Mt64::new(42);
        let buildings = life_pairs(&[building("Ruine", 1, 1)]);
        let mut scratch = Vec::new();
        for _ in 0..100 {
            let damage = reparo_gen(200, &buildings, &mut rng, &mut scratch);
            assert!(damage >= 0);
        }
    }

    #[test]
    fn test_reparo_gen_lower_bound_uses_max_life_not_current_life() {
        // Game: damages = min(pool, life, mt_rand(ceil(10), 100)) → never below 10, even
        // though ceil(life * 0.1) would be 5.
        let mut rng = Mt64::new(3);
        let buildings = [(50, 100)];
        let mut scratch = Vec::new();
        for _ in 0..2_000 {
            let damage = reparo_gen(1_000, &buildings, &mut rng, &mut scratch);
            assert!((10..=50).contains(&damage), "damage {damage} out of [10, 50]");
        }
    }

    #[test]
    fn test_reparo_gen_upper_bound_is_inclusive() {
        // mt_rand(1, 2) can return 2; realDamage cap is ceil(2 * 0.7) = 2.
        let mut rng = Mt64::new(11);
        let buildings = [(2, 2)];
        let mut scratch = Vec::new();
        let seen: std::collections::HashSet<i32> = (0..500)
            .map(|_| reparo_gen(1_000, &buildings, &mut rng, &mut scratch))
            .collect();
        assert_eq!(seen, [1, 2].into_iter().collect());
    }

    #[test]
    fn test_reparo_gen_skips_buildings_without_max_life() {
        let mut rng = Mt64::new(9);
        let buildings = [(0, 0), (-1, -5)];
        let mut scratch = Vec::new();
        assert_eq!(reparo_gen(1_000, &buildings, &mut rng, &mut scratch), 0);
    }

    #[test]
    fn test_reparo_gen_zero_pool_gives_zero_damage() {
        let mut rng = Mt64::new(1);
        let buildings = life_pairs(&default_buildings());
        let mut scratch = Vec::new();
        let damage = reparo_gen(0, &buildings, &mut rng, &mut scratch);
        assert_eq!(damage, 0);
    }

    #[test]
    fn test_reparo_gen_damage_is_non_negative_and_capped() {
        let mut rng = Mt64::new(7);
        let default_bs = default_buildings();
        let buildings = life_pairs(&default_bs);
        let mut scratch = Vec::new();
        let max_tank = default_bs
            .iter()
            .map(|b| min(b.life, (b.max_life as f64 * 0.7).ceil() as i32))
            .sum::<i32>();

        for _ in 0..50 {
            let damage = reparo_gen(100_000, &buildings, &mut rng, &mut scratch);
            assert!(damage >= 0);
            assert!(
                damage <= max_tank,
                "damage {} should not exceed buildings tank capacity {}",
                damage,
                max_tank
            );
        }
    }

    #[test]
    fn test_compute_statistics_quartiles() {
        let data = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let stats = compute_statistics(&data);
        assert_eq!(stats.min, 1);
        assert_eq!(stats.max, 8);
        assert_eq!(stats.mean, 4.5);
        assert_eq!(stats.median, 4.5);
        assert_eq!(stats.q1, 2.5);
        assert_eq!(stats.q3, 6.5);
    }

    #[test]
    fn test_compute_statistics_empty() {
        let stats = compute_statistics(&[]);
        assert_eq!(stats.min, 0);
        assert_eq!(stats.max, 0);
        assert_eq!(stats.mean, 0.0);
    }

    #[test]
    fn test_calculate_reparation_probabilities_covers_full_inclusive_range() {
        let buildings = default_buildings();
        let results = calculate_reparation_probabilities(0, 0, (100, 103), 10, &buildings);
        let attacks: Vec<i32> = results.iter().map(|(a, _)| *a).collect();
        assert_eq!(attacks.len(), 4, "range should be inclusive of tdg_max");
        assert!(attacks.contains(&100));
        assert!(attacks.contains(&103));
    }

    #[test]
    fn test_calculate_reparation_probabilities_empty_for_invalid_range() {
        let buildings = default_buildings();
        let results = calculate_reparation_probabilities(0, 0, (10, 5), 10, &buildings);
        assert!(results.is_empty());
    }

    #[test]
    fn test_calculate_reparation_probabilities_zero_stats_when_watch_kills_everything() {
        let buildings = default_buildings();
        // No total defense, watch=100: attacks up to 102 leave round((attack-100)*0.2) = 0
        // damage. attack 120 leaves round(20*0.2) = 4 damage to spread across buildings.
        let results = calculate_reparation_probabilities(0, 100, (90, 120), 200, &buildings);
        let by_attack: std::collections::HashMap<i32, Statistics> = results.into_iter().collect();

        for attack in [90, 100, 102] {
            let stats = by_attack[&attack];
            assert_eq!(stats.mean, 0.0, "attack {attack} should be fully killed by the watch");
            assert_eq!(stats.max, 0);
        }

        let above = by_attack[&120];
        assert!(above.max > 0, "attack beyond the watch should damage buildings");
    }

    #[test]
    fn test_calculate_reparation_probabilities_damage_despite_huge_total_defense() {
        let buildings = default_buildings();
        // Total defense stopping the whole attack leaves the watch nothing to kill, so
        // buildings still take 20% of the attack.
        let results = calculate_reparation_probabilities(999_999, 0, (100, 100), 200, &buildings);
        let (_, stats) = results[0];
        assert!(stats.max > 0, "total defense alone must not prevent building damage");
    }

    #[test]
    fn test_default_buildings_full_life() {
        let buildings = default_buildings();
        assert_eq!(buildings.len(), 60);
        assert!(buildings.iter().all(|b| b.life == b.max_life));
    }
}
