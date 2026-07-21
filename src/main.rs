//! DebordoLambda - Commande Discord slash pour simulations de débordements

mod config;
mod database;
mod discord;
mod myhordes;

use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::Serialize;
use std::cmp;
use tracing::{error, info};

use crate::config::{SimulationJob, SimulationCitizen};
use crate::discord::{
    interaction_types, response_types, verify_discord_signature, DiscordInteraction,
    DiscordResponse,
};

// ============================================================================
// LAMBDA HANDLER
// ============================================================================

/// Handler principal pour les requêtes Lambda via API Gateway.
async fn handler(
    event: LambdaEvent<ApiGatewayV2httpRequest>,
    sqs_client: aws_sdk_sqs::Client,
    queue_url: String,
    dynamodb_client: aws_sdk_dynamodb::Client,
    ssm_client: aws_sdk_ssm::Client,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let public_key =
        std::env::var("DISCORD_PUBLIC_KEY").expect("DISCORD_PUBLIC_KEY must be set");

    let request = event.payload;
    let body = request.body.unwrap_or_default();

    // Récupérer les headers pour la vérification de signature
    let headers = &request.headers;
    let signature = headers
        .get("x-signature-ed25519")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let timestamp = headers
        .get("x-signature-timestamp")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    // Vérifier la signature Discord (skip en mode test si la variable d'env est définie)
    let skip_signature = std::env::var("SKIP_SIGNATURE_CHECK")
        .map(|v| v == "true")
        .unwrap_or(false);

    if !skip_signature && !verify_discord_signature(&public_key, signature, timestamp, &body) {
        error!("Invalid Discord signature");
        return Ok(build_response(401, "Invalid signature"));
    }

    // Parser l'interaction Discord
    let interaction: DiscordInteraction = match serde_json::from_str(&body) {
        Ok(i) => i,
        Err(e) => {
            error!("Failed to parse interaction: {}", e);
            return Ok(build_response(400, "Invalid request body"));
        }
    };

    // Router selon le type d'interaction
    match interaction.interaction_type {
        interaction_types::PING => handle_ping(),
        interaction_types::APPLICATION_COMMAND => {
            let cmd_name = interaction
                .data
                .as_ref()
                .and_then(|d| d.name.as_deref())
                .unwrap_or("");

            if cmd_name == "register-key" {
                handle_register_key_command()
            } else {
                handle_command(interaction, &sqs_client, &queue_url, &dynamodb_client, &ssm_client).await
            }
        }
        interaction_types::MODAL_SUBMIT => {
            handle_modal_submit(interaction, &sqs_client, &queue_url, &dynamodb_client, &ssm_client).await
        }
        _ => Ok(build_response(400, "Unknown interaction type")),
    }
}

/// Répond au PING de validation Discord.
fn handle_ping() -> Result<ApiGatewayV2httpResponse, Error> {
    info!("Received PING, responding with PONG");
    let response = DiscordResponse {
        response_type: response_types::PONG,
        data: None,
    };
    Ok(build_json_response(200, &response))
}

/// Affiche le formulaire modal pour enregistrer la clé API.
fn handle_register_key_command() -> Result<ApiGatewayV2httpResponse, Error> {
    info!("Handling /register-key command, responding with Modal");
    let response = DiscordResponse {
        response_type: response_types::MODAL,
        data: Some(serde_json::json!({
            "title": "Enregistrer votre clé API",
            "custom_id": "register_key_modal",
            "components": [
                {
                    "type": 1,
                    "components": [
                        {
                            "type": 4,
                            "custom_id": "api_key_input",
                            "label": "Clé API MyHordes",
                            "style": 1,
                            "min_length": 1,
                            "max_length": 100,
                            "placeholder": "Entrez votre clé API...",
                            "required": true
                        }
                    ]
                }
            ]
        })),
    };
    Ok(build_json_response(200, &response))
}

