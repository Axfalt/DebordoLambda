//! Génération d'un graphique QuickChart.io pour visualiser les statistiques de réparation.

use reparo_lib::Statistics;
use serde::Deserialize;

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
                "text": "Dégâts de réparation estimés (PV) selon la force de l'attaque"
            },
            "scales": {
                "xAxes": [{ "scaleLabel": { "display": true, "labelString": "Attaque (TDG)" } }],
                "yAxes": [{ "scaleLabel": { "display": true, "labelString": "PV à réparer" } }]
            }
        }
    })
}

#[derive(Deserialize)]
struct QuickChartCreateResponse {
    success: bool,
    #[serde(default)]
    url: Option<String>,
}

/// Crée un graphique hébergé via l'API QuickChart.io et retourne son URL.
pub async fn create_chart_url(
    http_client: &reqwest::Client,
    chart_config: &serde_json::Value,
) -> Result<String, lambda_runtime::Error> {
    let body = serde_json::json!({
        "chart": chart_config,
        "width": 800,
        "height": 400,
        "backgroundColor": "white"
    });

    let resp = http_client
        .post("https://quickchart.io/chart/create")
        .json(&body)
        .send()
        .await
        .map_err(|e| lambda_runtime::Error::from(format!("Failed to reach QuickChart: {}", e)))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_body = resp.text().await.unwrap_or_default();
        return Err(lambda_runtime::Error::from(format!(
            "QuickChart returned status {}: {}",
            status, error_body
        )));
    }

    let parsed: QuickChartCreateResponse = resp
        .json()
        .await
        .map_err(|e| lambda_runtime::Error::from(format!("Failed to parse QuickChart response: {}", e)))?;

    if !parsed.success {
        return Err(lambda_runtime::Error::from(
            "QuickChart reported failure creating the chart",
        ));
    }

    parsed
        .url
        .ok_or_else(|| lambda_runtime::Error::from("QuickChart response missing url"))
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
}
