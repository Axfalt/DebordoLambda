//! Worker Lambda - déclenché par SQS, exécute la simulation et envoie le résultat à Discord.

use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use std::time::Instant;
use tokio::time::{Duration, timeout};
use tracing::{error, info};

use debordo_lib::config::{
    JobType, SimulationJob, format_reparo_results, format_results, truncate_for_discord,
};
use debordo_lib::discord::api::send_followup;
use debordo_lib::quickchart::{build_chart_config, create_chart_url};
use debordo_lib::simulation::{complete_overflow_probability, overflow_probability};

const SIMULATION_TIMEOUT_SECS: u64 = 120;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const DISCORD_MESSAGE_MAX_LENGTH: usize = 2000;

async fn handler(event: LambdaEvent<SqsEvent>, http_client: &reqwest::Client) -> Result<(), Error> {
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

        if let Err(e) = process_job(job, http_client).await {
            error!("Failed to process simulation job: {}", e);
        }
    }
    Ok(())
}

async fn process_job(job: SimulationJob, http_client: &reqwest::Client) -> Result<(), Error> {
    let config = job.config.clone();
    info!("Processing simulation with config: {:?}", config);

    match job.job_type {
        JobType::Debordo => process_debordo_job(job, config, http_client).await,
        JobType::Reparation => process_reparo_job(job, config, http_client).await,
    }
}

async fn process_debordo_job(
    job: SimulationJob,
    config: debordo_lib::config::SimConfig,
    http_client: &reqwest::Client,
) -> Result<(), Error> {
    let citizens = job.citizens.clone();
    let is_complete = config.is_complete;
    let sim_config = config.clone();

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(SIMULATION_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            if is_complete {
                let (prob, total_runs, citizen_percentages, avg_max_active) =
                    complete_overflow_probability(&sim_config, &citizens);
                (prob, total_runs, citizen_percentages, avg_max_active)
            } else {
                let (prob, total_runs, avg_max_active) = overflow_probability(&sim_config);
                (prob, total_runs, Vec::new(), avg_max_active)
            }
        }),
    )
    .await;

    let content = match result {
        Err(_elapsed) => {
            error!("Simulation timed out after {}s", SIMULATION_TIMEOUT_SECS);
            "⏱️ La simulation a expiré. Essayez avec moins de points ou d'itérations.".to_string()
        }
        Ok(Err(e)) => {
            error!("Simulation panicked: {}", e);
            "❌ La simulation a échoué. Veuillez réessayer.".to_string()
        }
        Ok(Ok((prob, total_runs, citizen_percentages, avg_max_active))) => format_results(
            &config,
            prob,
            start.elapsed().as_millis(),
            total_runs,
            avg_max_active,
            &job.citizens,
            &citizen_percentages,
        ),
    };

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Simulation results sent to Discord");
    Ok(())
}

async fn process_reparo_job(
    job: SimulationJob,
    config: debordo_lib::config::SimConfig,
    http_client: &reqwest::Client,
) -> Result<(), Error> {
    let buildings = job.buildings.clone();
    let buildings_for_display = buildings.clone();
    let total_defense = config.defense;
    let watch_def = config.veille;
    let tdg_interval = config.tdg_interval();
    let iterations = config.iterations;

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(SIMULATION_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            reparo_lib::calculate_reparation_probabilities(
                total_defense,
                watch_def,
                tdg_interval,
                iterations,
                &buildings,
            )
        }),
    )
    .await;

    let content = match result {
        Err(_elapsed) => {
            error!("Reparo simulation timed out after {}s", SIMULATION_TIMEOUT_SECS);
            "⏱️ La simulation a expiré. Essayez avec moins d'itérations ou une plage TDG plus étroite.".to_string()
        }
        Ok(Err(e)) => {
            error!("Reparo simulation panicked: {}", e);
            "❌ La simulation a échoué. Veuillez réessayer.".to_string()
        }
        Ok(Ok(results)) if results.is_empty() => {
            "❌ Aucun résultat : vérifiez que tdg_min <= tdg_max.".to_string()
        }
        Ok(Ok(results)) => {
            let ran_count = results
                .iter()
                .filter(|(attack, _)| {
                    reparo_lib::damage_pool(*attack, total_defense, watch_def) > 0
                })
                .count() as u64;
            let total_runs = ran_count * iterations as u64;

            let mut content = format_reparo_results(
                &config,
                &results,
                start.elapsed().as_millis(),
                total_runs,
                &buildings_for_display,
            );
            let chart_config = build_chart_config(&results);
            match create_chart_url(http_client, &chart_config).await {
                Ok(url) => {
                    content.push_str(&format!("\n\n🖼️ **Graphique**: {url}"));
                }
                Err(e) => {
                    error!("Failed to create QuickChart chart: {}", e);
                    content.push_str("\n-# ⚠️ Graphique indisponible.");
                }
            }

            content
        }
    };

    let content = truncate_for_discord(
        &content,
        DISCORD_MESSAGE_MAX_LENGTH,
        "\n… (message tronqué, réponse trop longue pour Discord)",
    );

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Reparo simulation results sent to Discord");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
        .build()
        .expect("failed to build reqwest client");
    info!("Starting DebordoLambda Worker");
    lambda_runtime::run(service_fn(move |event| {
        let client = http_client.clone();
        async move { handler(event, &client).await }
    }))
    .await
}