/// Gère la soumission du formulaire modal et stocke la clé chiffrée.
async fn handle_modal_submit(
    interaction: DiscordInteraction,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    dynamodb_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let custom_id = interaction
        .data
        .as_ref()
        .and_then(|d| d.custom_id.clone())
        .unwrap_or_default();

    if custom_id.starts_with("dm:") {
        return handle_debordo_modal_submit(interaction, &custom_id, sqs_client, queue_url).await;
    }

    if custom_id != "register_key_modal" {
        error!("Received unknown modal custom_id: {}", custom_id);
        return Ok(build_response(400, "Unknown modal custom_id"));
    }

    let user_id = match interaction.user_id() {
        Some(uid) => uid,
        None => {
            error!("Could not extract user_id from modal submit");
            let response = DiscordResponse {
                response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                data: Some(serde_json::json!({
                    "content": "Erreur : Impossible de récupérer votre identifiant Discord.",
                    "flags": 64
                })),
            };
            return Ok(build_json_response(200, &response));
        }
    };

    let api_key = match interaction.get_modal_value("api_key_input") {
        Some(k) => k,
        None => {
            error!("Could not extract api_key_input from modal submit");
            let response = DiscordResponse {
                response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                data: Some(serde_json::json!({
                    "content": "Erreur : Le champ de la clé API est vide.",
                    "flags": 64
                })),
            };
            return Ok(build_json_response(200, &response));
        }
    };

    match database::store_user_key(user_id, api_key, dynamodb_client, ssm_client).await {
        Ok(_) => {
            let response = DiscordResponse {
                response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                data: Some(serde_json::json!({
                    "content": "Votre clé API a été enregistrée de manière sécurisée.",
                    "flags": 64
                })),
            };
            Ok(build_json_response(200, &response))
        }
        Err(e) => {
            error!("Failed to store user key: {}", e);
            let response = DiscordResponse {
                response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                data: Some(serde_json::json!({
                    "content": "Erreur lors de l'enregistrement de votre clé API. Veuillez réessayer.",
                    "flags": 64
                })),
            };
            Ok(build_json_response(200, &response))
        }
    }
}

