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

use crate::config::SimulationJob;
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
            handle_modal_submit(interaction, &dynamodb_client, &ssm_client).await
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
    dynamodb_client: &aws_sdk_dynamodb::Client,
    ssm_client: &aws_sdk_ssm::Client,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let custom_id = interaction
        .data
        .as_ref()
        .and_then(|d| d.custom_id.as_deref())
        .unwrap_or("");

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
                                    .any(|b| b.name.to_lowercase().contains("réacteur"))
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
                        let mut b_levels = vec![0; 30];
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
    ];

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
