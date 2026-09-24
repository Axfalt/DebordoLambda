use serde::{Deserialize, Serialize};

pub const MAX_ITERATIONS: u32 = 10_000_000;
pub const MAX_REPARO_TOTAL_WORK: u64 = 20_000_000;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandOption {
    pub name: String,
    pub value: serde_json::Value,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct SimConfig {
    pub defense: i32,
    pub tdg_min: i32,
    pub tdg_max: i32,
    pub min_def: i32,
    pub nb_drapo: i32,
    pub day: i32,
    pub iterations: u32,
    pub is_reactor_built: bool,
    pub nb_hab: i32,
    pub b_level: Option<i32>,
    pub is_chaos: bool,
    pub is_devastated: bool,
    pub is_complete: bool,
    pub is_interactive: bool,
    pub custom_defenses: Option<String>,
    pub home_bonus: i32,
    #[serde(default)]
    pub veille: i32,
}

impl SimConfig {
    pub fn from_options(options: &[CommandOption]) -> Self {
        let mut config = SimConfig {
            iterations: 10000,
            day: 1,
            nb_hab: 40,
            ..Default::default()
        };

        for opt in options {
            match opt.name.as_str() {
                "defense" => config.defense = opt.value.as_i64().unwrap_or(0) as i32,
                "tdg_min" => config.tdg_min = opt.value.as_i64().unwrap_or(0) as i32,
                "tdg_max" => config.tdg_max = opt.value.as_i64().unwrap_or(0) as i32,
                "min_def" => config.min_def = opt.value.as_i64().unwrap_or(0) as i32,
                "nb_drapo" => config.nb_drapo = opt.value.as_i64().unwrap_or(0) as i32,
                "day" => config.day = opt.value.as_i64().unwrap_or(1) as i32,
                "iterations" => {
                    config.iterations =
                        (opt.value.as_i64().unwrap_or(10000) as u32).min(MAX_ITERATIONS)
                }
                "reactor" => config.is_reactor_built = opt.value.as_bool().unwrap_or(false),
                "nb_hab" => config.nb_hab = opt.value.as_i64().unwrap_or(40) as i32,
                "b_level" => config.b_level = opt.value.as_i64().map(|v| v as i32),
                "is_chaos" => config.is_chaos = opt.value.as_bool().unwrap_or(false),
                "is_devastated" => config.is_devastated = opt.value.as_bool().unwrap_or(false),
                "complete" => config.is_complete = opt.value.as_bool().unwrap_or(false),
                "interactive" => config.is_interactive = opt.value.as_bool().unwrap_or(false),
                "defenses" => config.custom_defenses = opt.value.as_str().map(|s| s.to_string()),
                "home_bonus" => config.home_bonus = opt.value.as_i64().unwrap_or(0) as i32,
                _ => {}
            }
        }

        config
    }

    pub fn tdg_interval(&self) -> (i32, i32) {
        (self.tdg_min, self.tdg_max)
    }
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimulationCitizen {
    pub name: String,
    pub defense: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone, Default, PartialEq, Eq)]
pub enum JobType {
    #[default]
    Debordo,
    Reparation,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct SimulationJob {
    pub token: String,
    pub application_id: String,
    pub config: SimConfig,
    #[serde(default)]
    pub citizens: Vec<SimulationCitizen>,
    #[serde(default)]
    pub job_type: JobType,
    #[serde(default)]
    pub buildings: Vec<reparo_lib::SimBuilding>,
}

pub fn format_conf(config: &SimConfig, citizens: &[SimulationCitizen]) -> String {
    let mut citizens_sorted = citizens.to_vec();
    citizens_sorted.sort_by_key(|a| a.name.to_lowercase());

    let mut config_lines = vec![
        format!("defense: {}", config.defense),
        format!("tdg: {}-{}", config.tdg_min, config.tdg_max),
    ];

    if !config.is_complete {
        config_lines.push(format!("min_def: {}", config.min_def));
    }

    config_lines.extend(vec![
        format!("nb_drapo: {}", config.nb_drapo),
        format!("day: {}", config.day),
        format!("iterations: {}", config.iterations),
        format!("reactor: {}", config.is_reactor_built),
        format!("nb_hab: {}", config.nb_hab),
        format!("chaos: {}", config.is_chaos),
        format!("devastated: {}", config.is_devastated),
    ]);

    if config.is_complete {
        config_lines.push("---".to_string());
        for c in &citizens_sorted {
            config_lines.push(format!("{}: {}", c.name, c.defense));
        }
    }

    config_lines.join("\n")
}

pub fn format_results(
    config: &SimConfig,
    prob: f64,
    elapsed_ms: u128,
    total_runs: u64,
    avg_max_active: Option<f64>,
    citizens: &[SimulationCitizen],
    citizen_percentages: &[f64],
) -> String {
    let mut output = String::new();
    output.push_str("## 🎲 Résultats de la simulation\n\n");
    output.push_str("**Paramètres:**\n");

    let fmt_line =
        |emoji_label: &str, val: i32| -> String { format!("• **{}**: {}\n", emoji_label, val) };

    let tdg_line = format!("• **🔭 TDG**: {} - {}\n", config.tdg_min, config.tdg_max);

    output.push_str(&fmt_line("🛡️ Défense", config.defense));
    output.push_str(&tdg_line);
    output.push_str(&fmt_line("🧑‍🤝‍🧑 Personnes en ville", config.nb_hab));
    if !config.is_complete {
        output.push_str(&fmt_line("🏠 Défense min", config.min_def));
    }
    output.push_str(&fmt_line("📅 Jour", config.day));
    if config.nb_drapo > 0 {
        output.push_str(&fmt_line("🚩 Drapeaux", config.nb_drapo));
    }
    if config.is_reactor_built {
        output.push_str("• **⚛️ Réacteur**: Oui\n");
    }
    if config.is_chaos {
        output.push_str("• **☣️ Chaos**: Oui\n");
    }
    if config.is_devastated {
        output.push_str("• **🏚️ Dévastée**: Oui\n");
    }
    if let Some(avg_active) = avg_max_active {
        output.push_str(&format!(
            "• **🧟 Max zombies actifs (moyenne)**: {:.1}\n",
            avg_active
        ));
    }
    // Always last, regardless of which optional lines above were printed.
    output.push_str(&format!("• **🔁 Itérations**: {}\n", config.iterations));
    output.push('\n');

    output.push_str(&format!("💀 **Probabilité de mort: {:.3}%**\n\n", prob));

    if config.is_complete && !citizens.is_empty() {
        output.push_str("**💀 Risque de mort par citoyen (détaillé) :**\n");
        let mut list: Vec<(&SimulationCitizen, f64)> = citizens
            .iter()
            .zip(citizen_percentages.iter().copied())
            .collect();
        list.sort_by_key(|a| a.0.name.to_lowercase());

        for &(citizen, c_prob) in &list {
            output.push_str(&format!(
                "• **{}**: {} 🛡️ — **{:.3}%**\n",
                citizen.name, citizen.defense, c_prob
            ));
        }
        output.push('\n');
    }

    output.push_str(&format!(
        "-# ⏱️ {} simulations en {}ms",
        total_runs, elapsed_ms
    ));

    output
}

pub fn format_reparo_results(
    config: &SimConfig,
    results: &[(i32, reparo_lib::Statistics)],
    elapsed_ms: u128,
    total_runs: u64,
    buildings: &[reparo_lib::SimBuilding],
) -> String {
    let mut output = String::new();
    output.push_str("## 🔧 Résultats de la simulation de réparation\n\n");
    output.push_str("**Paramètres:**\n");
    output.push_str(&format!("• **🛡️ Défense**: {}\n", config.defense));
    output.push_str(&format!("• **⚔️ Veille**: {}\n", config.veille));
    output.push_str(&format!(
        "• **🔭 TDG**: {} - {}\n",
        config.tdg_min, config.tdg_max
    ));
    output.push_str(&format!(
        "• **🏚️ Bâtiments pris en compte**: {}\n",
        buildings.len()
    ));
    output.push_str(&format!("• **🔁 Itérations**: {}\n", config.iterations));
    output.push('\n');

    if results.is_empty() {
        output.push_str("🔨 **Aucun dégât attendu sur cette plage d'attaque.**\n\n");
    } else {
        let mean_of_means: f64 =
            results.iter().map(|(_, s)| s.mean).sum::<f64>() / results.len() as f64;
        let overall_min = results.iter().map(|(_, s)| s.min).min().unwrap_or(0);
        let overall_max = results.iter().map(|(_, s)| s.max).max().unwrap_or(0);

        output.push_str(&format!(
            "🔨 **Dégâts moyens estimés: {:.1} PV** (min {} – max {})\n\n",
            mean_of_means, overall_min, overall_max
        ));
    }

    output.push_str(&format!(
        "🧱 **Capacité d'absorption des bâtiments: {} PV**\n-# Dégâts maximum encaissables en une nuit : au-delà, le surplus de dégâts est perdu.\n\n",
        reparo_lib::damage_capacity(buildings)
    ));

    output.push_str(&format!(
        "-# ⏱️ {} simulations en {}ms",
        total_runs, elapsed_ms
    ));

    output
}

pub fn format_reparo_conf(config: &SimConfig, buildings: &[reparo_lib::SimBuilding]) -> String {
    let mut buildings_sorted = buildings.to_vec();
    buildings_sorted.sort_by_key(|b| b.name.to_lowercase());

    let mut lines = vec![
        format!("defense: {}", config.defense),
        format!("veille: {}", config.veille),
        format!("tdg: {}-{}", config.tdg_min, config.tdg_max),
        format!("iterations: {}", config.iterations),
        "---".to_string(),
    ];

    for b in &buildings_sorted {
        lines.push(format!("{}: {}/{}", b.name, b.life, b.max_life));
    }

    lines.join("\n")
}

pub fn truncate_for_discord(text: &str, max_length: usize, notice: &str) -> String {
    if text.chars().count() <= max_length {
        return text.to_string();
    }

    let budget = max_length.saturating_sub(notice.chars().count());

    let mut truncated = String::new();
    for line in text.lines() {
        let candidate_len = truncated.chars().count()
            + line.chars().count()
            + usize::from(!truncated.is_empty());
        if candidate_len > budget {
            break;
        }
        if !truncated.is_empty() {
            truncated.push('\n');
        }
        truncated.push_str(line);
    }
    truncated.push_str(notice);
    truncated
}

fn parse_building_line(line: &str) -> Option<reparo_lib::SimBuilding> {
    let pos = line.rfind(':')?;
    let name = line[..pos].trim();
    let rest = line[pos + 1..].trim();
    let (life_str, max_str) = rest.split_once('/')?;
    let life = life_str.trim().parse::<i32>().ok()?;
    let max_life = max_str.trim().parse::<i32>().ok()?;

    if life >= 0 && max_life > 0 {
        Some(reparo_lib::SimBuilding {
            name: name.to_string(),
            life,
            max_life,
            breakable: true,
            temporary: false,
        })
    } else {
        None
    }
}

pub fn parse_reparo_modal_text(text: &str) -> (SimConfig, Vec<reparo_lib::SimBuilding>) {
    let mut config = SimConfig {
        iterations: 10000,
        ..Default::default()
    };
    let mut buildings = Vec::new();
    let mut in_buildings_section = false;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        if line == "---" {
            in_buildings_section = true;
            continue;
        }

        if in_buildings_section {
            if let Some(building) = parse_building_line(line) {
                buildings.push(building);
            }
            continue;
        }

        if let Some(pos) = line.find(':') {
            let key = line[..pos].trim().to_lowercase();
            let val = line[pos + 1..].trim();
            match key.as_str() {
                "defense" | "défense" => {
                    if let Ok(v) = val.parse::<i32>() {
                        config.defense = v;
                    }
                }
                "veille" => {
                    if let Ok(v) = val.parse::<i32>() {
                        config.veille = v;
                    }
                }
                "tdg" => {
                    if let Some((mn, mx)) = val.split_once('-') {
                        if let Ok(v) = mn.trim().parse::<i32>() {
                            config.tdg_min = v;
                        }
                        if let Ok(v) = mx.trim().parse::<i32>() {
                            config.tdg_max = v;
                        }
                    }
                }
                "iterations" | "itérations" => {
                    if let Ok(v) = val.parse::<u32>() {
                        config.iterations = v.min(MAX_ITERATIONS);
                    }
                }
                _ => {}
            }
        }
    }

    (config, buildings)
}

pub fn parse_reparo_result_content(content: &str) -> SimConfig {
    let mut config = SimConfig {
        iterations: 10000,
        ..Default::default()
    };

    for line in content.lines() {
        let line = line.trim();

        if line.contains("Défense") && line.contains("•") {
            if let Some(pos) = line.rfind(':')
                && let Ok(v) = line[pos + 1..].trim().parse::<i32>()
            {
                config.defense = v;
            }
        } else if line.contains("Veille") && line.contains("•") {
            if let Some(pos) = line.rfind(':')
                && let Ok(v) = line[pos + 1..].trim().parse::<i32>()
            {
                config.veille = v;
            }
        } else if line.contains("TDG") {
            if let Some(pos) = line.rfind(':') {
                let val_str = line[pos + 1..].trim();
                if let Some(dash_pos) = val_str.find('-') {
                    if let Ok(mn) = val_str[..dash_pos].trim().parse::<i32>() {
                        config.tdg_min = mn;
                    }
                    if let Ok(mx) = val_str[dash_pos + 1..].trim().parse::<i32>() {
                        config.tdg_max = mx;
                    }
                }
            }
        } else if line.contains("Itérations") {
            if let Some(pos) = line.rfind(':')
                && let Ok(v) = line[pos + 1..].trim().parse::<u32>()
            {
                config.iterations = v;
            }
        }
    }

    config
}

pub fn parse_result_message_content(content: &str) -> (SimConfig, Vec<SimulationCitizen>) {

    let mut config = SimConfig {
        iterations: 10000,
        day: 1,
        nb_hab: 40,
        ..Default::default()
    };
    let mut citizens = Vec::new();
    let is_complete = content.contains("Risque de mort par citoyen");
    config.is_complete = is_complete;

    let mut in_citizens_section = false;

    for line in content.lines() {
        let line = line.trim();

        if line.contains("Risque de mort par citoyen") {
            in_citizens_section = true;
            continue;
        }

        if in_citizens_section {
            if line.starts_with("-#") || line.is_empty() {
                in_citizens_section = false;
                continue;
            }
            // Format: • **Name**: 25 🛡️ — **5.000%**
            if line.starts_with("• **")
                && let Some(colon_pos) = line.find(':') {
                    let name = line[4..colon_pos].trim_matches('*').trim();
                    let rest = line[colon_pos + 1..].trim();
                    if let Some(def_str) = rest.split_whitespace().next()
                        && let Ok(def) = def_str.parse::<i32>() {
                            citizens.push(SimulationCitizen {
                                name: name.to_string(),
                                defense: def,
                            });
                        }
                }
        } else {
            if line.contains("Défense min") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.min_def = v;
                    }
            } else if line.contains("Défense") && line.contains("•") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.defense = v;
                    }
            } else if line.contains("TDG") {
                if let Some(pos) = line.rfind(':') {
                    let val_str = line[pos + 1..].trim();
                    if let Some(dash_pos) = val_str.find('-') {
                        if let Ok(mn) = val_str[..dash_pos].trim().parse::<i32>() {
                            config.tdg_min = mn;
                        }
                        if let Ok(mx) = val_str[dash_pos + 1..].trim().parse::<i32>() {
                            config.tdg_max = mx;
                        }
                    }
                }
            } else if line.contains("Personnes en ville") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.nb_hab = v;
                    }
            } else if line.contains("Jour") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.day = v;
                    }
            } else if line.contains("Itérations") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<u32>() {
                        config.iterations = v;
                    }
            } else if line.contains("Drapeaux") {
                if let Some(pos) = line.rfind(':')
                    && let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.nb_drapo = v;
                    }
            } else if line.contains("Réacteur") {
                config.is_reactor_built = true;
            } else if line.contains("Chaos") {
                config.is_chaos = true;
            } else if line.contains("Dévastée") {
                config.is_devastated = true;
            }
        }
    }

    (config, citizens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_opt(name: &str, value: serde_json::Value) -> CommandOption {
        CommandOption {
            name: name.to_string(),
            value,
        }
    }

    #[test]
    fn test_simconfig_defaults_when_no_options() {
        let config = SimConfig::from_options(&[]);
        assert_eq!(config.day, 1);
        assert_eq!(config.iterations, 10000);
        assert_eq!(config.defense, 0);
        assert_eq!(config.nb_hab, 40);
        assert!(!config.is_reactor_built);
    }

    #[test]
    fn test_simconfig_parses_all_options() {
        let options = vec![
            make_opt("defense", json!(150)),
            make_opt("tdg_min", json!(50)),
            make_opt("tdg_max", json!(80)),
            make_opt("min_def", json!(30)),
            make_opt("nb_drapo", json!(3)),
            make_opt("day", json!(7)),
            make_opt("iterations", json!(500)),
            make_opt("nb_hab", json!(12)),
            make_opt("reactor", json!(true)),
        ];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.defense, 150);
        assert_eq!(config.tdg_min, 50);
        assert_eq!(config.tdg_max, 80);
        assert_eq!(config.min_def, 30);
        assert_eq!(config.nb_drapo, 3);
        assert_eq!(config.day, 7);
        assert_eq!(config.iterations, 500);
        assert_eq!(config.nb_hab, 12);
        assert!(config.is_reactor_built);
    }

    #[test]
    fn test_simconfig_partial_options_keep_defaults() {
        // Only override day; everything else should use defaults.
        let options = vec![make_opt("day", json!(5))];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.day, 5);
        assert_eq!(config.iterations, 10000);
        assert!(!config.is_reactor_built);
    }

    #[test]
    fn test_simconfig_unknown_option_is_ignored() {
        let options = vec![make_opt("unknown_field", json!(42))];
        let config = SimConfig::from_options(&options);
        // Defaults should be intact
        assert_eq!(config.day, 1);
        assert_eq!(config.iterations, 10000);
    }

    #[test]
    fn test_simconfig_defense() {
        let options = vec![make_opt("defense", json!(150))];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.defense, 150);
    }

    #[test]
    fn test_simconfig_tdg_interval() {
        let options = vec![
            make_opt("tdg_min", json!(50)),
            make_opt("tdg_max", json!(80)),
        ];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.tdg_interval(), (50, 80));
    }

    #[test]
    fn test_simconfig_parses_interactive() {
        let options = vec![make_opt("interactive", json!(true))];
        let config = SimConfig::from_options(&options);
        assert!(config.is_interactive);
    }

    #[test]
    fn test_format_results_visibility_matrix() {
        let config_std = SimConfig {
            defense: 100,
            tdg_min: 50,
            tdg_max: 60,
            min_def: 15,
            home_bonus: 4,
            is_complete: false,
            ..Default::default()
        };
        let res_std = format_results(&config_std, 5.0, 10, 1000, Some(25.0), &[], &[]);
        assert!(res_std.contains("Défense min"));
        assert!(!res_std.contains("Bonus maison"));
        assert!(res_std.contains("Max zombies actifs (moyenne)"));

        let config_comp = SimConfig {
            defense: 100,
            tdg_min: 50,
            tdg_max: 60,
            min_def: 15,
            home_bonus: 4,
            is_complete: true,
            ..Default::default()
        };
        let res_comp = format_results(&config_comp, 5.0, 10, 1000, None, &[], &[]);
        assert!(!res_comp.contains("Défense min"));
        assert!(!res_comp.contains("Bonus maison"));
        assert!(!res_comp.contains("Max zombies actifs (moyenne)"));
    }

    #[test]
    fn test_format_results_iterations_always_last_parameter_line() {
        // With every optional parameter line enabled at once, "Itérations" must still be the
        // last bullet in the **Paramètres:** list, regardless of which combination is active.
        let config = SimConfig {
            defense: 100,
            tdg_min: 50,
            tdg_max: 60,
            min_def: 15,
            nb_drapo: 3,
            is_reactor_built: true,
            is_chaos: true,
            is_devastated: true,
            iterations: 12345,
            ..Default::default()
        };
        let output = format_results(&config, 5.0, 10, 1000, Some(25.0), &[], &[]);

        let iterations_pos = output.find("Itérations").expect("Itérations line missing");
        for label in [
            "Défense min",
            "Drapeaux",
            "Réacteur",
            "Chaos",
            "Dévastée",
            "Max zombies actifs",
        ] {
            let label_pos = output.find(label).unwrap_or_else(|| panic!("{label} line missing"));
            assert!(
                iterations_pos > label_pos,
                "Itérations (at {iterations_pos}) should come after {label} (at {label_pos})"
            );
        }

        // Iterations must still appear before the blank line that ends the parameter block.
        let params_end = output.find("💀 **Probabilité").expect("probability line missing");
        assert!(iterations_pos < params_end);
    }

    #[test]
    fn test_format_reparo_results_headline_stats() {
        let config = SimConfig {
            defense: 150,
            veille: 35,
            tdg_min: 200,
            tdg_max: 202,
            iterations: 500,
            ..Default::default()
        };
        let results = vec![
            (
                200,
                reparo_lib::Statistics { mean: 10.0, median: 9.0, min: 2, max: 20, q1: 5.0, q3: 15.0 },
            ),
            (
                201,
                reparo_lib::Statistics { mean: 20.0, median: 19.0, min: 8, max: 40, q1: 15.0, q3: 25.0 },
            ),
        ];
        let buildings: Vec<reparo_lib::SimBuilding> = (0..60)
            .map(|i| reparo_lib::SimBuilding {
                name: format!("Bâtiment {i}"),
                life: 10,
                max_life: 10,
                breakable: true,
                temporary: false,
            })
            .collect();
        let output = format_reparo_results(&config, &results, 42, 1000, &buildings);

        assert!(output.contains("Défense**: 150"));
        assert!(output.contains("Veille**: 35"));
        assert!(output.contains("TDG**: 200 - 202"));
        assert!(output.contains("Bâtiments pris en compte**: 60"));
        assert!(output.contains("Itérations**: 500"));
        assert!(output.contains("Dégâts moyens estimés: 15.0 PV"));
        assert!(output.contains("min 2"));
        assert!(output.contains("max 40"));
        assert!(output.contains("1000 simulations en 42ms"));
        // 60 buildings at 10/10: each absorbs at most ceil(10 * 0.7) = 7.
        assert!(output.contains("Capacité d'absorption des bâtiments: 420 PV"));
        assert!(!output.contains("||"));
    }

    #[test]
    fn test_format_reparo_results_empty_results() {
        let config = SimConfig {
            defense: 500,
            tdg_min: 10,
            tdg_max: 20,
            iterations: 500,
            ..Default::default()
        };
        let output = format_reparo_results(&config, &[], 5, 0, &[]);
        assert!(output.contains("Aucun dégât attendu"));
        assert!(output.contains("Capacité d'absorption des bâtiments: 0 PV"));
        assert!(!output.contains("||"));
    }

    #[test]
    fn test_parse_reparo_result_content_roundtrip() {
        let config = SimConfig {
            defense: 150,
            veille: 35,
            tdg_min: 200,
            tdg_max: 202,
            iterations: 500,
            ..Default::default()
        };
        let results = vec![(
            200,
            reparo_lib::Statistics { mean: 10.0, median: 9.0, min: 2, max: 20, q1: 5.0, q3: 15.0 },
        )];
        let buildings = vec![reparo_lib::SimBuilding {
            name: "Muraille".to_string(),
            life: 6,
            max_life: 25,
            breakable: true,
            temporary: false,
        }];
        let mut content = format_reparo_results(&config, &results, 42, 1000, &buildings);
        content.push_str("\n\n🖼️ **Graphique**: https://quickchart.io/chart/render/example");

        let parsed_config = parse_reparo_result_content(&content);
        assert_eq!(parsed_config.defense, 150);
        assert_eq!(parsed_config.veille, 35);
        assert_eq!(parsed_config.tdg_min, 200);
        assert_eq!(parsed_config.tdg_max, 202);
        assert_eq!(parsed_config.iterations, 500);
    }

    #[test]
    fn test_format_conf_standard_and_complete() {
        let config = SimConfig {
            defense: 150,
            tdg_min: 50,
            tdg_max: 80,
            min_def: 20,
            nb_drapo: 2,
            day: 5,
            iterations: 10000,
            is_reactor_built: false,
            nb_hab: 40,
            is_chaos: false,
            is_devastated: false,
            is_complete: false,
            ..Default::default()
        };
        let conf_std = format_conf(&config, &[]);
        assert!(conf_std.contains("defense: 150"));
        assert!(conf_std.contains("tdg: 50-80"));
        assert!(conf_std.contains("min_def: 20"));
        assert!(conf_std.contains("day: 5"));

        let config_complete = SimConfig {
            is_complete: true,
            ..config
        };
        let citizens = vec![
            SimulationCitizen {
                name: "Bob".to_string(),
                defense: 30,
            },
            SimulationCitizen {
                name: "Alice".to_string(),
                defense: 25,
            },
        ];
        let conf_comp = format_conf(&config_complete, &citizens);
        assert!(!conf_comp.contains("min_def:"));
        assert!(conf_comp.contains("---\nAlice: 25\nBob: 30"));
    }

    #[test]
    fn test_format_reparo_conf_and_parse_roundtrip() {
        let config = SimConfig {
            defense: 150,
            veille: 35,
            tdg_min: 50,
            tdg_max: 80,
            iterations: 5000,
            ..Default::default()
        };
        let buildings = vec![
            reparo_lib::SimBuilding {
                name: "Muraille".to_string(),
                life: 25,
                max_life: 25,
                breakable: true,
                temporary: false,
            },
            reparo_lib::SimBuilding {
                name: "Atelier".to_string(),
                life: 19,
                max_life: 25,
                breakable: true,
                temporary: false,
            },
        ];

        let text = format_reparo_conf(&config, &buildings);
        assert!(text.contains("defense: 150"));
        assert!(text.contains("veille: 35"));
        assert!(text.contains("tdg: 50-80"));
        assert!(text.contains("iterations: 5000"));
        assert!(text.contains("Atelier: 19/25"));
        assert!(text.contains("Muraille: 25/25"));

        let (parsed_config, mut parsed_buildings) = parse_reparo_modal_text(&text);
        assert_eq!(parsed_config.defense, 150);
        assert_eq!(parsed_config.veille, 35);
        assert_eq!(parsed_config.tdg_min, 50);
        assert_eq!(parsed_config.tdg_max, 80);
        assert_eq!(parsed_config.iterations, 5000);

        parsed_buildings.sort_by_key(|b| b.name.clone());
        assert_eq!(parsed_buildings.len(), 2);
        assert_eq!(parsed_buildings[0].name, "Atelier");
        assert_eq!(parsed_buildings[0].life, 19);
        assert_eq!(parsed_buildings[0].max_life, 25);
        assert_eq!(parsed_buildings[1].name, "Muraille");
        assert_eq!(parsed_buildings[1].life, 25);
    }

    #[test]
    fn test_parse_reparo_modal_text_ignores_malformed_building_lines() {
        let text = "defense: 100\nveille: 10\ntdg: 10-20\niterations: 1000\n---\nGoodBuilding: 5/10\nBadLine without slash\nAnother: notanumber/10";
        let (config, buildings) = parse_reparo_modal_text(text);
        assert_eq!(config.defense, 100);
        assert_eq!(config.veille, 10);
        assert_eq!(buildings.len(), 1);
        assert_eq!(buildings[0].name, "GoodBuilding");
    }

    #[test]
    fn test_parse_reparo_modal_text_rejects_negative_or_zero_building_values() {
        let text = "defense: 100\nveille: 10\ntdg: 10-20\niterations: 1000\n---\nNegativeLife: -5/10\nNegativeMax: 10/-5\nZeroMax: 5/0\nValid: 5/10";
        let (_, buildings) = parse_reparo_modal_text(text);
        assert_eq!(buildings.len(), 1);
        assert_eq!(buildings[0].name, "Valid");
    }

    #[test]
    fn test_truncate_for_discord_leaves_short_text_untouched() {
        let text = "defense: 100\ntdg: 10-20\n---\nMuraille: 25/25";
        assert_eq!(truncate_for_discord(text, 4000, "\n… (tronqué)"), text);
    }

    #[test]
    fn test_truncate_for_discord_stays_under_limit_and_keeps_whole_lines() {
        let mut lines = vec!["defense: 100".to_string(), "---".to_string()];
        for i in 0..500 {
            lines.push(format!("Bâtiment numéro {i}: 25/25"));
        }
        let text = lines.join("\n");
        assert!(text.chars().count() > 4000);

        let truncated = truncate_for_discord(&text, 4000, "\n… (tronqué)");
        assert!(truncated.chars().count() <= 4000);
        assert!(truncated.contains("tronqué"));
        // Every kept building line must be a complete, untruncated original line.
        for line in truncated.lines() {
            if line.starts_with("Bâtiment numéro") {
                assert!(text.contains(line));
            }
        }
    }

    #[test]
    fn test_parse_reparo_modal_text_accepts_french_key_aliases() {
        let text = "défense: 150\ntdg: 10-20\nitérations: 500\n---";
        let (config, _) = parse_reparo_modal_text(text);
        assert_eq!(config.defense, 150);
        assert_eq!(config.iterations, 500);
    }

    #[test]
    fn test_parse_reparo_modal_text_defense_and_veille_are_separate_fields() {
        let text = "defense: 1463\nveille: 42\ntdg: 10-20\niterations: 500\n---";
        let (config, _) = parse_reparo_modal_text(text);
        assert_eq!(config.defense, 1463);
        assert_eq!(config.veille, 42);
    }

    #[test]
    fn test_simulation_job_deserializes_without_new_fields() {
        let old_json = serde_json::json!({
            "token": "tok",
            "application_id": "app",
            "config": SimConfig::default(),
            "citizens": []
        });
        let job: SimulationJob = serde_json::from_value(old_json).unwrap();
        assert_eq!(job.job_type, JobType::Debordo);
        assert!(job.buildings.is_empty());
    }

    #[test]
    fn test_parse_result_message_content_roundtrip() {
        let config = SimConfig {
            defense: 200,
            tdg_min: 60,
            tdg_max: 90,
            min_def: 25,
            day: 3,
            iterations: 1000,
            nb_hab: 35,
            is_complete: true,
            ..Default::default()
        };
        let citizens = vec![
            SimulationCitizen {
                name: "Alice".to_string(),
                defense: 30,
            },
            SimulationCitizen {
                name: "Bob".to_string(),
                defense: 25,
            },
        ];
        let percentages = vec![5.0, 10.0];
        let result_text = format_results(&config, 12.5, 42, 1000, None, &citizens, &percentages);

        let (parsed_config, parsed_citizens) = parse_result_message_content(&result_text);
        assert_eq!(parsed_config.defense, 200);
        assert_eq!(parsed_config.tdg_min, 60);
        assert_eq!(parsed_config.tdg_max, 90);
        assert_eq!(parsed_config.nb_hab, 35);
        assert_eq!(parsed_config.day, 3);
        assert_eq!(parsed_config.iterations, 1000);
        assert!(parsed_config.is_complete);
        assert_eq!(parsed_citizens.len(), 2);
        assert_eq!(parsed_citizens[0].name, "Alice");
        assert_eq!(parsed_citizens[0].defense, 30);
        assert_eq!(parsed_citizens[1].name, "Bob");
        assert_eq!(parsed_citizens[1].defense, 25);
    }

    #[test]
    fn test_parse_result_message_content_roundtrips_reactor_chaos_devastated_flags() {
        let config = SimConfig {
            defense: 200,
            tdg_min: 60,
            tdg_max: 90,
            day: 3,
            iterations: 1000,
            nb_hab: 35,
            nb_drapo: 2,
            is_reactor_built: true,
            is_chaos: true,
            is_devastated: true,
            ..Default::default()
        };
        let result_text = format_results(&config, 12.5, 42, 1000, None, &[], &[]);

        let (parsed_config, _) = parse_result_message_content(&result_text);
        assert_eq!(parsed_config.nb_drapo, 2);
        assert!(
            parsed_config.is_reactor_built,
            "reactor flag should survive the round-trip through the results message"
        );
        assert!(parsed_config.is_chaos);
        assert!(parsed_config.is_devastated);
    }

    #[test]
    fn test_parse_result_message_content_defaults_flags_to_false_when_absent() {
        let config = SimConfig {
            defense: 100,
            tdg_min: 50,
            tdg_max: 60,
            day: 1,
            iterations: 100,
            nb_hab: 40,
            ..Default::default()
        };
        let result_text = format_results(&config, 1.0, 1, 100, None, &[], &[]);

        let (parsed_config, _) = parse_result_message_content(&result_text);
        assert_eq!(parsed_config.nb_drapo, 0);
        assert!(!parsed_config.is_reactor_built);
        assert!(!parsed_config.is_chaos);
        assert!(!parsed_config.is_devastated);
    }
}