/// Envoie un job de simulation sur SQS et répond immédiatement avec une réponse différée.
async fn handle_command(
    interaction: DiscordInteraction,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    dynamodb_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let token = interaction.token.clone().unwrap_or_default();
    let application_id = interaction.application_id.clone().unwrap_or_default();

    // 1. Parser les options saisies par l'utilisateur
    let mut user_defense: Option<i32> = None;
    let mut user_tdg_min: Option<i32> = None;
    let mut user_tdg_max: Option<i32> = None;
    let mut user_min_def: Option<i32> = None;
    let mut user_nb_drapo: Option<i32> = None;
    let mut user_day: Option<i32> = None;
    let mut user_iterations: Option<i32> = None;
    let mut user_reactor: Option<bool> = None;
    let mut user_nb_hab: Option<i32> = None;
    let mut user_complete: Option<bool> = None;
    let mut user_defenses: Option<String> = None;
    let mut user_home_bonus: Option<i32> = None;

    let options = interaction
        .data
        .as_ref()
        .and_then(|d| d.options.as_ref())
        .cloned()
        .unwrap_or_default();

    for opt in &options {
        match opt.name.as_str() {
            "defense" => user_defense = opt.value.as_i64().map(|v| v as i32),
            "tdg_min" => user_tdg_min = opt.value.as_i64().map(|v| v as i32),
            "tdg_max" => user_tdg_max = opt.value.as_i64().map(|v| v as i32),
            "min_def" => user_min_def = opt.value.as_i64().map(|v| v as i32),
            "nb_drapo" => user_nb_drapo = opt.value.as_i64().map(|v| v as i32),
            "day" => user_day = opt.value.as_i64().map(|v| v as i32),
            "iterations" => user_iterations = opt.value.as_i64().map(|v| v as i32),
            "reactor" => user_reactor = opt.value.as_bool(),
            "nb_hab" => user_nb_hab = opt.value.as_i64().map(|v| v as i32),
            "complete" => user_complete = opt.value.as_bool(),
            "defenses" => user_defenses = opt.value.as_str().map(|s| s.to_string()),
            "home_bonus" => user_home_bonus = opt.value.as_i64().map(|v| v as i32),
            _ => {}
        }
    }

    // 2. Vérifier si on a tous les paramètres requis manuellement
    let has_all_critical = user_defense.is_some()
        && user_tdg_min.is_some()
        && user_tdg_max.is_some()
        && user_min_def.is_some();

    if has_all_critical {
        info!("All critical parameters provided manually. Skipping API call.");
        let day = user_day.unwrap_or(1);
        let defense = user_defense.unwrap();
        let tdg_min = user_tdg_min.unwrap();
        let tdg_max = user_tdg_max.unwrap();
        let reactor = user_reactor.unwrap_or(false);
        let nb_hab = user_nb_hab.unwrap_or(40);
        let min_def = user_min_def.unwrap();
        let nb_drapo = user_nb_drapo.unwrap_or(0);
        let iterations = user_iterations.unwrap_or(10000);

        let citizens = resolve_citizens(
            user_defenses.as_deref(),
            user_home_bonus.unwrap_or(0),
            None,
            nb_hab,
            min_def,
        );

        if user_complete.unwrap_or(false) && user_defenses.is_none() && user_home_bonus.is_none() {
            return respond_with_defenses_modal(
                defense,
                tdg_min,
                tdg_max,
                min_def,
                nb_drapo,
                day,
                iterations,
                reactor,
                nb_hab,
                None,
                None,
                false,
                false,
                false,
                &citizens,
            );
        }

        return enqueue_simulation(
            token,
            application_id,
            sqs_client,
            queue_url,
            defense,
            tdg_min,
            tdg_max,
            min_def,
            nb_drapo,
            day,
            iterations,
            reactor,
            nb_hab,
            None,
            None,
            false,
            false,
            Vec::new(),
            user_complete.unwrap_or(false),
            user_defenses.clone(),
            user_home_bonus.unwrap_or(0),
            citizens,
        )
        .await;
    }

    // 3. Essayer de récupérer la clé de l'utilisateur
    let user_id = interaction.user_id().unwrap_or("");
    let user_key = if !user_id.is_empty() {
        database::get_user_key(user_id, dynamodb_client, ssm_client)
            .await
            .unwrap_or(None)
    } else {
        None
    };

    match user_key {
        None => {
            // Utilisateur non enregistré: valider les paramètres manquants et renvoyer une erreur s'ils n'ont pas de défaut
            if user_defense.is_none() || user_tdg_min.is_none() || user_tdg_max.is_none() || user_min_def.is_none() {
                let error_msg = "Certains paramètres requis sont manquants (defense, tdg_min, tdg_max, min_def) et vous n'avez pas enregistré votre clé API MyHordes. Veuillez utiliser `/register-key` ou fournir tous les paramètres manuellement.";
                let response = DiscordResponse {
                    response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                    data: Some(serde_json::json!({
                        "content": error_msg,
                        "flags": 64 // Ephemeral
                    })),
                };
                return Ok(build_json_response(200, &response));
            }

            // Si tout le reste a des défauts, on lance avec les overrides + valeurs par défaut
            let day = user_day.unwrap_or(1);
            let defense = user_defense.unwrap_or(0);
            let tdg_min = user_tdg_min.unwrap_or(0);
            let tdg_max = user_tdg_max.unwrap_or(0);
            let reactor = user_reactor.unwrap_or(false);
            let nb_hab = user_nb_hab.unwrap_or(40);
            let min_def = user_min_def.unwrap_or(0);
            let nb_drapo = user_nb_drapo.unwrap_or(0);
            let iterations = user_iterations.unwrap_or(10000);

            let citizens = resolve_citizens(
                user_defenses.as_deref(),
                user_home_bonus.unwrap_or(0),
                None,
                nb_hab,
                min_def,
            );

            if user_complete.unwrap_or(false) && user_defenses.is_none() && user_home_bonus.is_none() {
                return respond_with_defenses_modal(
                    defense,
                    tdg_min,
                    tdg_max,
                    min_def,
                    nb_drapo,
                    day,
                    iterations,
                    reactor,
                    nb_hab,
                    None,
                    None,
                    false,
                    false,
                    false,
                    &citizens,
                );
            }

            enqueue_simulation(
                token,
                application_id,
                sqs_client,
                queue_url,
                defense,
                tdg_min,
                tdg_max,
                min_def,
                nb_drapo,
                day,
                iterations,
                reactor,
                nb_hab,
                None,
                None,
                false,
                false,
                Vec::new(),
                user_complete.unwrap_or(false),
                user_defenses.clone(),
                user_home_bonus.unwrap_or(0),
                citizens,
            )
            .await
        }
        Some(key) => {
            // Utilisateur enregistré: appeler l'API de MyHordes
            match myhordes::fetch_mh_data(&key, ssm_client).await {
                Ok(mh_data) => {
                    if let Some(map) = mh_data.map {
                        let api_day = map.days;
                        let api_defense = map
                            .city
                            .as_ref()
                            .and_then(|c| c.defense.as_ref())
                            .map(|d| d.total)
                            .unwrap_or(0);
                        let api_tdg_min = map
                            .city
                            .as_ref()
                            .and_then(|c| c.estimations.as_ref())
                            .map(|e| e.min)
                            .unwrap_or(0);
                        let api_tdg_max = map
                            .city
                            .as_ref()
                            .and_then(|c| c.estimations.as_ref())
                            .map(|e| e.max)
                            .unwrap_or(0);
                        let api_reactor = map
                            .city
                            .as_ref()
                            .map(|c| {
                                c.buildings
                                    .iter()
                                    .any(|b| {
                                        let name = b.name.to_lowercase();
                                        name.contains("réacteur") || name.contains("reactor")
                                    })
                            })
                            .unwrap_or(false);
                        let api_fortifications = map
                            .city
                            .as_ref()
                            .map(|c| {
                                c.buildings
                                    .iter()
                                    .any(|b| b.name.to_lowercase().contains("habitations fortifi"))
                            })
                            .unwrap_or(false);
                        let api_nb_hab = map.citizens.iter().filter(|c| !c.dead).count() as i32;
                        let api_min_def = map
                            .citizens
                            .iter()
                            .filter(|c| !c.dead)
                            .map(|c| c.base_def)
                            .min()
                            .unwrap_or(0);

                        let api_chaos = map.city.as_ref().and_then(|c| c.chaos).unwrap_or(false);
                        let api_devast = map.city.as_ref().and_then(|c| c.devast).unwrap_or(false);

                        // Calculer population et b_level (tercile)
                        let mut b_levels = [0; 30];
                        let mut targets = 0;
                        let mut max_b_level = -1;
                        for citizen in &map.citizens {
                            if !citizen.dead {
                                let base_def = citizen.base_def;
                                let citizen_b_level = match base_def {
                                    0..=1 => 0,
                                    2..=5 => 1,
                                    6..=9 => 2,
                                    10..=13 => 3,
                                    14..=17 => 4,
                                    _ => 5,
                                };
                                max_b_level = cmp::max(max_b_level, citizen_b_level);
                                for l in 0..=citizen_b_level {
                                    if l < 30 {
                                        b_levels[l as usize] += 1;
                                    }
                                }
                                targets += 1;
                            }
                        }
                        let mut b_level = max_b_level;
                        let ceil_target_third = ((targets as f64) / 3.0).ceil() as i32;
                        let mut tercile = -1;
                        for l in (0..30).rev() {
                            if b_levels[l] >= ceil_target_third {
                                tercile = l as i32;
                                break;
                            }
                        }
                        if tercile > 0 {
                            b_level = tercile;
                        }

                        let population = map.citizens.len() as i32;

                        // Fusionner les valeurs: Input > API > Default
                        let day = user_day.unwrap_or(api_day);
                        let defense = user_defense.unwrap_or(api_defense);
                        let tdg_min = user_tdg_min.unwrap_or(api_tdg_min);
                        let tdg_max = user_tdg_max.unwrap_or(api_tdg_max);
                        let reactor = user_reactor.unwrap_or(api_reactor);
                        let nb_hab = user_nb_hab.unwrap_or(api_nb_hab);
                        let min_def = user_min_def.unwrap_or(api_min_def);
                        let nb_drapo = user_nb_drapo.unwrap_or(0);
                        let iterations = user_iterations.unwrap_or(10000);

                        let mut api_pulled_fields = Vec::new();
                        if user_day.is_none() { api_pulled_fields.push("day".to_string()); }
                        if user_defense.is_none() { api_pulled_fields.push("defense".to_string()); }
                        if user_tdg_min.is_none() || user_tdg_max.is_none() { api_pulled_fields.push("tdg".to_string()); }
                        if user_nb_hab.is_none() { api_pulled_fields.push("nb_hab".to_string()); }
                        if user_min_def.is_none() { api_pulled_fields.push("min_def".to_string()); }

                        // Si après la fusion, des paramètres critiques restent à 0, renvoyer une erreur
                        if defense <= 0 || tdg_min <= 0 || tdg_max <= 0 || min_def <= 0 {
                            let error_msg = "Erreur : Impossible de récupérer des données de ville valides via l'API (êtes-vous actuellement en vie dans une ville ?). Veuillez saisir les paramètres requis manuellement.";
                            let response = DiscordResponse {
                                response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                                data: Some(serde_json::json!({
                                    "content": error_msg,
                                    "flags": 64
                                })),
                            };
                            return Ok(build_json_response(200, &response));
                        }

                        let home_bonus = user_home_bonus.unwrap_or(0) + if api_fortifications { 4 } else { 0 };

                        let citizens = resolve_citizens(
                            user_defenses.as_deref(),
                            home_bonus,
                            Some(&map.citizens),
                            nb_hab,
                            min_def,
                        );

                        if user_complete.unwrap_or(false) && user_defenses.is_none() && user_home_bonus.is_none() {
                            return respond_with_defenses_modal(
                                defense,
                                tdg_min,
                                tdg_max,
                                min_def,
                                nb_drapo,
                                day,
                                iterations,
                                reactor,
                                nb_hab,
                                Some(b_level),
                                Some(population),
                                api_chaos,
                                api_devast,
                                true, // is API
                                &citizens,
                            );
                        }

                        enqueue_simulation(
                            token,
                            application_id,
                            sqs_client,
                            queue_url,
                            defense,
                            tdg_min,
                            tdg_max,
                            min_def,
                            nb_drapo,
                            day,
                            iterations,
                            reactor,
                            nb_hab,
                            Some(b_level),
                            Some(population),
                            api_chaos,
                            api_devast,
                            api_pulled_fields,
                            user_complete.unwrap_or(false),
                            user_defenses.clone(),
                            home_bonus,
                            citizens,
                        )
                        .await
                    } else {
                        // Pas de ville active (map est None)
                        let error_msg = "Erreur MyHordes : Vous ne semblez pas être actuellement incarné dans une ville active. Veuillez vous incarner ou saisir les paramètres manuellement.";
                        let response = DiscordResponse {
                            response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                            data: Some(serde_json::json!({
                                "content": error_msg,
                                "flags": 64
                            })),
                        };
                        Ok(build_json_response(200, &response))
                    }
                }
                Err(e) => {
                    error!("MyHordes API call failed: {}", e);

                    // Si l'API échoue, on ne peut continuer que si l'utilisateur a tout fourni manuellement
                    if user_defense.is_none() || user_tdg_min.is_none() || user_tdg_max.is_none() || user_min_def.is_none() {
                        let error_msg = format!(
                            "Erreur de connexion à l'API MyHordes : {}. Veuillez vérifier votre clé avec `/register-key` ou saisir les paramètres requis manuellement.",
                            e
                        );
                        let response = DiscordResponse {
                            response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
                            data: Some(serde_json::json!({
                                "content": error_msg,
                                "flags": 64
                            })),
                        };
                        return Ok(build_json_response(200, &response));
                    }

                    // Fallback sur les paramètres fournis
                    let day = user_day.unwrap_or(1);
                    let defense = user_defense.unwrap_or(0);
                    let tdg_min = user_tdg_min.unwrap_or(0);
                    let tdg_max = user_tdg_max.unwrap_or(0);
                    let reactor = user_reactor.unwrap_or(false);
                    let nb_hab = user_nb_hab.unwrap_or(40);
                    let min_def = user_min_def.unwrap_or(0);
                    let nb_drapo = user_nb_drapo.unwrap_or(0);
                    let iterations = user_iterations.unwrap_or(10000);

                    let citizens = resolve_citizens(
                        user_defenses.as_deref(),
                        user_home_bonus.unwrap_or(0),
                        None,
                        nb_hab,
                        min_def,
                    );

                    if user_complete.unwrap_or(false) && user_defenses.is_none() && user_home_bonus.is_none() {
                        return respond_with_defenses_modal(
                            defense,
                            tdg_min,
                            tdg_max,
                            min_def,
                            nb_drapo,
                            day,
                            iterations,
                            reactor,
                            nb_hab,
                            None,
                            None,
                            false,
                            false,
                            false,
                            &citizens,
                        );
                    }

                    enqueue_simulation(
                        token,
                        application_id,
                        sqs_client,
                        queue_url,
                        defense,
                        tdg_min,
                        tdg_max,
                        min_def,
                        nb_drapo,
                        day,
                        iterations,
                        reactor,
                        nb_hab,
                        None,
                        None,
                        false,
                        false,
                        Vec::new(),
                        user_complete.unwrap_or(false),
                        user_defenses.clone(),
                        user_home_bonus.unwrap_or(0),
                        citizens,
                    )
                    .await
                }
            }
        }
    }
}

