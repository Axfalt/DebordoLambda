//! Worker Lambda - déclenché par SQS, exécute la simulation et envoie le résultat à Discord.

use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;
use tokio::time::{Duration, MissedTickBehavior, interval, sleep, timeout};
use tracing::{error, info};

use debordo_lib::config::{
    ESTIMATION_CANCEL_BUTTON, ESTIMATION_CONFIG_BUTTON, EstimationJob, EstimationSource,
    EstimationStage, JobType, SimulationJob, format_defense_search_results, format_reparo_results,
    format_results, truncate_for_discord,
};
use debordo_lib::database;
use debordo_lib::discord::api::{
    delete_original, post_followup_mentioning, send_followup, send_followup_with_button,
    send_followup_with_cancel, send_followup_without_buttons,
};
use debordo_lib::quickchart::{build_chart_config, create_chart_url};
use debordo_lib::simulation::{
    complete_overflow_probability, overflow_probability, required_defense,
};
use estimation25_lib::mho::{self, MhoEstimations};
use estimation25_lib::parse::InputOverrides;
use estimation25_lib::{
    EstimConf, EstimationError, EstimationInput, format_summary, parse_text, seed_slice,
};

const SIMULATION_TIMEOUT_SECS: u64 = 120;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const DISCORD_MESSAGE_MAX_LENGTH: usize = 2000;
const TRUNCATED_NOTICE: &str = "\n… (message tronqué, réponse trop longue pour Discord)";
/// One `/estimation25` part must finish within the 300 s Lambda timeout.
const ESTIMATION_TIMEOUT_SECS: u64 = 280;
/// Parallel parts of an `/estimation25` run (`ESTIMATION_PARTS` overrides it).
/// 8 fits the free tier's 10 concurrent executions in a single wave, while the worker's SQS
/// trigger (maximum concurrency 8) leaves 2 slots for the receiver.
const DEFAULT_ESTIMATION_PARTS: u32 = 8;
/// Delay of the watchdog of a run: well past a normal run, within Discord's 15 minutes to edit
/// the interaction reply (and SQS's 900 s maximum delay).
const ESTIMATION_WATCHDOG_SECS: i32 = 600;
const ESTIMATION_SEEDS: u64 = 1 << 32;
/// How often each part reports how many seeds it has searched.
const PROGRESS_REPORT_SECS: u64 = 5;
/// Minimum delay between two edits of the waiting message, across all the parts of a run
/// (well within Discord's rate limit).
const PROGRESS_EDIT_GAP_SECS: u64 = 5;
/// A progress edit slower than this is abandoned, so it cannot land after the next one.
const PROGRESS_EDIT_TIMEOUT_SECS: u64 = 3;
/// The bar moves every edit, the phrase every other one.
const EDITS_PER_PHRASE: u64 = 2;
const PROGRESS_BAR_CELLS: u64 = 10;

const PROGRESS_MESSAGES: [&str; 16] = [
    "⏳ Recompte les zombies avec attention...",
    "⏳ Nettoie la lunette de la tour...",
    "⏳ Demande à Cubique si on sera Top2...",
    "⏳ Ajoute de l'huile de frein dans le réacteur...",
    "⏳ Cherche qui va faire l'os...",
    "⏳ Ooops j'ai compté de travers, boarf c'est pas si grave...",
    "⏳ Planque un VTT sur le puit abandonné...",
    "⏳ Recompte les oignons pour préparer la soupe...",
    "⏳ Recalcule la probabilité de monter une table...",
    "⏳ Recompte le nombre de candidats à la présidentielle de 2027...",
    "⏳ Je prends bien en compte de laisser mourir Fofotre...",
    "⏳ Recompte les graines...",
    "⏳ Oui bah fallait pas oublier une estimation...",
    "⏳ Analyse du ~~sanctuaire~~ zoo en cours...",
    "⏳ Compte les grains de sable pour voir",
    "⏳ Et un peu de vitriole ...",
];

