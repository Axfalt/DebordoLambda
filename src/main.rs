//! DebordoLambda - Commande Discord slash pour simulations Monte Carlo sur AWS Lambda.
//!
//! Ce projet fournit une commande `/debordo` pour calculer les probabilités
//! de débordement dans un jeu de stratégie.

mod config;
mod discord;
mod simulation;

use aws_lambda_events::apigw::{ApiGatewayProxyRequest, ApiGatewayProxyResponse};
use aws_lambda_events::http::HeaderMap;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use serde::Serialize;
use tracing::{error, info};

use crate::config::{format_results, SimConfig};
use crate::discord::{
    interaction_types, response_types, verify_discord_signature, DiscordInteraction,
    DiscordResponse, ResponseData,
};
use crate::simulation::calculate_defense_probabilities;

// ============================================================================
// LAMBDA HANDLER
// ============================================================================

/// Handler principal pour les requêtes Lambda via API Gateway.
async fn handler(
    event: LambdaEvent<ApiGatewayProxyRequest>,
) -> Result<ApiGatewayProxyResponse, Error> {
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

    // Vérifier la signature Discord (obligatoire)
    if !verify_discord_signature(&public_key, signature, timestamp, &body) {
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
        interaction_types::APPLICATION_COMMAND => handle_command(interaction).await,
        _ => Ok(build_response(400, "Unknown interaction type")),
    }
}

/// Répond au PING de validation Discord.
fn handle_ping() -> Result<ApiGatewayProxyResponse, Error> {
    info!("Received PING, responding with PONG");
    let response = DiscordResponse {
        response_type: response_types::PONG,
        data: None,
    };
    Ok(build_json_response(200, &response))
}

/// Traite une commande slash Discord.
async fn handle_command(
    interaction: DiscordInteraction,
) -> Result<ApiGatewayProxyResponse, Error> {
    // Extraire les options de la commande
    let options = interaction
        .data
        .as_ref()
        .and_then(|d| d.options.as_ref())
        .map(|o| o.as_slice())
        .unwrap_or(&[]);

    let config = SimConfig::from_options(options);
    info!("Running simulation with config: {:?}", config);

    // Cloner les valeurs nécessaires pour le spawn_blocking
    let defense_range = config.defense_range();
    let tdg_interval = config.tdg_interval();
    let min_def = config.min_def;
    let nb_drapo = config.nb_drapo;
    let day = config.day;
    let iterations = config.iterations;
    let points = config.points;
    let is_reactor_built = config.is_reactor_built;

    // Exécuter la simulation dans un thread séparé (car elle est bloquante)
    let results = tokio::task::spawn_blocking(move || {
        calculate_defense_probabilities(
            defense_range,
            tdg_interval,
            min_def,
            nb_drapo,
            day,
            iterations,
            points,
            is_reactor_built,
        )
    })
    .await?;

    // Trier les résultats par défense
    let mut sorted_results = results;
    sorted_results.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

    // Formater la réponse
    let content = format_results(&config, &sorted_results);

    let response = DiscordResponse {
        response_type: response_types::CHANNEL_MESSAGE_WITH_SOURCE,
        data: Some(ResponseData {
            content,
            flags: None,
        }),
    };

    Ok(build_json_response(200, &response))
}

// ============================================================================
// RESPONSE BUILDERS
// ============================================================================

/// Construit une réponse HTTP simple avec du texte.
fn build_response(status_code: i64, body: &str) -> ApiGatewayProxyResponse {
    ApiGatewayProxyResponse {
        status_code,
        headers: HeaderMap::new(),
        multi_value_headers: HeaderMap::new(),
        body: Some(aws_lambda_events::encodings::Body::Text(body.to_string())),
        is_base64_encoded: false,
    }
}

/// Construit une réponse HTTP JSON.
fn build_json_response<T: Serialize>(status_code: i64, body: &T) -> ApiGatewayProxyResponse {
    let json_body = serde_json::to_string(body).unwrap_or_default();
    let mut headers = HeaderMap::new();
    headers.insert("Content-Type", "application/json".parse().unwrap());

    ApiGatewayProxyResponse {
        status_code,
        headers,
        multi_value_headers: HeaderMap::new(),
        body: Some(aws_lambda_events::encodings::Body::Text(json_body)),
        is_base64_encoded: false,
    }
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

    info!("Starting DebordoLambda Discord handler");
    lambda_runtime::run(service_fn(handler)).await
}