/// Helper pour formater et enfiler le job de simulation SQS.
async fn enqueue_simulation(
    token: String,
    application_id: String,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    defense: i32,
    tdg_min: i32,
    tdg_max: i32,
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: i32,
    reactor: bool,
    nb_hab: i32,
    b_level: Option<i32>,
    population: Option<i32>,
    is_chaos: bool,
    is_devastated: bool,
    api_pulled_fields: Vec<String>,
    is_complete: bool,
    custom_defenses: Option<String>,
    home_bonus: i32,
    citizens: Vec<SimulationCitizen>,
) -> Result<ApiGatewayV2httpResponse, Error> {
    use crate::config::CommandOption;

    let mut finalized_options = vec![
        CommandOption {
            name: "defense".to_string(),
            value: serde_json::json!(defense),
        },
        CommandOption {
            name: "tdg_min".to_string(),
            value: serde_json::json!(tdg_min),
        },
        CommandOption {
            name: "tdg_max".to_string(),
            value: serde_json::json!(tdg_max),
        },
        CommandOption {
            name: "min_def".to_string(),
            value: serde_json::json!(min_def),
        },
        CommandOption {
            name: "nb_drapo".to_string(),
            value: serde_json::json!(nb_drapo),
        },
        CommandOption {
            name: "day".to_string(),
            value: serde_json::json!(day),
        },
        CommandOption {
            name: "iterations".to_string(),
            value: serde_json::json!(iterations),
        },
        CommandOption {
            name: "reactor".to_string(),
            value: serde_json::json!(reactor),
        },
        CommandOption {
            name: "nb_hab".to_string(),
            value: serde_json::json!(nb_hab),
        },
        CommandOption {
            name: "is_chaos".to_string(),
            value: serde_json::json!(is_chaos),
        },
        CommandOption {
            name: "is_devastated".to_string(),
            value: serde_json::json!(is_devastated),
        },
        CommandOption {
            name: "complete".to_string(),
            value: serde_json::json!(is_complete),
        },
        CommandOption {
            name: "home_bonus".to_string(),
            value: serde_json::json!(home_bonus),
        },
    ];

    if let Some(ref cd) = custom_defenses {
        finalized_options.push(CommandOption {
            name: "defenses".to_string(),
            value: serde_json::json!(cd),
        });
    }

    if let Some(bl) = b_level {
        finalized_options.push(CommandOption {
            name: "b_level".to_string(),
            value: serde_json::json!(bl),
        });
    }

    if let Some(pop) = population {
        finalized_options.push(CommandOption {
            name: "population".to_string(),
            value: serde_json::json!(pop),
        });
    }

    let job = SimulationJob {
        token,
        application_id,
        options: finalized_options,
        api_pulled_fields,
        citizens,
    };
    let job_json = serde_json::to_string(&job)?;

    sqs_client
        .send_message()
        .queue_url(queue_url)
        .message_body(job_json)
        .send()
        .await?;

    info!("Simulation job enqueued to SQS");

    let response = DiscordResponse {
        response_type: response_types::DEFERRED_CHANNEL_MESSAGE_WITH_SOURCE,
        data: None,
    };
    Ok(build_json_response(200, &response))
}