/// Waiting message number `step` of a run, starting at a place taken from its id so consecutive
/// runs do not all open with the same line.
fn progress_message(run_id: &str, step: u64) -> &'static str {
    let offset = run_id
        .get(..8)
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or(0);
    let len = PROGRESS_MESSAGES.len() as u64;
    PROGRESS_MESSAGES[((offset % len + step % len) % len) as usize]
}

fn waiting_message(run_id: &str, step: u64, searched: u64) -> String {
    // Capped at 99 %: the search is only over once the result replaces the message.
    let pct = (searched.min(ESTIMATION_SEEDS) * 100 / ESTIMATION_SEEDS).min(99);
    let full = (pct * PROGRESS_BAR_CELLS / 100) as usize;
    let empty = PROGRESS_BAR_CELLS as usize - full;
    format!(
        "{}\n`{}{}` {pct} %",
        progress_message(run_id, step),
        "▰".repeat(full),
        "▱".repeat(empty)
    )
}

/// Clients shared by every invocation, built once per cold start (cheap to clone).
#[derive(Clone)]
struct Clients {
    http: reqwest::Client,
    sqs: aws_sdk_sqs::Client,
    dynamodb: aws_sdk_dynamodb::Client,
    /// Queue the `/estimation25` parts are sent to.
    queue_url: Option<String>,
}

fn discord_text(content: &str) -> String {
    truncate_for_discord(content, DISCORD_MESSAGE_MAX_LENGTH, TRUNCATED_NOTICE)
}

async fn handler(event: LambdaEvent<SqsEvent>, clients: &Clients) -> Result<(), Error> {
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

        let stage = job.estimation.as_ref().map(|e| &e.stage);
        if let Some(EstimationStage::Part {
            run_id,
            index,
            parts,
            ..
        }) = stage
            && let Some(wait) = queue_wait_ms(&record.attributes)
        {
            info!(
                "Estimation part {}/{} of {} waited {} ms in the queue",
                index + 1,
                parts,
                run_id,
                wait
            );
        }
        // A failed bench fails the invocation, so Power Tuning does not time it as a success.
        let bench = matches!(stage, Some(EstimationStage::Bench { .. }));
        match process_job(job, clients).await {
            Err(e) if bench => return Err(e),
            Err(e) => error!("Failed to process simulation job: {}", e),
            Ok(()) => {}
        }
    }
    Ok(())
}

/// Time between the sending of an SQS message and its first delivery.
fn queue_wait_ms(attributes: &HashMap<String, String>) -> Option<u64> {
    let millis = |name: &str| attributes.get(name)?.parse::<u64>().ok();
    Some(millis("ApproximateFirstReceiveTimestamp")?.saturating_sub(millis("SentTimestamp")?))
}

