//! DebordoLambda - Commande Discord slash pour simulations de débordements

use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use serde::Serialize;
use std::cmp;
use tracing::{error, info};

use debordo_lib::config::{SimConfig, SimulationCitizen, SimulationJob};
use debordo_lib::discord::{
    DiscordInteraction, DiscordResponse, interaction_types, response_types,
    verify_discord_signature,
};
use debordo_lib::{database, myhordes};

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
    http_client: &reqwest::Client,
    public_key: &str,
) -> Result<ApiGatewayV2httpResponse, Error> {
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

    if !skip_signature && !verify_discord_signature(public_key, signature, timestamp, &body) {
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
                handle_command(
                    interaction,
                    &sqs_client,
                    &queue_url,
                    &dynamodb_client,
                    &ssm_client,
                    http_client,
                )
                .await
            }
        }
        interaction_types::MODAL_SUBMIT => {
            handle_modal_submit(
                interaction,
                &sqs_client,
                &queue_url,
                &dynamodb_client,
                &ssm_client,
            )
            .await
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
    http_client: &reqwest::Client,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let token = interaction.token.clone().unwrap_or_default();
    let application_id = interaction.application_id.clone().unwrap_or_default();
    let command_name = interaction
        .data
        .as_ref()
        .and_then(|d| d.name.as_deref())
        .unwrap_or("debordo");
    let is_complete_cmd = command_name == "debordo-complete" || command_name == "debordo_complete";

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
    let mut user_interactive: Option<bool> = None;
    let mut user_defenses: Option<String> = None;
    let mut user_home_bonus: Option<i32> = None;

    let options = interaction.data.as_ref().and_then(|d| d.options.as_ref());

    if let Some(opts) = options {
        for opt in opts {
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
                "interactive" => user_interactive = opt.value.as_bool(),
                "defenses" => user_defenses = opt.value.as_str().map(|s| s.to_string()),
                "home_bonus" => user_home_bonus = opt.value.as_i64().map(|v| v as i32),
                _ => {}
            }
        }
    }

    let is_complete = is_complete_cmd || user_complete.unwrap_or(false);
    let is_interactive = is_complete_cmd || user_interactive.unwrap_or(false);

    // 2. Vérifier si on a tous les paramètres requis manuellement
    let has_all_critical = user_defense.is_some()
        && user_tdg_min.is_some()
        && user_tdg_max.is_some()
        && user_min_def.is_some();

    if has_all_critical {
        info!("All critical parameters provided manually. Skipping API call.");
        let config = SimConfig {
            defense: user_defense.unwrap(),
            tdg_min: user_tdg_min.unwrap(),
            tdg_max: user_tdg_max.unwrap(),
            min_def: user_min_def.unwrap(),
            nb_drapo: user_nb_drapo.unwrap_or(0),
            day: user_day.unwrap_or(1),
            iterations: user_iterations.unwrap_or(10000) as u32,
            is_reactor_built: user_reactor.unwrap_or(false),
            nb_hab: user_nb_hab.unwrap_or(40),
            is_complete,
            is_interactive,
            custom_defenses: user_defenses.clone(),
            home_bonus: user_home_bonus.unwrap_or(0),
            ..Default::default()
        };

        let citizens = resolve_citizens(
            user_defenses.as_deref(),
            config.home_bonus,
            None,
            config.nb_hab,
            config.min_def,
        );

        let job = SimulationJob {
            token,
            application_id,
            config,
            citizens,
        };

        return finalize_and_dispatch(job, sqs_client, queue_url, false).await;
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
            if user_defense.is_none()
                || user_tdg_min.is_none()
                || user_tdg_max.is_none()
                || user_min_def.is_none()
            {
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

            let config = SimConfig {
                defense: user_defense.unwrap_or(0),
                tdg_min: user_tdg_min.unwrap_or(0),
                tdg_max: user_tdg_max.unwrap_or(0),
                min_def: user_min_def.unwrap_or(0),
                nb_drapo: user_nb_drapo.unwrap_or(0),
                day: user_day.unwrap_or(1),
                iterations: user_iterations.unwrap_or(10000) as u32,
                is_reactor_built: user_reactor.unwrap_or(false),
                nb_hab: user_nb_hab.unwrap_or(40),
                is_complete,
                is_interactive,
                custom_defenses: user_defenses.clone(),
                home_bonus: user_home_bonus.unwrap_or(0),
                ..Default::default()
            };

            let citizens = resolve_citizens(
                user_defenses.as_deref(),
                config.home_bonus,
                None,
                config.nb_hab,
                config.min_def,
            );

            let job = SimulationJob {
                token,
                application_id,
                config,
                citizens,
            };

            finalize_and_dispatch(job, sqs_client, queue_url, false).await
        }
        Some(key) => {
            // Utilisateur enregistré: appeler l'API de MyHordes
            match myhordes::fetch_mh_data(&key, ssm_client, http_client).await {
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
                                c.buildings.iter().any(|b| {
                                    let name = b.name.to_lowercase();
                                    name.contains("réacteur") || name.contains("reactor")
                                })
                            })
                            .unwrap_or(false);
                        let api_fortifications = map
                            .city
                            .as_ref()
                            .map(|c| {
                                c.buildings.iter().any(|b| {
                                    let name = b.name.to_lowercase();
                                    name == "habitations fortifiées"
                                        || name == "habitations fortifiees"
                                })
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
                                let citizen_b_level =
                                    debordo_lib::simulation::citizen_home_level(base_def);
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
                        let iterations = user_iterations.unwrap_or(10000) as u32;

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

                        let home_bonus =
                            user_home_bonus.unwrap_or(0) + if api_fortifications { 4 } else { 0 };

                        let citizens = resolve_citizens(
                            user_defenses.as_deref(),
                            home_bonus,
                            Some(&map.citizens),
                            nb_hab,
                            min_def,
                        );

                        let config = SimConfig {
                            defense,
                            tdg_min,
                            tdg_max,
                            min_def,
                            nb_drapo,
                            day,
                            iterations,
                            is_reactor_built: reactor,
                            nb_hab,
                            b_level: Some(b_level),
                            population: Some(population),
                            is_chaos: api_chaos,
                            is_devastated: api_devast,
                            is_complete,
                            is_interactive,
                            custom_defenses: user_defenses.clone(),
                            home_bonus,
                        };

                        let job = SimulationJob {
                            token,
                            application_id,
                            config,
                            citizens,
                        };

                        finalize_and_dispatch(job, sqs_client, queue_url, true).await
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
                    if user_defense.is_none()
                        || user_tdg_min.is_none()
                        || user_tdg_max.is_none()
                        || user_min_def.is_none()
                    {
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

                    let config = SimConfig {
                        defense: user_defense.unwrap_or(0),
                        tdg_min: user_tdg_min.unwrap_or(0),
                        tdg_max: user_tdg_max.unwrap_or(0),
                        min_def: user_min_def.unwrap_or(0),
                        nb_drapo: user_nb_drapo.unwrap_or(0),
                        day: user_day.unwrap_or(1),
                        iterations: user_iterations.unwrap_or(10000) as u32,
                        is_reactor_built: user_reactor.unwrap_or(false),
                        nb_hab: user_nb_hab.unwrap_or(40),
                        is_complete,
                        is_interactive,
                        custom_defenses: user_defenses.clone(),
                        home_bonus: user_home_bonus.unwrap_or(0),
                        ..Default::default()
                    };

                    let citizens = resolve_citizens(
                        user_defenses.as_deref(),
                        config.home_bonus,
                        None,
                        config.nb_hab,
                        config.min_def,
                    );

                    let job = SimulationJob {
                        token,
                        application_id,
                        config,
                        citizens,
                    };

                    finalize_and_dispatch(job, sqs_client, queue_url, false).await
                }
            }
        }
    }
}