fn parse_custom_defenses(defenses_str: &str) -> std::collections::HashMap<String, i32> {
    let mut map = std::collections::HashMap::new();
    for part in defenses_str.split(|c| c == ',' || c == '\n' || c == '\r') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(pos) = part.rfind(':') {
            let name = part[..pos].trim().to_lowercase();
            let def_str = part[pos+1..].trim();
            if let Ok(def) = def_str.parse::<i32>() {
                map.insert(name, def);
            }
        }
    }
    map
}

fn resolve_citizens(
    custom_defenses_str: Option<&str>,
    home_bonus: i32,
    api_citizens: Option<&[crate::myhordes::MHCitizen]>,
    nb_hab: i32,
    min_def: i32,
) -> Vec<SimulationCitizen> {
    let mut citizens = Vec::new();
    let custom_map = custom_defenses_str
        .map(parse_custom_defenses)
        .unwrap_or_default();

    if let Some(api_list) = api_citizens {
        // Mode API: Iterate through alive API citizens
        for citizen in api_list {
            if !citizen.dead {
                let name_lower = citizen.name.to_lowercase();
                let defense = if let Some(&custom_def) = custom_map.get(&name_lower) {
                    custom_def
                } else {
                    let job_id = citizen.job.as_ref().map(|j| j.id).unwrap_or(0);
                    let job_bonus = match job_id {
                        0 => 0, // Resident
                        3 => 3, // Guardian
                        _ => 2, // Other Hero
                    };
                    citizen.base_def + home_bonus + job_bonus
                };
                citizens.push(SimulationCitizen {
                    name: citizen.name.clone(),
                    defense,
                });
            }
        }
    } else {
        // Mode Manuel: Build based on custom map first, then fill remainder
        let mut added_names = std::collections::HashSet::new();
        
        // 1. Add explicitly nominated citizens from defenses string
        if let Some(s) = custom_defenses_str {
            for part in s.split(|c| c == ',' || c == '\n' || c == '\r') {
                let part = part.trim();
                if part.is_empty() {
                    continue;
                }
                if let Some(pos) = part.rfind(':') {
                    let name = part[..pos].trim();
                    let name_lower = name.to_lowercase();
                    if added_names.contains(&name_lower) {
                        continue;
                    }
                    let def_str = part[pos+1..].trim();
                    if let Ok(def) = def_str.parse::<i32>() {
                        citizens.push(SimulationCitizen {
                            name: name.to_string(),
                            defense: def,
                        });
                        added_names.insert(name_lower);
                    }
                }
            }
        }

        // 2. Fill the remainder up to nb_hab
        let mut count = 1;
        while citizens.len() < nb_hab as usize {
            let gen_name = format!("Citoyen {}", count);
            let gen_name_lower = gen_name.to_lowercase();
            if !added_names.contains(&gen_name_lower) {
                citizens.push(SimulationCitizen {
                    name: gen_name,
                    defense: min_def + home_bonus,
                });
            }
            count += 1;
        }

        citizens.truncate(nb_hab as usize);
    }

    citizens
}

