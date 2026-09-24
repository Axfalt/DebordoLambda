//! Génération d'un graphique QuickChart.io pour visualiser les statistiques de réparation.

use reparo_lib::Statistics;

/// Construit la configuration Chart.js représentant l'évolution des dégâts de réparation
/// (moyenne, bande q1-q3, min/max) en fonction de la force de l'attaque.
pub fn build_chart_config(results: &[(i32, Statistics)]) -> serde_json::Value {
    let labels: Vec<i32> = results.iter().map(|(attack, _)| *attack).collect();
    let q1: Vec<f64> = results.iter().map(|(_, s)| s.q1).collect();
    let q3: Vec<f64> = results.iter().map(|(_, s)| s.q3).collect();
    let mean: Vec<f64> = results.iter().map(|(_, s)| s.mean).collect();
    let min: Vec<i32> = results.iter().map(|(_, s)| s.min).collect();
    let max: Vec<i32> = results.iter().map(|(_, s)| s.max).collect();

    serde_json::json!({
        "type": "line",
        "data": {
            "labels": labels,
            "datasets": [
                {
                    "label": "Q1",
                    "data": q1,
                    "borderColor": "transparent",
                    "backgroundColor": "transparent",
                    "fill": false,
                    "pointRadius": 0
                },
                {
                    "label": "Q1-Q3",
                    "data": q3,
                    "borderColor": "transparent",
                    "backgroundColor": "rgba(54, 162, 235, 0.25)",
                    "fill": 0,
                    "pointRadius": 0
                },
                {
                    "label": "Moyenne",
                    "data": mean,
                    "borderColor": "rgb(54, 162, 235)",
                    "backgroundColor": "transparent",
                    "fill": false,
                    "pointRadius": 0,
                    "borderWidth": 2
                },
                {
                    "label": "Min",
                    "data": min,
                    "borderColor": "rgba(120, 120, 120, 0.6)",
                    "backgroundColor": "transparent",
                    "borderDash": [5, 5],
                    "fill": false,
                    "pointRadius": 0
                },
                {
                    "label": "Max",
                    "data": max,
                    "borderColor": "rgba(120, 120, 120, 0.6)",
                    "backgroundColor": "transparent",
                    "borderDash": [5, 5],
                    "fill": false,
                    "pointRadius": 0
                }
            ]
        },
        "options": {
            "title": {
                "display": true,
                "text": "Dégâts de réparation estimés selon la force de l'attaque"
            },
            "scales": {
                "xAxes": [{ "scaleLabel": { "display": true, "labelString": "Attaque" } }],
                "yAxes": [{ "scaleLabel": { "display": true, "labelString": "Dommages" } }]
            }
        }
    })
}

/// Construit une URL QuickChart.io affichant directement le graphique (rendu à la volée via le
/// endpoint GET `/chart`), sans appel réseau ni dépendance à la persistance d'une URL courte
/// créée via `/chart/create` — évite qu'un lien expiré ou une création manquée empêche le
/// graphique de s'afficher dans l'embed Discord.
pub fn build_chart_url(chart_config: &serde_json::Value) -> String {
    let chart_json = chart_config.to_string();

    reqwest::Url::parse_with_params(
        "https://quickchart.io/chart",
        &[
            ("c", chart_json.as_str()),
            ("width", "800"),
            ("height", "400"),
            ("backgroundColor", "white"),
        ],
    )
    .map(|url| url.to_string())
    // Infallible in practice (fixed valid base URL, percent-encoding handles any content), but
    // fall back to a minimal-config URL rather than panicking if it ever weren't.
    .unwrap_or_else(|_| "https://quickchart.io/chart?c=%7B%7D".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_results() -> Vec<(i32, Statistics)> {
        vec![
            (
                100,
                Statistics {
                    mean: 10.0,
                    median: 9.0,
                    min: 2,
                    max: 20,
                    q1: 5.0,
                    q3: 15.0,
                },
            ),
            (
                101,
                Statistics {
                    mean: 12.0,
                    median: 11.0,
                    min: 3,
                    max: 22,
                    q1: 6.0,
                    q3: 17.0,
                },
            ),
        ]
    }

    #[test]
    fn build_chart_config_has_matching_label_and_dataset_lengths() {
        let results = sample_results();
        let config = build_chart_config(&results);

        let labels = config["data"]["labels"].as_array().unwrap();
        assert_eq!(labels.len(), 2);
        assert_eq!(labels[0], 100);
        assert_eq!(labels[1], 101);

        let datasets = config["data"]["datasets"].as_array().unwrap();
        assert_eq!(datasets.len(), 5);
        for dataset in datasets {
            assert_eq!(dataset["data"].as_array().unwrap().len(), 2);
        }
    }

    #[test]
    fn build_chart_config_is_a_line_chart() {
        let config = build_chart_config(&sample_results());
        assert_eq!(config["type"], "line");
    }

    #[test]
    fn build_chart_url_produces_a_valid_get_render_url_roundtripping_the_config() {
        let config = build_chart_config(&sample_results());
        let url_str = build_chart_url(&config);

        let url = reqwest::Url::parse(&url_str).expect("build_chart_url must return a valid URL");
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("quickchart.io"));
        assert_eq!(url.path(), "/chart");

        let params: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
        let decoded_config: serde_json::Value =
            serde_json::from_str(&params["c"]).expect("c param must be the chart config JSON");
        assert_eq!(decoded_config, config);
        assert_eq!(params["width"], "800");
        assert_eq!(params["height"], "400");
    }
}
