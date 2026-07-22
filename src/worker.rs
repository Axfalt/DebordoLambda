//! Worker Lambda - déclenché par SQS, exécute la simulation et envoie le résultat à Discord.

use aws_lambda_events::sqs::SqsEvent;
use lambda_runtime::{service_fn, Error, LambdaEvent};
use tokio::time::{timeout, Duration};
use tracing::{error, info};
use std::time::Instant;

use debordo_lib::config::{format_results, SimulationJob};
use debordo_lib::discord::api::send_followup;
use debordo_lib::simulation::{complete_overflow_probability, overflow_probability};

const SIMULATION_TIMEOUT_SECS: u64 = 120;

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

    let defense = config.defense as f64;
    let tdg_interval = config.tdg_interval();
    let min_def = config.min_def;
    let nb_drapo = config.nb_drapo;
    let day = config.day;
    let iterations = config.iterations;
    let is_reactor_built = config.is_reactor_built;
    let nb_hab = config.nb_hab;

    let citizens = job.citizens.clone();
    let is_complete = config.is_complete;

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(SIMULATION_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || {
            if is_complete {
                let (prob, total_runs, citizen_percentages) = complete_overflow_probability(
                    defense,
                    tdg_interval,
                    nb_drapo,
                    day,
                    iterations,
                    is_reactor_built,
                    nb_hab,
                    config.population,
                    config.is_chaos,
                    config.is_devastated,
                    &citizens,
                );
                (prob, total_runs, citizen_percentages)
            } else {
                let (prob, total_runs) = overflow_probability(
                    defense,
                    tdg_interval,
                    min_def,
                    nb_drapo,
                    day,
                    iterations,
                    is_reactor_built,
                    nb_hab,
                    config.b_level,
                    config.population,
                    config.is_chaos,
                    config.is_devastated,
                );
                (prob, total_runs, Vec::new())
            }
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
        Ok(Ok((prob, total_runs, citizen_percentages))) => format_results(
            &config,
            prob,
            start.elapsed().as_millis(),
            total_runs,
            &job.api_pulled_fields,
            &job.citizens,
            &citizen_percentages,
        ),
    };

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Simulation results sent to Discord");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let http_client = reqwest::Client::new();
    info!("Starting DebordoLambda Worker");
    lambda_runtime::run(service_fn(move |event| {
        let client = http_client.clone();
        async move { handler(event, &client).await }
    }))
    .await
}