// ============================================================================
// RESPONSE BUILDERS
// ============================================================================

/// Construit une réponse HTTP simple avec du texte.
fn build_response(status_code: i64, body: &str) -> ApiGatewayV2httpResponse {
    let mut r = ApiGatewayV2httpResponse::default();
    r.status_code = status_code;
    r.body = Some(aws_lambda_events::encodings::Body::Text(body.to_string()));
    r
}

/// Construit une réponse HTTP JSON.
fn build_json_response<T: Serialize>(status_code: i64, body: &T) -> ApiGatewayV2httpResponse {
    let json_body = serde_json::to_string(body).unwrap_or_default();
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());

    let mut r = ApiGatewayV2httpResponse::default();
    r.status_code = status_code;
    r.headers = headers;
    r.body = Some(aws_lambda_events::encodings::Body::Text(json_body));
    r
}

fn parse_complete_modal_text(text: &str) -> (
    i32, // defense
    i32, // tdg_min
    i32, // tdg_max
    i32, // min_def
    i32, // day
    i32, // iterations
    bool, // reactor
    i32, // nb_hab
    Option<i32>, // b_level
    Option<i32>, // population
    bool, // is_chaos
    bool, // is_devastated
    Vec<SimulationCitizen>,
) {
    let mut defense = 0;
    let mut tdg_min = 0;
    let mut tdg_max = 0;
    let mut min_def = 0;
    let mut day = 1;
    let mut iterations = 10000;
    let mut reactor = false;
    let mut nb_hab = 40;
    let mut b_level = None;
    let mut population = None;
    let mut is_chaos = false;
    let mut is_devastated = false;
    let mut citizens = Vec::new();

    for line in text.split(|c| c == '\n' || c == '\r') {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }

        if let Some(pos) = line.find(':') {
            let key = line[..pos].trim().to_lowercase();
            let val_str = line[pos + 1..].trim();

            match key.as_str() {
                "defense" | "défense" => {
                    if let Ok(v) = val_str.parse::<i32>() { defense = v; }
                }
                "tdg" | "estimations" => {
                    if let Some(dash_pos) = val_str.find('-') {
                        if let Ok(mn) = val_str[..dash_pos].trim().parse::<i32>() { tdg_min = mn; }
                        if let Ok(mx) = val_str[dash_pos + 1..].trim().parse::<i32>() { tdg_max = mx; }
                    } else if let Ok(v) = val_str.parse::<i32>() {
                        tdg_min = v;
                        tdg_max = v;
                    }
                }
                "tdg_min" => {
                    if let Ok(v) = val_str.parse::<i32>() { tdg_min = v; }
                }
                "tdg_max" => {
                    if let Ok(v) = val_str.parse::<i32>() { tdg_max = v; }
                }
                "min_def" | "defense_minimale" | "défense_minimale" => {
                    if let Ok(v) = val_str.parse::<i32>() { min_def = v; }
                }
                "day" | "jour" => {
                    if let Ok(v) = val_str.parse::<i32>() { day = v; }
                }
                "iterations" | "itérations" => {
                    if let Ok(v) = val_str.parse::<i32>() { iterations = v; }
                }
                "reactor" | "réacteur" => {
                    let lower = val_str.to_lowercase();
                    reactor = lower == "true" || lower == "1" || lower == "oui" || lower == "yes" || lower == "y";
                }
                "nb_hab" | "citoyens_max" => {
                    if let Ok(v) = val_str.parse::<i32>() { nb_hab = v; }
                }
                "b_level" | "tercile" => {
                    if val_str.to_lowercase() != "none" && val_str.to_lowercase() != "n" {
                        if let Ok(v) = val_str.parse::<i32>() { b_level = Some(v); }
                    }
                }
                "population" => {
                    if val_str.to_lowercase() != "none" && val_str.to_lowercase() != "n" {
                        if let Ok(v) = val_str.parse::<i32>() { population = Some(v); }
                    }
                }
                "chaos" => {
                    let lower = val_str.to_lowercase();
                    is_chaos = lower == "true" || lower == "1" || lower == "oui" || lower == "yes" || lower == "y";
                }
                "devastated" | "dévastée" | "devast" => {
                    let lower = val_str.to_lowercase();
                    is_devastated = lower == "true" || lower == "1" || lower == "oui" || lower == "yes" || lower == "y";
                }
                "nb_drapo" => {} // Handled separately to avoid citizen parsing
                _ => {
                    if let Ok(def) = val_str.parse::<i32>() {
                        let name = line[..pos].trim().to_string();
                        citizens.push(SimulationCitizen { name, defense: def });
                    }
                }
            }
        }
    }

    (
        defense,
        tdg_min,
        tdg_max,
        min_def,
        day,
        iterations,
        reactor,
        nb_hab,
        b_level,
        population,
        is_chaos,
        is_devastated,
        citizens,
    )
}

