//! DebordoLambda - Commande Discord slash pour simulations de débordements

mod config;
mod database;
mod discord;

use aws_lambda_events::apigw::{ApiGatewayV2httpRequest, ApiGatewayV2httpResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::Serialize;
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
                handle_command(interaction, &sqs_client, &queue_url).await
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
) -> Result<ApiGatewayV2httpResponse, Error> {
    let token = interaction.token.unwrap_or_default();
    let application_id = interaction.application_id.unwrap_or_default();

    let options = interaction
        .data
        .and_then(|d| d.options)
        .unwrap_or_default();

    let job = SimulationJob { token, application_id, options };
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
