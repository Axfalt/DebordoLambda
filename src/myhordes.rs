//! Client module for MyHordes External JSON API.

use serde::Deserialize;
use tracing::{info, error};

#[derive(Deserialize, Debug, Clone)]
pub struct MHMeResponse {
    pub map: Option<MHMap>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHMap {
    pub days: i32,
    pub city: Option<MHCity>,
    pub citizens: Vec<MHCitizen>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHCity {
    pub defense: Option<MHDefense>,
    pub buildings: Vec<MHBuilding>,
    pub estimations: Option<MHEstimation>,
    pub chaos: Option<bool>,
    pub devast: Option<bool>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHDefense {
    pub total: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHBuilding {
    pub name: String,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHEstimation {
    pub min: i32,
    pub max: i32,
}

#[derive(Deserialize, Debug, Clone)]
pub struct MHCitizen {
    pub name: String,
    pub dead: bool,
    #[serde(rename = "baseDef")]
    pub base_def: i32,
}

/// Fetches current user map and city details from MyHordes JSON API.
pub async fn fetch_mh_data(
    user_key: &str,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<MHMeResponse, lambda_runtime::Error> {
    let param_name = std::env::var("SSM_APP_KEY_PARAMETER").unwrap_or_else(|_| "MH_APP_KEY".to_string());
    
    let app_key = match ssm_client
        .get_parameter()
        .name(&param_name)
        .with_decryption(true)
        .send()
        .await 
    {
        Ok(res) => res.parameter.and_then(|p| p.value).unwrap_or_else(|| {
            std::env::var("MH_APP_KEY").unwrap_or_else(|_| "fefe0000fefe0000fefe0000fefe0000".to_string())
        }),
        Err(e) => {
            info!("SSM lookup for app key parameter '{}' failed ({}). Falling back to environment variables.", param_name, e);
            std::env::var("MH_APP_KEY").unwrap_or_else(|_| "fefe0000fefe0000fefe0000fefe0000".to_string())
        }
    };

    let fields_param = "map.fields(days,city.fields(chaos,devast,defense.fields(total),buildings.fields(name),estimations.fields(min,max)),citizens.fields(name,dead,baseDef))";

    let url = "https://myhordes.eu/api/x/json/me";
    info!("Querying MyHordes API for me/map details...");

    let client = reqwest::Client::new();
    let resp = client
        .get(url)
        .query(&[
            ("userkey", user_key),
            ("appkey", &app_key),
            ("languages", "fr"),
            ("fields", fields_param),
        ])
        .send()
        .await
        .map_err(|e| {
            error!("Failed to connect to MyHordes API: {}", e);
            lambda_runtime::Error::from(format!("Failed to connect to MyHordes API: {}", e))
        })?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_body = resp.text().await.unwrap_or_default();
        error!("MyHordes API error: status={}, body={}", status, error_body);
        return Err(lambda_runtime::Error::from(format!(
            "MyHordes API returned status {}: {}",
            status, error_body
        )));
    }

    let data: MHMeResponse = resp
        .json()
        .await
        .map_err(|e| {
            error!("Failed to parse MyHordes response JSON: {}", e);
            lambda_runtime::Error::from(format!("Failed to parse MyHordes response JSON: {}", e))
        })?;

    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_myhordes_response() {
        let json_data = serde_json::json!({
            "map": {
                "days": 4,
                "city": {
                  "defense": { "total": 125 },
                  "buildings": [
                    { "name": "Réacteur chimique" },
                    { "name": "Fortifications de fortune" },
                    { "name": "Wassergraben" }
                  ],
                  "estimations": { "min": 250, "max": 400 }
                },
                "citizens": [
                  { "name": "Axfalt", "dead": false, "baseDef": 12 },
                  { "name": "Bob", "dead": true, "baseDef": 8 },
                  { "name": "Charlie", "dead": false, "baseDef": 15 }
                ]
            }
        });

        let response: MHMeResponse = serde_json::from_value(json_data).unwrap();
        assert!(response.map.is_some());
        
        let map = response.map.unwrap();
        assert_eq!(map.days, 4);

        let city = map.city.unwrap();
        assert_eq!(city.defense.unwrap().total, 125);

        // Test reactor check
        let has_reactor = city.buildings.iter().any(|b| {
            let name = b.name.to_lowercase();
            name.contains("réacteur") || name.contains("reactor")
        });
        assert!(has_reactor);

        // Test fortifications check
        let has_fortifications = city.buildings.iter().any(|b| b.name.to_lowercase().contains("fortification"));
        assert!(has_fortifications);

        let estimations = city.estimations.unwrap();
        assert_eq!(estimations.min, 250);
        assert_eq!(estimations.max, 400);

        // Test alive count
        let nb_hab = map.citizens.iter().filter(|c| !c.dead).count();
        assert_eq!(nb_hab, 2);

        // Test min_def among alive citizens
        let min_def = map.citizens.iter()
            .filter(|c| !c.dead)
            .map(|c| c.base_def)
            .min()
            .unwrap_or(0);
        assert_eq!(min_def, 12);
    }
}