fn respond_with_defenses_modal(
    defense: i32,
    tdg_min: i32,
    tdg_max: i32,
    min_def: i32,
    nb_drapo: i32,
    day: i32,
    iterations: i32,
    reactor: bool,
    nb_hab: i32,
    b_level: Option<i32>,
    population: Option<i32>,
    is_chaos: bool,
    is_devastated: bool,
    is_api: bool,
    citizens: &[SimulationCitizen],
) -> Result<ApiGatewayV2httpResponse, Error> {
    info!("Responding with defenses edit modal");

    let custom_id = if is_api { "dm:api" } else { "dm:manual" };

    let b_level_str = b_level.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string());
    let pop_str = population.map(|v| v.to_string()).unwrap_or_else(|| "none".to_string());

    let mut citizens_sorted = citizens.to_vec();
    citizens_sorted.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    let mut config_lines = vec![
        format!("defense: {}", defense),
        format!("tdg: {}-{}", tdg_min, tdg_max),
        format!("min_def: {}", min_def),
        format!("nb_drapo: {}", nb_drapo),
        format!("day: {}", day),
        format!("iterations: {}", iterations),
        format!("reactor: {}", reactor),
        format!("nb_hab: {}", nb_hab),
        format!("b_level: {}", b_level_str),
        format!("population: {}", pop_str),
        format!("chaos: {}", is_chaos),
        format!("devastated: {}", is_devastated),
        "---".to_string(),
    ];

    for c in &citizens_sorted {
        config_lines.push(format!("{}: {}", c.name, c.defense));
    }

    let citizens_str = config_lines.join("\n");

    let response = DiscordResponse {
        response_type: response_types::MODAL,
        data: Some(serde_json::json!({
            "title": "Configuration & Défenses",
            "custom_id": custom_id,
            "components": [
                {
                    "type": 1, // ACTION_ROW
                    "components": [
                        {
                            "type": 4, // TEXT_INPUT
                            "custom_id": "defenses_input",
                            "label": "Configuration et défenses",
                            "style": 2, // PARAGRAPH
                            "min_length": 1,
                            "max_length": 4000,
                            "value": citizens_str,
                            "required": true
                        }
                    ]
                }
            ]
        })),
    };

    Ok(build_json_response(200, &response))
}

