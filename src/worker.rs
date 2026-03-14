//! Worker Lambda - déclenché par SQS, exécute la simulation et envoie le résultat à Discord.

mod config;
mod discord;
mod simulation;

use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use tokio::time::{timeout, Duration};
use tracing::{error, info};
use std::time::Instant;

use crate::config::{format_results, SimConfig, SimulationJob};
use crate::discord::api::send_followup;
use crate::simulation::calculate_defense_probabilities;

const SIMULATION_TIMEOUT_SECS: u64 = 120;

async fn handler(event: LambdaEvent<SqsEvent>) -> Result<(), Error> {
    for record in event.payload.records {
        let body = match record.body {
            Some(b) => b,
            None => {
                error!("SQS record has no body, skipping");
                continue;
            }
        };

        let job: SimulationJob = match serde_json::from_str(&body) {
            Ok(j) => j,
            Err(e) => {
                error!("Failed to deserialize simulation job: {}", e);
                continue;
            }
        };

        if let Err(e) = process_job(job).await {
            error!("Failed to process simulation job: {}", e);
        }
    }
    Ok(())
}

async fn process_job(job: SimulationJob) -> Result<(), Error> {
    let config = SimConfig::from_options(&job.options);
    info!("Processing simulation with config: {:?}", config);

    let defense = config.defense;
    let tdg_interval = config.tdg_interval();
    let min_def = config.min_def;
    let nb_drapo = config.nb_drapo;
    let day = config.day;
    let iterations = config.iterations;
    let is_reactor_built = config.is_reactor_built;

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(SIMULATION_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            calculate_defense_probabilities(
                defense,
                tdg_interval,
                min_def,
                nb_drapo,
                day,
                iterations,
                is_reactor_built,
            )
        }),
    )
    .await;

    let content = match result {
        Err(_elapsed) => {
            error!("Simulation timed out after {}s", SIMULATION_TIMEOUT_SECS);
            "⏱️ La simulation a expiré. Essayez avec moins de points ou d'itérations."
                .to_string()
        }
        Ok(Err(e)) => {
            error!("Simulation panicked: {}", e);
            "❌ La simulation a échoué. Veuillez réessayer.".to_string()
        }
        Ok(Ok(prob)) => format_results(&config, prob, start.elapsed().as_millis()),
    };

    send_followup(&job.application_id, &job.token, &content).await?;

    info!("Simulation results sent to Discord");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("Starting DebordoLambda Worker");
    lambda_runtime::run(service_fn(handler)).await
}
