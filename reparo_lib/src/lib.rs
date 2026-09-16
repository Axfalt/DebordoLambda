//! Simulation Monte-Carlo des dégâts de réparation infligés aux bâtiments d'une ville
//! lors d'une attaque de zombies. Porté depuis le prototype ReparoStats.

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

/// Liste de bâtiments par défaut (pleine vie) utilisée quand aucune donnée MyHordes n'est
/// disponible pour l'utilisateur. Restreinte aux prototypes réellement déblocables en mode
/// Pandemonium (`panda`), d'après la clé `unlocked_buildings` de la ville de type `panda` dans
/// `config/app/rules.yml` du moteur MyHordes (88 codenames — pas de `disabled_buildings` propre
/// à ce mode) recoupée avec le catalogue complet des prototypes relevé via un appel réel à
/// l'API MyHordes (`GET /api/x/json/buildings`, indépendant de toute ville — contrairement à
/// `city.buildings` sur `/api/x/json/me` qui ne liste que les bâtiments déjà construits dans la
/// ville courante). Les 88 codenames se sont tous résolus dans le catalogue ; filtré aux
/// bâtiments `breakable && !temporary`, à l'image du filtre appliqué lors de la récupération
/// live dans `handle_reparo_command`.
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

/// Une seule simulation: répartit les dégâts d'une attaque sur les bâtiments de la ville
/// et retourne le total de points de vie endommagés (à réparer).
///
/// Prend `(life, max_life)` plutôt que `&[SimBuilding]` pour éviter de cloner le nom (String)
/// de chaque bâtiment à chaque itération — seuls ces deux entiers varient dans la boucle.
fn reparo_gen(attack: i32, watch_def: i32, buildings: &[(i32, i32)], rng: &mut Mt64) -> i32 {
    let damage_inflicted = ((attack - watch_def) as f64 * 0.2).ceil() as i32;
    let mut damage_counter = damage_inflicted;
    let mut bs = buildings.to_vec();
    let mut total_damaged_hp = 0;

    bs.shuffle(rng);

    while damage_counter > 0 && !bs.is_empty() {
        let (life, max_life) = bs.pop().unwrap();
        let lower_damage_limit = (life as f64 * 0.1).ceil() as i32;

        // Guard against an empty sampling range (e.g. a building already near destroyed,
        // where lower_damage_limit >= max_life) — sampling such a range panics.
        let raw_damage = if lower_damage_limit >= max_life {
            life
        } else {
            rng.random_range(lower_damage_limit..max_life)
        };

        let damages = min(life, raw_damage);
        let damages = min(damages, damage_counter);

        let real_damages = min(damages, (max_life as f64 * 0.7).ceil() as i32);
        total_damaged_hp += real_damages;
        damage_counter -= damages;
    }

    total_damaged_hp
}

fn reparostats(attack: i32, watch_def: i32, iterations: u32, buildings: &[SimBuilding]) -> Vec<i32> {
    let mut rng = Mt64::new(rand::random());
    // Converted once per attack value rather than per iteration — reparo_gen only needs the
    // (life, max_life) pair, a cheap Copy tuple, not the whole SimBuilding (with its String).
    let life_pairs: Vec<(i32, i32)> = buildings.iter().map(|b| (b.life, b.max_life)).collect();
    (0..iterations)
        .map(|_| reparo_gen(attack, watch_def, &life_pairs, &mut rng))
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

    let median = if len % 2 == 0 {
        (sorted_data[len / 2 - 1] + sorted_data[len / 2]) as f64 / 2.0
    } else {
        sorted_data[len / 2] as f64
    };

    let min = *sorted_data.first().unwrap();
    let max = *sorted_data.last().unwrap();

    let q1 = if len % 4 == 0 {
        (sorted_data[len / 4 - 1] + sorted_data[len / 4]) as f64 / 2.0
    } else {
        sorted_data[len / 4] as f64
    };

    let q3 = if (len * 3) % 4 == 0 {
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

/// Calcule les statistiques de dégâts de réparation pour chaque valeur d'attaque possible
/// dans l'intervalle de TDG donné.
pub fn calculate_reparation_probabilities(
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
            let results = reparostats(attack, watch_def, iterations, buildings);
            let stats = compute_statistics(&results);
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

    #[test]
    fn test_reparo_gen_does_not_panic_on_near_destroyed_building() {
        // life=1, max_life=1 => lower_damage_limit=ceil(0.1)=1 == max_life => empty range guard.
        let mut rng = Mt64::new(42);
        let buildings = life_pairs(&[building("Ruine", 1, 1)]);
        for _ in 0..100 {
            let damage = reparo_gen(1000, 0, &buildings, &mut rng);
            assert!(damage >= 0);
        }
    }

    #[test]
    fn test_reparo_gen_zero_overflow_gives_zero_damage() {
        let mut rng = Mt64::new(1);
        let buildings = life_pairs(&default_buildings());
        // attack <= watch_def => no damage budget.
        let damage = reparo_gen(50, 100, &buildings, &mut rng);
        assert_eq!(damage, 0);
    }

    #[test]
    fn test_reparo_gen_damage_is_non_negative_and_capped() {
        let mut rng = Mt64::new(7);
        let default_bs = default_buildings();
        let buildings = life_pairs(&default_bs);
        let max_tank = default_bs
            .iter()
            .map(|b| min(b.life, (b.max_life as f64 * 0.7).ceil() as i32))
            .sum::<i32>();

        for _ in 0..50 {
            let damage = reparo_gen(100_000, 0, &buildings, &mut rng);
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
        let results = calculate_reparation_probabilities(0, (100, 103), 10, &buildings);
        let attacks: Vec<i32> = results.iter().map(|(a, _)| *a).collect();
        assert_eq!(attacks.len(), 4, "range should be inclusive of tdg_max");
        assert!(attacks.contains(&100));
        assert!(attacks.contains(&103));
    }

    #[test]
    fn test_calculate_reparation_probabilities_empty_for_invalid_range() {
        let buildings = default_buildings();
        let results = calculate_reparation_probabilities(0, (10, 5), 10, &buildings);
        assert!(results.is_empty());
    }

    #[test]
    fn test_default_buildings_full_life() {
        let buildings = default_buildings();
        assert_eq!(buildings.len(), 60);
        assert!(buildings.iter().all(|b| b.life == b.max_life));
    }
}
