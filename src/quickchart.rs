use reparo_lib::Statistics;
use serde::Deserialize;

/// Nombre maximal de points par série accepté par l'offre gratuite de QuickChart : au-delà, le
/// rendu échoue en 400 ("maximum chart data exceeded"), quel que soit le nombre de séries.
/// Source : https://community.quickchart.io/t/maximum-chart-data-exceeded/727
const MAX_CHART_POINTS: usize = 250;

/// Sélectionne au plus `max_points` éléments régulièrement espacés, en conservant toujours le
/// premier et le dernier (les bornes de la TDG).
fn downsample<T>(items: &[T], max_points: usize) -> Vec<&T> {
    if items.len() <= max_points || max_points < 2 {
        return items.iter().collect();
    }
    let last = items.len() - 1;
    (0..max_points)
        .map(|i| &items[i * last / (max_points - 1)])
        .collect()
}

fn round1(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Une seule valeur d'attaque (tdg_min == tdg_max) : une ligne a besoin de deux points, donc
/// avec `pointRadius: 0` rien n'est dessiné. On affiche à la place des marqueurs centrés façon
/// boîte à moustaches : traits pour min/max et Q1/Q3, point pour la moyenne.
fn apply_single_point_style(config: &mut serde_json::Value) {
    let markers = [
        ("Q1", "line", 40, "rgb(54, 162, 235)"),
        ("Q3", "line", 40, "rgb(54, 162, 235)"),
        ("Moyenne", "circle", 6, "rgb(54, 162, 235)"),
        ("Min", "line", 25, "rgba(120, 120, 120, 0.6)"),
        ("Max", "line", 25, "rgba(120, 120, 120, 0.6)"),
    ];
    if let Some(datasets) = config["data"]["datasets"].as_array_mut() {
        for (dataset, (label, style, radius, color)) in datasets.iter_mut().zip(markers) {
            dataset["label"] = label.into();
            dataset["showLine"] = false.into();
            dataset["fill"] = false.into();
            dataset["pointStyle"] = style.into();
            dataset["pointRadius"] = radius.into();
            dataset["borderWidth"] = 2.into();
            dataset["borderColor"] = color.into();
            dataset["backgroundColor"] = color.into();
            if let Some(d) = dataset.as_object_mut() {
                d.remove("borderDash");
            }
        }
    }
    config["options"]["scales"]["xAxes"][0]["offset"] = true.into();
}

pub fn build_chart_config(results: &[(i32, Statistics)]) -> serde_json::Value {
    let points = downsample(results, MAX_CHART_POINTS);
    let single_point = points.len() == 1;
    let labels: Vec<i32> = points.iter().map(|(attack, _)| *attack).collect();
    let q1: Vec<f64> = points.iter().map(|(_, s)| round1(s.q1)).collect();
    let q3: Vec<f64> = points.iter().map(|(_, s)| round1(s.q3)).collect();
    let mean: Vec<f64> = points.iter().map(|(_, s)| round1(s.mean)).collect();
    let min: Vec<i32> = points.iter().map(|(_, s)| s.min).collect();
    let max: Vec<i32> = points.iter().map(|(_, s)| s.max).collect();

    let mut config = serde_json::json!({
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
    });

    if single_point {
        apply_single_point_style(&mut config);
    }
    config
}

#[derive(Deserialize)]
struct QuickChartCreateResponse {
    success: bool,
    #[serde(default)]
    url: Option<String>,
}

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

    let parsed: QuickChartCreateResponse = resp.json().await.map_err(|e| {
        lambda_runtime::Error::from(format!("Failed to parse QuickChart response: {}", e))
    })?;

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

    #[test]
    fn build_chart_config_single_attack_value_uses_visible_markers() {
        let config = build_chart_config(&sample_results()[..1]);

        let datasets = config["data"]["datasets"].as_array().unwrap();
        let labels: Vec<&str> = datasets.iter().map(|d| d["label"].as_str().unwrap()).collect();
        assert_eq!(labels, ["Q1", "Q3", "Moyenne", "Min", "Max"]);
        for dataset in datasets {
            assert_eq!(dataset["showLine"], false);
            assert!(dataset["pointRadius"].as_i64().unwrap() > 0);
            assert_ne!(dataset["borderColor"], "transparent");
        }
        assert_eq!(config["options"]["scales"]["xAxes"][0]["offset"], true);
    }

    #[test]
    fn build_chart_config_multiple_attack_values_keep_line_style() {
        let config = build_chart_config(&sample_results());
        for dataset in config["data"]["datasets"].as_array().unwrap() {
            assert_eq!(dataset["pointRadius"], 0);
            assert!(dataset.get("showLine").is_none());
        }
        assert!(config["options"]["scales"]["xAxes"][0].get("offset").is_none());
    }

    #[test]
    fn downsample_keeps_small_inputs_untouched() {
        let items: Vec<i32> = (0..10).collect();
        let sampled: Vec<i32> = downsample(&items, 100).into_iter().copied().collect();
        assert_eq!(sampled, items);
    }

    #[test]
    fn downsample_caps_points_and_keeps_bounds() {
        let items: Vec<i32> = (2275..=2514).collect();
        let sampled: Vec<i32> = downsample(&items, 100).into_iter().copied().collect();
        assert_eq!(sampled.len(), 100);
        assert_eq!(sampled.first(), Some(&2275));
        assert_eq!(sampled.last(), Some(&2514));
        assert!(sampled.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn build_chart_config_caps_points_for_wide_tdg_range() {
        let stats = sample_results()[0].1;
        let results: Vec<(i32, Statistics)> =
            (1000..=3000).map(|attack| (attack, stats)).collect();
        let config = build_chart_config(&results);

        let labels = config["data"]["labels"].as_array().unwrap();
        assert_eq!(labels.len(), MAX_CHART_POINTS);
        assert_eq!(labels[0], 1000);
        assert_eq!(labels[MAX_CHART_POINTS - 1], 3000);
        for dataset in config["data"]["datasets"].as_array().unwrap() {
            assert_eq!(dataset["data"].as_array().unwrap().len(), MAX_CHART_POINTS);
        }
    }

    #[test]
    fn build_chart_config_rounds_float_series() {
        let mut stats = sample_results()[0].1;
        stats.mean = 12.345_678;
        let config = build_chart_config(&[(100, stats)]);
        assert_eq!(config["data"]["datasets"][2]["data"][0], 12.3);
    }
}