/// Dispatcher helper
async fn finalize_and_dispatch(
    job: SimulationJob,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
    is_api: bool,
) -> Result<ApiGatewayV2httpResponse, Error> {
    if job.config.is_interactive || (job.config.is_complete && job.config.custom_defenses.is_none())
    {
        return respond_with_defenses_modal(&job.config, is_api, &job.citizens);
    }

    enqueue_simulation(&job, sqs_client, queue_url).await
}

/// Helper pour formater et enfiler le job de simulation SQS.
async fn enqueue_simulation(
    job: &SimulationJob,
    sqs_client: &aws_sdk_sqs::Client,
    queue_url: &str,
) -> Result<ApiGatewayV2httpResponse, Error> {
    let job_json = serde_json::to_string(job)?;

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
    for part in defenses_str.split([',', '\n', '\r']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some(pos) = part.rfind(':') {
            let name = part[..pos].trim().to_lowercase();
            let def_str = part[pos + 1..].trim();
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
    api_citizens: Option<&[myhordes::MHCitizen]>,
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
                    custom_def + home_bonus
                } else {
                    let job_uid = citizen
                        .job
                        .as_ref()
                        .map(|j| j.uid.to_lowercase())
                        .unwrap_or_default();

                    let is_guardian = job_uid == "shield";
                    let is_resident = job_uid == "basic";

                    let job_bonus = if is_guardian {
                        3 // Guardian (+3)
                    } else if is_resident {
                        0 // Resident (+0)
                    } else {
                        2 // Other Hero professions (+2)
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
            for part in s.split([',', '\n', '\r']) {
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
                    let def_str = part[pos + 1..].trim();
                    if let Ok(def) = def_str.parse::<i32>() {
                        citizens.push(SimulationCitizen {
                            name: name.to_string(),
                            defense: def + home_bonus,
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

fn parse_complete_modal_text(text: &str) -> (SimConfig, Vec<SimulationCitizen>) {
    let mut config = SimConfig {
        iterations: 10000,
        day: 1,
        nb_hab: 40,
        ..Default::default()
    };
    let mut citizens = Vec::new();

    for line in text.split(['\n', '\r']) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with('-') {
            continue;
        }

        if let Some(pos) = line.find(':') {
            let key = line[..pos].trim().to_lowercase();
            let val_str = line[pos + 1..].trim();

            match key.as_str() {
                "defense" | "défense" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.defense = v;
                    }
                }
                "tdg" | "estimations" => {
                    if let Some(dash_pos) = val_str.find('-') {
                        if let Ok(mn) = val_str[..dash_pos].trim().parse::<i32>() {
                            config.tdg_min = mn;
                        }
                        if let Ok(mx) = val_str[dash_pos + 1..].trim().parse::<i32>() {
                            config.tdg_max = mx;
                        }
                    } else if let Ok(v) = val_str.parse::<i32>() {
                        config.tdg_min = v;
                        config.tdg_max = v;
                    }
                }
                "tdg_min" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.tdg_min = v;
                    }
                }
                "tdg_max" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.tdg_max = v;
                    }
                }
                "min_def" | "defense_minimale" | "défense_minimale" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.min_def = v;
                    }
                }
                "day" | "jour" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.day = v;
                    }
                }
                "iterations" | "itérations" => {
                    if let Ok(v) = val_str.parse::<u32>() {
                        config.iterations = v;
                    }
                }
                "reactor" | "réacteur" => {
                    let lower = val_str.to_lowercase();
                    config.is_reactor_built = lower == "true"
                        || lower == "1"
                        || lower == "oui"
                        || lower == "yes"
                        || lower == "y";
                }
                "nb_hab" | "citoyens_max" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.nb_hab = v;
                    }
                }
                "b_level" | "tercile" => {
                    if val_str.to_lowercase() != "none"
                        && val_str.to_lowercase() != "n"
                        && let Ok(v) = val_str.parse::<i32>()
                    {
                        config.b_level = Some(v);
                    }
                }
                "population" => {
                    if val_str.to_lowercase() != "none"
                        && val_str.to_lowercase() != "n"
                        && let Ok(v) = val_str.parse::<i32>()
                    {
                        config.population = Some(v);
                    }
                }
                "chaos" => {
                    let lower = val_str.to_lowercase();
                    config.is_chaos = lower == "true"
                        || lower == "1"
                        || lower == "oui"
                        || lower == "yes"
                        || lower == "y";
                }
                "devastated" | "dévastée" | "devast" => {
                    let lower = val_str.to_lowercase();
                    config.is_devastated = lower == "true"
                        || lower == "1"
                        || lower == "oui"
                        || lower == "yes"
                        || lower == "y";
                }
                "nb_drapo" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.nb_drapo = v;
                    }
                }
                "home_bonus" | "bonus_maison" => {
                    if let Ok(v) = val_str.parse::<i32>() {
                        config.home_bonus = v;
                    }
                }
                _ => {
                    if let Ok(def) = val_str.parse::<i32>() {
                        let name = line[..pos].trim().to_string();
                        citizens.push(SimulationCitizen { name, defense: def });
                    }
                }
            }
        }
    }

    (config, citizens)
}