async fn process_job(job: SimulationJob, clients: &Clients) -> Result<(), Error> {
    let config = job.config.clone();
    info!("Processing simulation with config: {:?}", config);

    let http_client = &clients.http;
    match job.job_type {
        JobType::Debordo if config.target_death.is_some() => {
            process_defense_search_job(job, config, http_client).await
        }
        JobType::Debordo => process_debordo_job(job, config, http_client).await,
        JobType::Reparation => process_reparo_job(job, config, http_client).await,
        JobType::Estimation => process_estimation_job(job, clients).await,
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

async fn process_defense_search_job(
    job: SimulationJob,
    config: debordo_lib::config::SimConfig,
    http_client: &reqwest::Client,
) -> Result<(), Error> {
    let citizens = job.citizens.clone();
    let sim_config = config.clone();
    let target = config.target_death.unwrap_or(0.0);

    let start = Instant::now();
    let result = timeout(
        Duration::from_secs(SIMULATION_TIMEOUT_SECS),
        tokio::task::spawn_blocking(move || required_defense(&sim_config, &citizens, target)),
    )
    .await;

    let content = match result {
        Err(_elapsed) => {
            error!(
                "Defense search timed out after {}s",
                SIMULATION_TIMEOUT_SECS
            );
            "⏱️ La recherche de défense a expiré. Essayez avec moins d'itérations ou une plage TDG plus étroite.".to_string()
        }
        Ok(Err(e)) => {
            error!("Defense search panicked: {}", e);
            "❌ La simulation a échoué. Veuillez réessayer.".to_string()
        }
        Ok(Ok(search)) => {
            format_defense_search_results(&config, &search, start.elapsed().as_millis())
        }
    };

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Defense search results sent to Discord");
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
            error!(
                "Reparo simulation timed out after {}s",
                SIMULATION_TIMEOUT_SECS
            );
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

    let content = discord_text(&content);

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Reparo simulation results sent to Discord");
    Ok(())
}

// /estimation25: the plan job resolves the readings and fans the 2^32 seeds out to part jobs;
// each part searches its slice and records it in DynamoDB; the last one posts the result.

fn estimation_parts() -> u32 {
    std::env::var("ESTIMATION_PARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ESTIMATION_PARTS)
        .clamp(1, 256)
}

/// Replaces the deferred message with `content`, removing the "Annuler" button of a waiting
/// message.
async fn reply(job: &SimulationJob, clients: &Clients, content: &str) -> Result<(), Error> {
    let content = discord_text(content);
    send_followup_without_buttons(&clients.http, &job.application_id, &job.token, &content).await?;
    Ok(())
}

/// Discord id of the user who ran `/estimation25`.
fn caller(job: &SimulationJob) -> Option<String> {
    job.estimation.as_ref().and_then(|e| e.user_id.clone())
}

/// A follow-up job of the same interaction and caller.
fn estimation_job(
    job: &SimulationJob,
    overrides: &InputOverrides,
    stage: EstimationStage,
) -> SimulationJob {
    SimulationJob {
        token: job.token.clone(),
        application_id: job.application_id.clone(),
        job_type: JobType::Estimation,
        estimation: Some(EstimationJob {
            overrides: overrides.clone(),
            stage,
            user_id: caller(job),
        }),
        ..Default::default()
    }
}

fn cancel_button_id(run_id: &str) -> String {
    format!("{ESTIMATION_CANCEL_BUTTON}{run_id}")
}

fn config_button_id(run_id: &str) -> String {
    format!("{ESTIMATION_CONFIG_BUTTON}{run_id}")
}

/// Final outcome of a run (result or failure). Editing a message never notifies a mention, so
/// it is posted as a new message mentioning the caller and the waiting message is removed;
/// if that post fails, the waiting message is edited instead so the outcome is never lost.
/// `button` adds the "Voir la configuration" button (results only).
async fn deliver(
    job: &SimulationJob,
    clients: &Clients,
    content: &str,
    button: Option<&str>,
) -> Result<(), Error> {
    let (http, app, token) = (&clients.http, &job.application_id, &job.token);
    let edit_waiting_message = || async {
        match button {
            Some(custom_id) => {
                let content = discord_text(content);
                send_followup_with_button(http, app, token, &content, custom_id).await?;
                Ok(())
            }
            None => reply(job, clients, content).await,
        }
    };
    let Some(user_id) = caller(job) else {
        return edit_waiting_message().await;
    };
    let text = discord_text(&format!("<@{user_id}>\n{content}"));
    match post_followup_mentioning(http, app, token, &text, &user_id, button).await {
        Ok(()) => {
            if let Err(e) = delete_original(http, app, token).await {
                info!("Could not delete the waiting message: {}", e);
            }
            Ok(())
        }
        Err(e) => {
            error!(
                "Notified follow-up failed, editing the waiting message: {}",
                e
            );
            edit_waiting_message().await
        }
    }
}

/// Failure of a part: reported once per run. The part claims the run first, so the other parts
/// (failing the same way, or finishing later) and the watchdog stay silent.
async fn deliver_failure(
    job: &SimulationJob,
    clients: &Clients,
    run_id: &str,
    content: &str,
) -> Result<(), Error> {
    match database::claim_estimation_run(run_id, &clients.dynamodb).await {
        Ok(Some(_)) => deliver(job, clients, content, None).await,
        Ok(None) => {
            info!("Run {} already reported, not posting: {}", run_id, content);
            Ok(())
        }
        Err(e) => {
            // Better a duplicate than a run left on its waiting message.
            error!("Could not claim run {} to report a failure: {}", run_id, e);
            deliver(job, clients, content, None).await
        }
    }
}

async fn process_estimation_job(job: SimulationJob, clients: &Clients) -> Result<(), Error> {
    let Some(estimation) = job.estimation.clone() else {
        error!("Estimation job without estimation payload, skipping");
        return Ok(());
    };
    match estimation.stage {
        EstimationStage::Plan { source } => {
            plan_estimation(&job, source, &estimation.overrides, clients).await
        }
        EstimationStage::Part {
            run_id,
            index,
            parts,
            input,
        } => run_estimation_part(&job, &run_id, index, parts, input, clients).await,
        EstimationStage::Watchdog { run_id, parts } => {
            check_estimation_run(&job, &run_id, parts, clients).await
        }
        EstimationStage::Bench { input, parts } => run_estimation_bench(input, parts).await,
    }
}

/// Searches the first of `parts` slices like a part would, without DynamoDB nor Discord: the
/// duration of the invocation is what AWS Lambda Power Tuning measures.
async fn run_estimation_bench(input: EstimationInput, parts: u32) -> Result<(), Error> {
    let Some(slice) = seed_slice(0, parts) else {
        return Err(format!("Bench slice out of range for {parts} parts").into());
    };
    let start = Instant::now();
    let slice_len = u64::from(slice.end() - slice.start()) + 1;
    let cancel = Arc::new(AtomicBool::new(false));
    let search_cancel = Arc::clone(&cancel);
    let search = tokio::task::spawn_blocking(move || {
        estimation25_lib::search_seeds(
            &input,
            &EstimConf::default(),
            slice,
            &AtomicU64::new(0),
            &search_cancel,
        )
    });
    let matches = match timeout(Duration::from_secs(ESTIMATION_TIMEOUT_SECS), search).await {
        Ok(Ok(Ok(matches))) => matches,
        Ok(Ok(Err(e))) => return Err(format!("Estimation bench failed: {e}").into()),
        Ok(Err(e)) => return Err(format!("Estimation bench panicked: {e}").into()),
        Err(_) => {
            cancel.store(true, Ordering::Relaxed);
            return Err("Estimation bench timed out".into());
        }
    };
    info!(
        "Estimation bench: {} window(s), {} seeds (1/{} of the space) in {:.1} s on {} thread(s)",
        matches.len(),
        slice_len,
        parts.max(1),
        start.elapsed().as_secs_f64(),
        rayon::current_num_threads()
    );
    Ok(())
}

/// Watchdog: if the run has not posted its result yet, it never will (a part was lost or
/// failed): replace the waiting message with an error.
async fn check_estimation_run(
    job: &SimulationJob,
    run_id: &str,
    parts: u32,
    clients: &Clients,
) -> Result<(), Error> {
    let Some(done) = database::claim_estimation_run(run_id, &clients.dynamodb).await? else {
        return Ok(());
    };
    error!(
        "Estimation run {} incomplete after {} s: {}/{} parts done",
        run_id, ESTIMATION_WATCHDOG_SECS, done, parts
    );
    deliver(
        job,
        clients,
        "⏱️ La recherche n'a pas abouti. Veuillez réessayer.",
        None,
    )
    .await
}

/// Readings of the run, as the CLI would read them (MyHordes Optimizer or pasted text), with
/// their pasteable text (kept for the result's "Voir la configuration" button).
async fn resolve_readings(
    source: EstimationSource,
    overrides: &InputOverrides,
    http: &reqwest::Client,
) -> Result<(EstimationInput, String), String> {
    let (town_id, day, pandemonium) = match source {
        EstimationSource::Text(text) => {
            let parsed = parse_text(&text);
            let (settings, town_id) = (parsed.settings(overrides), parsed.town_id);
            let input = parsed
                .into_input(overrides)
                .map_err(|e| format!("❌ Erreur : {e}"))?;
            let config = estimation25_lib::parse::format_input_text(&input, &settings, town_id);
            return Ok((input, config));
        }
        EstimationSource::Mho {
            town_id,
            day,
            pandemonium,
        } => (town_id, day, pandemonium),
    };

    let attack_day = day + i64::from(overrides.future.unwrap_or(false));
    let [attack_payload, eve_payload] = mho::payload_days(attack_day);
    let (attack, eve) = tokio::join!(
        fetch_mho(http, attack_payload, town_id),
        fetch_mho(http, eve_payload, town_id)
    );
    let (attack, eve) = (attack?, eve?);

    let overrides = overrides.clone().for_town(pandemonium);
    let input = mho::attack_input(attack_day, attack.as_ref(), eve.as_ref(), &overrides)
        .map_err(|e| match e {
            EstimationError::NoReadings => format!(
                "❌ MyHordes Optimizer n'a aucun relevé pour la ville {town_id} (attaque du J{attack_day})."
            ),
            e => format!("❌ Erreur : {e}"),
        })?;
    let config = estimation25_lib::parse::format_input_text(&input, &overrides, Some(town_id));
    Ok((input, config))
}

/// What the watchtower showed on `day` (`None` before day 1).
async fn fetch_mho(
    http: &reqwest::Client,
    day: i64,
    town_id: i64,
) -> Result<Option<MhoEstimations>, String> {
    if day < 1 {
        return Ok(None);
    }
    let (header, origin) = mho::ORIGIN_HEADER;
    let response = http
        .get(mho::estimations_url(day, town_id))
        .header(header, origin)
        .send()
        .await
        .map_err(|e| format!("❌ MyHordes Optimizer injoignable : {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("❌ Réponse de MyHordes Optimizer illisible : {e}"))?;
    if !status.is_success() {
        return Err(format!("❌ MyHordes Optimizer a répondu {status}."));
    }
    MhoEstimations::from_json(&body)
        .map(Some)
        .map_err(|e| format!("❌ Réponse de MyHordes Optimizer inattendue : {e}"))
}

async fn plan_estimation(
    job: &SimulationJob,
    source: EstimationSource,
    overrides: &InputOverrides,
    clients: &Clients,
) -> Result<(), Error> {
    let (input, config) = match resolve_readings(source, overrides, &clients.http).await {
        Ok(resolved) => resolved,
        Err(message) => return reply(job, clients, &message).await,
    };
    if let Err(e) = estimation25_lib::check_input(&input, &EstimConf::default()) {
        return reply(job, clients, &format!("❌ Erreur : {e}")).await;
    }
    let Some(queue_url) = clients.queue_url.as_deref() else {
        error!("SQS_QUEUE_URL is not set on the worker: cannot fan /estimation25 out");
        return reply(
            job,
            clients,
            "❌ La recherche n'est pas configurée sur ce serveur.",
        )
        .await;
    };

    let parts = estimation_parts();
    let run_id = database::estimation_run_id(&job.token);
    match database::create_estimation_run(
        &run_id,
        parts,
        &config,
        caller(job).as_deref(),
        &clients.dynamodb,
    )
    .await
    {
        Ok(true) => {}
        Ok(false) => {
            // Redelivered plan job: the run is already going (or done); leave it alone.
            info!(
                "Estimation run {} already exists, skipping duplicate plan",
                run_id
            );
            return Ok(());
        }
        Err(e) => {
            error!("Failed to create estimation run {}: {}", run_id, e);
            return reply(
                job,
                clients,
                "❌ La recherche n'a pas pu démarrer. Réessayez.",
            )
            .await;
        }
    }
    send_followup_with_cancel(
        &clients.http,
        &job.application_id,
        &job.token,
        &waiting_message(&run_id, 0, 0),
        &cancel_button_id(&run_id),
    )
    .await?;

    let indices: Vec<u32> = (0..parts).collect();
    for chunk in indices.chunks(10) {
        let mut entries = Vec::with_capacity(chunk.len());
        for &index in chunk {
            let part = estimation_job(
                job,
                overrides,
                EstimationStage::Part {
                    run_id: run_id.clone(),
                    index,
                    parts,
                    input: input.clone(),
                },
            );
            entries.push(
                SendMessageBatchRequestEntry::builder()
                    .id(index.to_string())
                    .message_body(serde_json::to_string(&part)?)
                    .build()?,
            );
        }
        let sent = clients
            .sqs
            .send_message_batch()
            .queue_url(queue_url)
            .set_entries(Some(entries))
            .send()
            .await;
        let failed = match sent {
            Ok(out) => !out.failed().is_empty(),
            Err(e) => {
                error!("SendMessageBatch failed for run {}: {}", run_id, e);
                true
            }
        };
        if failed {
            return reply(
                job,
                clients,
                "❌ La recherche n'a pas pu démarrer. Réessayez.",
            )
            .await;
        }
    }
    info!("Estimation run {} fanned out to {} parts", run_id, parts);

    // Delayed check, so a lost or failed part cannot leave the waiting message forever.
    let watchdog = estimation_job(
        job,
        overrides,
        EstimationStage::Watchdog {
            run_id: run_id.clone(),
            parts,
        },
    );
    if let Err(e) = clients
        .sqs
        .send_message()
        .queue_url(queue_url)
        .message_body(serde_json::to_string(&watchdog)?)
        .delay_seconds(ESTIMATION_WATCHDOG_SECS)
        .send()
        .await
    {
        error!("Failed to schedule the watchdog of run {}: {}", run_id, e);
    }
    Ok(())
}

async fn run_estimation_part(
    job: &SimulationJob,
    run_id: &str,
    index: u32,
    parts: u32,
    input: EstimationInput,
    clients: &Clients,
) -> Result<(), Error> {
    let Some(slice) = seed_slice(index, parts) else {
        error!("Part {} out of range for {} parts", index, parts);
        return Ok(());
    };

    let start = Instant::now();
    let slice_len = u64::from(slice.end() - slice.start()) + 1;
    let progress = Arc::new(AtomicU64::new(0));
    // Set on timeout, or once the run no longer needs this part: the blocking search would
    // otherwise keep burning the CPU of this (frozen, then reused) Lambda instance.
    let cancel = Arc::new(AtomicBool::new(false));
    let search_input = input.clone();
    let (search_progress, search_cancel) = (Arc::clone(&progress), Arc::clone(&cancel));
    let search = tokio::task::spawn_blocking(move || {
        estimation25_lib::search_seeds(
            &search_input,
            &EstimConf::default(),
            slice,
            &search_progress,
            &search_cancel,
        )
    });
    let deadline = sleep(Duration::from_secs(ESTIMATION_TIMEOUT_SECS));
    tokio::pin!(search, deadline);
    let period = Duration::from_secs(PROGRESS_REPORT_SECS);
    // The first tick is immediate: a part of a cancelled run stops before doing any work.
    let mut ticks = interval(period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Progress is reported from this task, between polls of the search: an edit is always
    // finished before the part is recorded, hence before the result can be posted.
    let mut live = true;
    let searched = loop {
        tokio::select! {
            joined = &mut search => break Some(joined),
            () = &mut deadline => break None,
            _ = ticks.tick(), if live => {
                let searched = progress.load(Ordering::Relaxed);
                live = show_progress(job, clients, run_id, index, searched).await;
                if !live {
                    // The run was reported (result, failure or cancelled by its caller) or
                    // this part was already recorded by an earlier delivery: the rest of the
                    // search is useless.
                    info!("Part {} of run {} no longer needed, cancelling", index, run_id);
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
    };
    let matches = match searched {
        Some(Ok(Ok(matches))) => matches,
        Some(Ok(Err(EstimationError::Cancelled))) => return Ok(()),
        Some(Ok(Err(e))) => {
            return deliver_failure(job, clients, run_id, &format!("❌ Erreur : {e}")).await;
        }
        Some(Err(e)) => {
            error!("Estimation part {} of {} panicked: {}", index, run_id, e);
            return deliver_failure(
                job,
                clients,
                run_id,
                "❌ La recherche a échoué. Veuillez réessayer.",
            )
            .await;
        }
        None => {
            cancel.store(true, Ordering::Relaxed);
            error!("Estimation part {} of {} timed out", index, run_id);
            return deliver_failure(
                job,
                clients,
                run_id,
                "⏱️ La recherche a expiré. Veuillez réessayer.",
            )
            .await;
        }
    };
    info!(
        "Estimation part {}/{} of {}: {} seed(s) in {:.1} s",
        index + 1,
        parts,
        run_id,
        matches.len(),
        start.elapsed().as_secs_f64()
    );

    let done = match database::record_estimation_part(
        run_id,
        index,
        slice_len,
        &matches,
        &clients.dynamodb,
    )
    .await
    {
        Ok(Some(done)) => done,
        Ok(None) => return Ok(()),
        Err(e) => {
            error!("Failed to record part {} of run {}: {}", index, run_id, e);
            return deliver_failure(
                job,
                clients,
                run_id,
                "❌ La recherche a échoué (enregistrement impossible). Veuillez réessayer.",
            )
            .await;
        }
    };
    let button = config_button_id(run_id);
    let (content, button) =
        match estimation25_lib::finish(&input, &EstimConf::default(), done.matches) {
            Ok(estimate) => (
                format!(
                    "{}\n-# ⏱️ {ESTIMATION_SEEDS} runs testées en {} s",
                    format_summary(&input, &estimate),
                    database::seconds_since(done.started_at)
                ),
                Some(button.as_str()),
            ),
            Err(e) => (format!("❌ Erreur : {e}"), None),
        };
    deliver(job, clients, &content, button).await?;
    info!("Estimation run {} result sent to Discord", run_id);
    Ok(())
}

/// Reports the seeds part `index` has searched and, when it is this part's turn, edits the
/// waiting message with the run's progress. Returns `false` once the waiting message must no
/// longer be touched (result posted, or this part already recorded by an earlier delivery).
async fn show_progress(
    job: &SimulationJob,
    clients: &Clients,
    run_id: &str,
    index: u32,
    searched: u64,
) -> bool {
    let reported = database::report_estimation_progress(
        run_id,
        index,
        searched,
        PROGRESS_EDIT_GAP_SECS,
        &clients.dynamodb,
    )
    .await;
    let progress = match reported {
        Ok(Some(progress)) => progress,
        Ok(None) => return false,
        Err(e) => {
            info!("Progress report of part {} failed: {}", index, e);
            return true;
        }
    };
    if let Some(edit) = progress.edit {
        let step = edit.div_ceil(EDITS_PER_PHRASE);
        let content = waiting_message(run_id, step, progress.searched);
        // A late edit would land after the next one and show an older total.
        let edited = timeout(
            Duration::from_secs(PROGRESS_EDIT_TIMEOUT_SECS),
            send_followup(&clients.http, &job.application_id, &job.token, &content),
        )
        .await;
        match edited {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => info!("Progress update of part {} skipped: {}", index, e),
            Err(_) => info!("Progress update of part {} timed out", index),
        }
    }
    true
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    // INFO unless RUST_LOG says otherwise (`from_default_env` alone keeps only errors).
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::builder()
                .with_default_directive(tracing::level_filters::LevelFilter::INFO.into())
                .from_env_lossy(),
        )
        .init();

    let aws_config = aws_config::load_from_env().await;
    let clients = Clients {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(HTTP_REQUEST_TIMEOUT_SECS))
            .build()
            .expect("failed to build reqwest client"),
        sqs: aws_sdk_sqs::Client::new(&aws_config),
        dynamodb: aws_sdk_dynamodb::Client::new(&aws_config),
        queue_url: std::env::var("SQS_QUEUE_URL").ok(),
    };
    info!("Starting DebordoLambda Worker");
    lambda_runtime::run(service_fn(move |event| {
        let clients = clients.clone();
        async move { handler(event, &clients).await }
    }))
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Power Tuning payloads (`power-tuning/`): one `Bench` job each, over the j15 fixture.
    const POWER_TUNING_PAYLOADS: [(&str, u32); 2] = [
        (include_str!("../power-tuning/payload.json"), 8),
        (include_str!("../power-tuning/payload-p64.json"), 64),
    ];

    fn bench_job(parts: u32) -> SimulationJob {
        let text = include_str!("../estimation25_lib/tests/data/j15_real_attack_2587.txt");
        let input = parse_text(text)
            .into_input(&InputOverrides::default())
            .unwrap();
        SimulationJob {
            job_type: JobType::Estimation,
            estimation: Some(EstimationJob {
                overrides: InputOverrides::default(),
                stage: EstimationStage::Bench { input, parts },
                user_id: None,
            }),
            ..Default::default()
        }
    }

    #[test]
    fn test_power_tuning_payloads_are_bench_jobs_of_the_fixture() {
        for (payload, parts) in POWER_TUNING_PAYLOADS {
            let event: SqsEvent = serde_json::from_str(payload).unwrap();
            assert_eq!(event.records.len(), 1);
            let body = event.records[0].body.as_deref().unwrap();
            let job: SimulationJob = serde_json::from_str(body).unwrap();
            assert_eq!(job.job_type, JobType::Estimation);
            assert_eq!(job.estimation, bench_job(parts).estimation, "{parts} parts");
        }
    }

    #[test]
    fn test_queue_wait_is_first_receive_minus_sent() {
        let attributes = HashMap::from([
            ("SentTimestamp".to_string(), "1000".to_string()),
            (
                "ApproximateFirstReceiveTimestamp".to_string(),
                "1250".to_string(),
            ),
        ]);
        assert_eq!(queue_wait_ms(&attributes), Some(250));
        assert_eq!(queue_wait_ms(&HashMap::new()), None);
    }

    #[test]
    #[ignore = "writes power-tuning/*.json"]
    fn generate_power_tuning_payloads() {
        for (name, parts) in [("payload.json", 8), ("payload-p64.json", 64)] {
            let body = serde_json::to_string(&bench_job(parts)).unwrap();
            let event = serde_json::json!({ "Records": [{ "body": body }] });
            std::fs::write(
                format!("power-tuning/{name}"),
                serde_json::to_string_pretty(&event).unwrap() + "\n",
            )
            .unwrap();
        }
    }

    #[test]
    fn test_progress_message_rotates_from_the_run_offset() {
        let run_id = database::estimation_run_id("interaction-token");
        let first = progress_message(&run_id, 0);
        assert!(PROGRESS_MESSAGES.contains(&first));
        assert_ne!(first, progress_message(&run_id, 1));
        let len = PROGRESS_MESSAGES.len() as u64;
        assert_eq!(first, progress_message(&run_id, len));
        // A malformed id falls back to the start of the list.
        assert_eq!(progress_message("", 0), PROGRESS_MESSAGES[0]);
    }

    #[test]
    fn test_waiting_message_shows_the_progress_bar() {
        assert_eq!(
            waiting_message("", 0, 0),
            format!("{}\n`▱▱▱▱▱▱▱▱▱▱` 0 %", PROGRESS_MESSAGES[0])
        );
        assert_eq!(
            waiting_message("", 1, ESTIMATION_SEEDS * 42 / 100 + 1),
            format!("{}\n`▰▰▰▰▱▱▱▱▱▱` 42 %", PROGRESS_MESSAGES[1])
        );
        assert!(waiting_message("", 0, ESTIMATION_SEEDS).ends_with("` 99 %"));
    }
}
