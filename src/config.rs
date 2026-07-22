//! Configuration de simulation extraite des paramètres Discord.

use serde::{Deserialize, Serialize};

pub const MAX_ITERATIONS: u32 = 10_000_000;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandOption {
    pub name: String,
    pub value: serde_json::Value,
}

/// Configuration de simulation avec tous les paramètres nécessaires.
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
}

impl SimConfig {
    /// Crée une configuration à partir des options de commande Discord.
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

/// Citoyen modélisé pour la simulation de survie détaillée.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimulationCitizen {
    pub name: String,
    pub defense: i32,
}

/// Payload envoyé via SQS au worker Lambda pour exécuter une simulation.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimulationJob {
    pub token: String,
    pub application_id: String,
    pub config: SimConfig,
    #[serde(default)]
    pub citizens: Vec<SimulationCitizen>,
}

/// Formate la configuration sous forme textuelle clé: valeur, réutilisable pour le copier/coller en mode interactif.
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

/// Formate les résultats de simulation pour l'affichage Discord.
pub fn format_results(
    config: &SimConfig,
    prob: f64,
    elapsed_ms: u128,
    total_runs: u64,
    avg_max_active: Option<f64>,
    citizens: &[SimulationCitizen],
    citizen_percentages: &[f64],
) -> String {
    #[derive(Serialize)]
    struct ReusableConfigPayload<'a> {
        config: &'a SimConfig,
        citizens: &'a [SimulationCitizen],
    }

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
    output.push_str(&format!("• **🔁 Itérations**: {}\n", config.iterations));
    if let Some(avg_active) = avg_max_active {
        output.push_str(&format!(
            "• **🧟 Max zombies actifs (moyenne)**: {:.1}\n",
            avg_active
        ));
    }
    output.push('\n');

    output.push_str(&format!("💀 **Probabilité de mort: {:.3}%**\n\n", prob));

    if config.is_complete && !citizens.is_empty() {
        output.push_str("**💀 Risque de mort par citoyen (détaillé) :**\n");
        let mut list: Vec<(&SimulationCitizen, f64)> = citizens
            .iter()
            .zip(citizen_percentages.iter().copied())
            .collect();
        // Sort alphabetically by name (case-insensitive)
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
    let reusable_payload = ReusableConfigPayload { config, citizens };
    if let Ok(payload_json) = serde_json::to_string(&reusable_payload) {
        output.push_str(&format!("\n-# cfg:{}", payload_json));
    }

    output
}

/// Extrait la configuration SimConfig et les citoyens à partir du texte d'un message de résultats Discord.
pub fn parse_result_message_content(content: &str) -> (SimConfig, Vec<SimulationCitizen>) {
    #[derive(Deserialize)]
    struct ReusableConfigPayload {
        config: SimConfig,
        #[serde(default)]
        citizens: Vec<SimulationCitizen>,
    }

    for line in content.lines() {
        if let Some(payload) = line.trim().strip_prefix("-# cfg:") {
            if let Ok(parsed) = serde_json::from_str::<ReusableConfigPayload>(payload.trim()) {
                return (parsed.config, parsed.citizens);
            }
        }
    }

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
            if let Some(name_start) = line.find("**") {
                let rest = &line[name_start + 2..];
                if let Some(name_end) = rest.find("**") {
                    let name = rest[..name_end].trim();
                    if let Some(colon_pos) = rest[name_end..].find(':') {
                        let after_colon = rest[name_end + colon_pos + 1..].trim();
                        let def_str = after_colon.split_whitespace().next().unwrap_or("0");
                        if let Ok(def) = def_str.parse::<i32>() {
                            citizens.push(SimulationCitizen {
                                name: name.to_string(),
                                defense: def,
                            });
                        }
                    }
                }
            }
        } else {
            if line.contains("Défense min") {
                if let Some(pos) = line.rfind(':') {
                    if let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.min_def = v;
                    }
                }
            } else if line.contains("Défense") && line.contains("•") {
                if let Some(pos) = line.rfind(':') {
                    if let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.defense = v;
                    }
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
                if let Some(pos) = line.rfind(':') {
                    if let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.nb_hab = v;
                    }
                }
            } else if line.contains("Jour") {
                if let Some(pos) = line.rfind(':') {
                    if let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                        config.day = v;
                    }
                }
            } else if line.contains("Itérations") {
                if let Some(pos) = line.rfind(':') {
                    if let Ok(v) = line[pos + 1..].trim().parse::<u32>() {
                        config.iterations = v;
                    }
                }
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
    fn test_parse_result_message_content_roundtrip() {
        let config = SimConfig {
            defense: 200,
            tdg_min: 60,
            tdg_max: 90,
            min_def: 25,
            nb_drapo: 2,
            day: 3,
            iterations: 1000,
            is_reactor_built: true,
            nb_hab: 35,
            b_level: Some(13),
            is_chaos: true,
            is_devastated: true,
            is_complete: true,
            home_bonus: 7,
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
        assert_eq!(parsed_config.nb_drapo, 2);
        assert!(parsed_config.is_reactor_built);
        assert_eq!(parsed_config.b_level, Some(13));
        assert!(parsed_config.is_chaos);
        assert!(parsed_config.is_devastated);
        assert_eq!(parsed_config.home_bonus, 7);
        assert!(parsed_config.is_complete);
        assert_eq!(parsed_citizens.len(), 2);
    }
}
