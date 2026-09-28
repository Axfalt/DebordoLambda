//! Worker Lambda - déclenché par SQS, exécute la simulation et envoie le résultat à Discord.

use aws_lambda_events::sqs::SqsEvent;
use aws_sdk_sqs::types::SendMessageBatchRequestEntry;
use lambda_runtime::{Error, LambdaEvent, service_fn};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;
use tokio::time::{Duration, MissedTickBehavior, interval_at, sleep, timeout};
use tracing::{error, info};

use debordo_lib::config::{
    ESTIMATION_CONFIG_BUTTON, EstimationJob, EstimationSource, EstimationStage, JobType,
    SimulationJob, format_defense_search_results, format_reparo_results, format_results,
    truncate_for_discord,
};
use debordo_lib::database;
use debordo_lib::discord::api::{
    delete_original, post_followup_mentioning, send_followup, send_followup_with_button,
};
use debordo_lib::quickchart::{build_chart_config, create_chart_url};
use debordo_lib::simulation::{
    complete_overflow_probability, overflow_probability, required_defense,
};
use estimation25_lib::mho::{self, MhoEstimations};
use estimation25_lib::parse::InputOverrides;
use estimation25_lib::{
    EstimConf, EstimationError, EstimationInput, format_summary, parse_text, seed_slices,
};

const SIMULATION_TIMEOUT_SECS: u64 = 120;
const HTTP_REQUEST_TIMEOUT_SECS: u64 = 30;
const DISCORD_MESSAGE_MAX_LENGTH: usize = 2000;
/// One `/estimation25` part must finish within the 300 s Lambda timeout.
const ESTIMATION_TIMEOUT_SECS: u64 = 280;
/// Parallel parts of an `/estimation25` run (`ESTIMATION_PARTS` overrides it).
/// 8 fits the free tier's 10 concurrent executions in a single wave, while the worker's SQS
/// trigger (maximum concurrency 8) leaves 2 slots for the receiver.
const DEFAULT_ESTIMATION_PARTS: u32 = 8;
/// Delay of the watchdog of a run: well past a normal run, within Discord's 15 minutes to edit
/// the interaction reply (and SQS's 900 s maximum delay).
const ESTIMATION_WATCHDOG_SECS: i32 = 600;
/// `modifiers.red_soul_max_factor` of Pandemonium towns.
const PANDEMONIUM_SOUL_MAX: f64 = 666.0;
/// Seeds of a whole `/estimation25` run (2^32).
const ESTIMATION_SEEDS: u64 = 1 << 32;
/// How often each part reports how many seeds it has searched.
const PROGRESS_REPORT_SECS: u64 = 5;
/// Minimum delay between two edits of the waiting message, across all the parts of a run
/// (well within Discord's rate limit).
const PROGRESS_EDIT_GAP_SECS: u64 = 5;
/// A progress edit slower than this is abandoned, so it cannot land after the next one.
const PROGRESS_EDIT_TIMEOUT_SECS: u64 = 3;
/// Waiting-message edits per phrase: the bar moves every edit, the phrase every other one.
const EDITS_PER_PHRASE: u64 = 2;
/// Cells of the progress bar.
const PROGRESS_BAR_CELLS: u64 = 10;

/// Waiting messages of an `/estimation25` run, rotated as the search goes.
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

/// Waiting message number `step` of a run. Each run starts at its own place in the list (taken
/// from its id), so consecutive runs do not all open with the same line.
fn progress_message(run_id: &str, step: u64) -> &'static str {
    let offset = run_id
        .get(..8)
        .and_then(|hex| u64::from_str_radix(hex, 16).ok())
        .unwrap_or(0);
    let len = PROGRESS_MESSAGES.len() as u64;
    PROGRESS_MESSAGES[((offset % len + step % len) % len) as usize]
}

/// Waiting message after `searched` seeds: phrase `step` and a progress bar.
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
    /// Queue the `/estimation25` parts are sent to (`SQS_QUEUE_URL`).
    queue_url: Option<String>,
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

        if let Err(e) = process_job(job, clients).await {
            error!("Failed to process simulation job: {}", e);
        }
    }
    Ok(())
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

    let content = truncate_for_discord(
        &content,
        DISCORD_MESSAGE_MAX_LENGTH,
        "\n… (message tronqué, réponse trop longue pour Discord)",
    );

    send_followup(http_client, &job.application_id, &job.token, &content).await?;

    info!("Reparo simulation results sent to Discord");
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// /estimation25: the plan job resolves the readings and fans the 2^32 seeds out to part jobs;
// each part searches its slice and records it in DynamoDB; the last one posts the result.
// ---------------------------------------------------------------------------------------------

