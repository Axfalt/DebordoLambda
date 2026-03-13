//! Configuration de simulation extraite des paramètres Discord.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct CommandOption {
    pub name: String,
    pub value: serde_json::Value,
}

/// Configuration de simulation avec tous les paramètres nécessaires.
#[derive(Debug, Default, Clone)]
pub struct SimConfig {
    pub defense_min: i32,
    pub defense_max: i32,
    pub tdg_min: i32,
    pub tdg_max: i32,
    pub min_def: i32,
    pub nb_drapo: i32,
    pub day: i32,
    pub iterations: u32,
    pub points: u32,
    pub is_reactor_built: bool,
}

impl SimConfig {
    /// Crée une configuration à partir des options de commande Discord.
    pub fn from_options(options: &[CommandOption]) -> Self {
        let mut config = SimConfig {
            iterations: 10000,
            points: 10,
            day: 1,
            ..Default::default()
        };

        for opt in options {
            match opt.name.as_str() {
                "defense_min" => config.defense_min = opt.value.as_i64().unwrap_or(0) as i32,
                "defense_max" => config.defense_max = opt.value.as_i64().unwrap_or(0) as i32,
                "tdg_min" => config.tdg_min = opt.value.as_i64().unwrap_or(0) as i32,
                "tdg_max" => config.tdg_max = opt.value.as_i64().unwrap_or(0) as i32,
                "min_def" => config.min_def = opt.value.as_i64().unwrap_or(0) as i32,
                "nb_drapo" => config.nb_drapo = opt.value.as_i64().unwrap_or(0) as i32,
                "day" => config.day = opt.value.as_i64().unwrap_or(1) as i32,
                "iterations" => config.iterations = opt.value.as_i64().unwrap_or(10000) as u32,
                "points" => config.points = opt.value.as_i64().unwrap_or(10) as u32,
                "reactor" => config.is_reactor_built = opt.value.as_bool().unwrap_or(false),
                _ => {}
            }
        }

        config
    }


    pub fn defense_range(&self) -> (i32, i32) {
        (self.defense_min, self.defense_max)
    }
    
    pub fn tdg_interval(&self) -> (i32, i32) {
        (self.tdg_min, self.tdg_max)
    }
}

/// Payload envoyé via SQS au worker Lambda pour exécuter une simulation.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SimulationJob {
    pub token: String,
    pub application_id: String,
    pub options: Vec<CommandOption>,
}

/// Formate les résultats de simulation pour l'affichage Discord.
pub fn format_results(config: &SimConfig, results: &[(f64, f64)]) -> String {
    let mut output = String::new();
    output.push_str("## 🎲 Résultats de la simulation\n\n");
    output.push_str("**Paramètres:**\n");
    output.push_str(&format!(
        "• Défense: {} - {}\n",
        config.defense_min, config.defense_max
    ));
    output.push_str(&format!(
        "• TDG: {} - {}\n",
        config.tdg_min, config.tdg_max
    ));
    output.push_str(&format!("• Défense min: {}\n", config.min_def));
    output.push_str(&format!("• Drapeaux: {}\n", config.nb_drapo));
    output.push_str(&format!("• Jour: {}\n", config.day));
    output.push_str(&format!("• Itérations: {}\n\n", config.iterations));

    output.push_str("```\n");
    output.push_str("Défense    | Prob. mort\n");
    output.push_str("-----------|-----------\n");

    for (defense, prob) in results {
        output.push_str(&format!("{:>9.1} | {:>8.3}%\n", defense, prob));
    }
    output.push_str("```");

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_opt(name: &str, value: serde_json::Value) -> CommandOption {
        CommandOption { name: name.to_string(), value }
    }

    // =========================================================================
    // SimConfig::from_options
    // =========================================================================

    #[test]
    fn test_simconfig_defaults_when_no_options() {
        let config = SimConfig::from_options(&[]);
        assert_eq!(config.day, 1);
        assert_eq!(config.iterations, 10000);
        assert_eq!(config.points, 10);
        assert_eq!(config.defense_min, 0);
        assert_eq!(config.defense_max, 0);
        assert!(!config.is_reactor_built);
    }

    #[test]
    fn test_simconfig_parses_all_options() {
        let options = vec![
            make_opt("defense_min", json!(100)),
            make_opt("defense_max", json!(200)),
            make_opt("tdg_min", json!(50)),
            make_opt("tdg_max", json!(80)),
            make_opt("min_def", json!(30)),
            make_opt("nb_drapo", json!(3)),
            make_opt("day", json!(7)),
            make_opt("iterations", json!(500)),
            make_opt("points", json!(5)),
            make_opt("reactor", json!(true)),
        ];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.defense_min, 100);
        assert_eq!(config.defense_max, 200);
        assert_eq!(config.tdg_min, 50);
        assert_eq!(config.tdg_max, 80);
        assert_eq!(config.min_def, 30);
        assert_eq!(config.nb_drapo, 3);
        assert_eq!(config.day, 7);
        assert_eq!(config.iterations, 500);
        assert_eq!(config.points, 5);
        assert!(config.is_reactor_built);
    }

    #[test]
    fn test_simconfig_partial_options_keep_defaults() {
        // Only override day; everything else should use defaults.
        let options = vec![make_opt("day", json!(5))];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.day, 5);
        assert_eq!(config.iterations, 10000);
        assert_eq!(config.points, 10);
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
    fn test_simconfig_defense_range() {
        let options = vec![
            make_opt("defense_min", json!(100)),
            make_opt("defense_max", json!(200)),
        ];
        let config = SimConfig::from_options(&options);
        assert_eq!(config.defense_range(), (100, 200));
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

    // =========================================================================
    // format_results
    // =========================================================================

    #[test]
    fn test_format_results_contains_section_headers() {
        let config = SimConfig::from_options(&[]);
        let output = format_results(&config, &[]);
        assert!(output.contains("Résultats de la simulation"));
        assert!(output.contains("Défense"));
        assert!(output.contains("Prob. mort"));
    }

    #[test]
    fn test_format_results_contains_config_params() {
        let options = vec![
            make_opt("day", json!(3)),
            make_opt("iterations", json!(500)),
            make_opt("defense_min", json!(100)),
            make_opt("defense_max", json!(200)),
        ];
        let config = SimConfig::from_options(&options);
        let output = format_results(&config, &[]);
        assert!(output.contains("Jour: 3"));
        assert!(output.contains("Itérations: 500"));
        assert!(output.contains("Défense: 100 - 200"));
    }

    #[test]
    fn test_format_results_contains_data_rows() {
        let config = SimConfig::from_options(&[]);
        let results = vec![(150.0_f64, 5.678_f64), (200.0, 0.001)];
        let output = format_results(&config, &results);
        assert!(output.contains("150"), "should contain defense value 150");
        assert!(output.contains("5.678"), "should contain probability 5.678");
        assert!(output.contains("200"), "should contain defense value 200");
    }

    #[test]
    fn test_format_results_empty_results_still_valid() {
        let config = SimConfig::from_options(&[]);
        let output = format_results(&config, &[]);
        // Should still produce a valid output with headers but no data rows
        assert!(output.contains("```"));
        assert!(output.contains("Défense"));
    }
}