fn respond_with_defenses_modal(
    config: &SimConfig,
    is_api: bool,
    citizens: &[SimulationCitizen],
) -> Result<ApiGatewayV2httpResponse, Error> {
    info!("Responding with defenses edit modal");

    let mode_tag = if config.is_complete { "comp" } else { "std" };
    let custom_id = if is_api {
        format!("dm:api:{}", mode_tag)
    } else {
        format!("dm:manual:{}", mode_tag)
    };

    let pop_str = config
        .population
        .map(|v| v.to_string())
        .unwrap_or_else(|| "none".to_string());

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
        format!("population: {}", pop_str),
        format!("chaos: {}", config.is_chaos),
        format!("devastated: {}", config.is_devastated),
    ]);

    if config.is_complete {
        config_lines.push("---".to_string());
        for c in &citizens_sorted {
            config_lines.push(format!("{}: {}", c.name, c.defense));
        }
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

    let was_complete = custom_id.contains("comp");

    let token = interaction.token.clone().unwrap_or_default();
    let application_id = interaction.application_id.clone().unwrap_or_default();

    let defenses_val = interaction.get_modal_value("defenses_input").unwrap_or("");

    let (mut config, citizens) = parse_complete_modal_text(defenses_val);
    config.is_complete = was_complete;
    config.custom_defenses = Some(defenses_val.to_string());

    let job = SimulationJob {
        token,
        application_id,
        config,
        citizens,
    };

    enqueue_simulation(&job, sqs_client, queue_url).await
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
    let http_client = reqwest::Client::new();
    let queue_url = std::env::var("SQS_QUEUE_URL").expect("SQS_QUEUE_URL must be set");
    let public_key = std::env::var("DISCORD_PUBLIC_KEY").expect("DISCORD_PUBLIC_KEY must be set");

    info!("Starting DebordoLambda Discord handler");

    lambda_runtime::run(service_fn(move |event| {
        let client = sqs_client.clone();
        let url = queue_url.clone();
        let db = dynamodb_client.clone();
        let ssm = ssm_client.clone();
        let http = http_client.clone();
        let pkey = public_key.clone();
        async move { handler(event, client, url, db, ssm, &http, &pkey).await }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_citizens_applies_home_bonus() {
        let citizens = resolve_citizens(Some("Axfalt:10"), 4, None, 2, 5);
        assert_eq!(citizens.len(), 2);
        assert_eq!(citizens[0].name, "Axfalt");
        assert_eq!(citizens[0].defense, 14); // 10 + 4
        assert_eq!(citizens[1].name, "Citoyen 1");
        assert_eq!(citizens[1].defense, 9); // 5 (min_def) + 4
    }
}