async fn handle_debordo_modal_submit(
    interaction: DiscordInteraction,
    custom_id: &str,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
) -> Result<ApiGatewayV2httpResponse, Error> {
    info!("Handling debordo configuration modal submission");

    let is_api = custom_id == "dm:api";

    let token = interaction.token.clone().unwrap_or_default();
    let application_id = interaction.application_id.clone().unwrap_or_default();

    let defenses_val = interaction.get_modal_value("defenses_input").unwrap_or("");
    
    let (
        defense,
        tdg_min,
        tdg_max,
        min_def,
        day,
        iterations,
        reactor,
        nb_hab,
        b_level,
        population,
        is_chaos,
        is_devastated,
        citizens,
    ) = parse_complete_modal_text(defenses_val);

    let mut nb_drapo = 0;
    for line in defenses_val.split(|c| c == '\n' || c == '\r') {
        let line = line.trim();
        if let Some(pos) = line.find(':') {
            let key = line[..pos].trim().to_lowercase();
            if key == "nb_drapo" {
                if let Ok(v) = line[pos + 1..].trim().parse::<i32>() {
                    nb_drapo = v;
                }
            }
        }
    }

    let mut api_pulled_fields = Vec::new();
    if is_api {
        api_pulled_fields.push("defense".to_string());
        api_pulled_fields.push("tdg".to_string());
        api_pulled_fields.push("nb_hab".to_string());
        api_pulled_fields.push("min_def".to_string());
    }

    enqueue_simulation(
        token,
        application_id,
        sqs_client,
        queue_url,
        defense,
        tdg_min,
        tdg_max,
        min_def,
        nb_drapo,
        day,
        iterations,
        reactor,
        nb_hab,
        b_level,
        population,
        is_chaos,
        is_devastated,
        api_pulled_fields,
        true, // complete
        Some(defenses_val.to_string()),
        0, // home_bonus
        citizens,
    )
    .await
}

// ============================================================================
// ENTRYPOINT
// ============================================================================

#[tokio::main]
async fn main() -> Result<(), Error> {
    // Initialiser le logging structuré pour CloudWatch
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let aws_config = aws_config::load_from_env().await;
    let sqs_client = aws_sdk_sqs::Client::new(&aws_config);
    let dynamodb_client = aws_sdk_dynamodb::Client::new(&aws_config);
    let ssm_client = aws_sdk_ssm::Client::new(&aws_config);
    let queue_url = std::env::var("SQS_QUEUE_URL").expect("SQS_QUEUE_URL must be set");

    info!("Starting DebordoLambda Discord handler");

    lambda_runtime::run(service_fn(move |event| {
        let client = sqs_client.clone();
        let url = queue_url.clone();
        let db = dynamodb_client.clone();
        let ssm = ssm_client.clone();
        async move { handler(event, client, url, db, ssm).await }
    }))
    .await
}