fn estimation_parts() -> u32 {
    std::env::var("ESTIMATION_PARTS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_ESTIMATION_PARTS)
        .clamp(1, 256)
}

/// Replaces the deferred message with `content` (truncated to Discord's limit).
async fn reply(job: &SimulationJob, clients: &Clients, content: &str) -> Result<(), Error> {
    let content = truncate_for_discord(
        content,
        DISCORD_MESSAGE_MAX_LENGTH,
        "\n… (message tronqué, réponse trop longue pour Discord)",
    );
    send_followup(&clients.http, &job.application_id, &job.token, &content).await?;
    Ok(())
}

/// Discord id of the user who ran `/estimation25`.
fn caller(job: &SimulationJob) -> Option<String> {
    job.estimation.as_ref().and_then(|e| e.user_id.clone())
}

/// `custom_id` of the "Voir la configuration" button of a run's result.
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
                let content = truncate_for_discord(
                    content,
                    DISCORD_MESSAGE_MAX_LENGTH,
                    "\n… (message tronqué, réponse trop longue pour Discord)",
                );
                send_followup_with_button(http, app, token, &content, custom_id).await?;
                Ok(())
            }
            None => reply(job, clients, content).await,
        }
    };
    let Some(user_id) = caller(job) else {
        return edit_waiting_message().await;
    };
    let text = truncate_for_discord(
        &format!("<@{user_id}>\n{content}"),
        DISCORD_MESSAGE_MAX_LENGTH,
        "\n… (message tronqué, réponse trop longue pour Discord)",
    );
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
    }
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
            let input = parse_text(&text)
                .into_input(overrides)
                .map_err(|e| format!("❌ Erreur : {e}"))?;
            return Ok((input, text));
        }
        EstimationSource::Mho {
            town_id,
            day,
            pandemonium,
        } => (town_id, day, pandemonium),
    };

    // The attack's readings are `estim` of its day and `planif` (J+1) of the day before.
    let attack_day = day + i64::from(overrides.future.unwrap_or(false));
    let [attack_payload, eve_payload] = mho::payload_days(attack_day);
    let (attack, eve) = tokio::join!(
        fetch_mho(http, attack_payload, town_id),
        fetch_mho(http, eve_payload, town_id)
    );
    let (attack, eve) = (attack?, eve?);

    let mut overrides = overrides.clone();
    if pandemonium && overrides.soul_max.is_none() {
        overrides.soul_max = Some(PANDEMONIUM_SOUL_MAX);
    }
    let input = mho::attack_input(attack_day, attack.as_ref(), eve.as_ref(), &overrides)
        .map_err(|e| match e {
            EstimationError::NoReadings => format!(
                "❌ MyHordes Optimizer n'a aucun relevé pour la ville {town_id} (attaque du J{attack_day})."
            ),
            e => format!("❌ Erreur : {e}"),
        })?;
    let config = estimation25_lib::parse::format_input_text(&input, &overrides);
    Ok((input, config))
}

/// What the watchtower showed on `day` according to MyHordes Optimizer (`None` before day 1).
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
    match database::create_estimation_run(&run_id, parts, &config, &clients.dynamodb).await {
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
    reply(job, clients, &waiting_message(&run_id, 0, 0)).await?;

    let indices: Vec<u32> = (0..parts).collect();
    for chunk in indices.chunks(10) {
        let mut entries = Vec::with_capacity(chunk.len());
        for &index in chunk {
            let part = SimulationJob {
                token: job.token.clone(),
                application_id: job.application_id.clone(),
                job_type: JobType::Estimation,
                estimation: Some(EstimationJob {
                    overrides: overrides.clone(),
                    stage: EstimationStage::Part {
                        run_id: run_id.clone(),
                        index,
                        parts,
                        input: input.clone(),
                    },
                    user_id: caller(job),
                }),
                ..Default::default()
            };
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
    let watchdog = SimulationJob {
        token: job.token.clone(),
        application_id: job.application_id.clone(),
        job_type: JobType::Estimation,
        estimation: Some(EstimationJob {
            overrides: overrides.clone(),
            stage: EstimationStage::Watchdog {
                run_id: run_id.clone(),
                parts,
            },
            user_id: caller(job),
        }),
        ..Default::default()
    };
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
    let Some(slice) = seed_slices(parts).into_iter().nth(index as usize) else {
        error!("Part {} out of range for {} parts", index, parts);
        return Ok(());
    };

    let start = Instant::now();
    let slice_len = u64::from(slice.end() - slice.start()) + 1;
    let progress = Arc::new(AtomicU64::new(0));
    let search_input = input.clone();
    let search_progress = Arc::clone(&progress);
    let search = tokio::task::spawn_blocking(move || {
        estimation25_lib::search_seeds(
            &search_input,
            &EstimConf::default(),
            slice,
            &search_progress,
        )
    });
    let deadline = sleep(Duration::from_secs(ESTIMATION_TIMEOUT_SECS));
    tokio::pin!(search, deadline);
    let period = Duration::from_secs(PROGRESS_REPORT_SECS);
    let mut ticks = interval_at(tokio::time::Instant::now() + period, period);
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
            }
        }
    };
    let matches = match searched {
        Some(Ok(Ok(matches))) => matches,
        Some(Ok(Err(e))) => return deliver(job, clients, &format!("❌ Erreur : {e}"), None).await,
        Some(Err(e)) => {
            error!("Estimation part {} of {} panicked: {}", index, run_id, e);
            return deliver(
                job,
                clients,
                "❌ La recherche a échoué. Veuillez réessayer.",
                None,
            )
            .await;
        }
        None => {
            error!("Estimation part {} of {} timed out", index, run_id);
            return deliver(
                job,
                clients,
                "⏱️ La recherche a expiré. Veuillez réessayer.",
                None,
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
            return deliver(
                job,
                clients,
                "❌ La recherche a échoué (enregistrement impossible). Veuillez réessayer.",
                None,
            )
            .await;
        }
    };
    let button = config_button_id(run_id);
    let (content, button) =
        match estimation25_lib::finish(&input, &EstimConf::default(), done.matches) {
            Ok(estimate) => (
                format!(
                    "{}\n-# ⏱️ 4294967296 runs testés en {} s",
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
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
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
