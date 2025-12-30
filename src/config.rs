//! Configuration de simulation extraite des paramètres Discord.

use serde::Deserialize;
#[derive(Debug, Deserialize, Clone)]
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
            points: 10000,
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
